/// mrs::PluginHost: name resolution, timers and the step.
#include <hypha/config.h>
#include <hypha_log/hypha_log.h>

#include <mrs_host/plugin_host.h>

#include <algorithm>
#include <atomic>
#include <mutex>

HYPHA_LOG_MODULE_INIT("mrs_host")

namespace mrs {

namespace {
std::atomic<uint64_t> paramErrors{0};
std::mutex            paramErrorMutex;
std::string           paramErrorText;

/// Join a namespace and a relative name with one '/'.
std::string join(const std::string& ns, const std::string& name)
{
    if (name.empty()) {
        return ns.empty() ? "/" : ns;
    }
    if (ns.empty() || ns == "/") {
        return "/" + name;
    }
    return ns + "/" + name;
}

/// Drop a trailing '/' and collapse "//".
std::string clean(std::string name)
{
    std::string out;
    for (char c : name) {
        if (c == '/' && !out.empty() && out.back() == '/') {
            continue;
        }
        out.push_back(c);
    }
    if (out.size() > 1 && out.back() == '/') {
        out.pop_back();
    }
    return out;
}
}  // namespace

uint64_t paramErrorCount()
{
    return paramErrors.load();
}

const std::string& lastParamError()
{
    const std::lock_guard lock(paramErrorMutex);
    return paramErrorText;
}

void recordParamError(const std::string& message)
{
    {
        const std::lock_guard lock(paramErrorMutex);
        paramErrorText = message;
    }
    paramErrors.fetch_add(1);
    HYPHA_LOG_ERROR("{}", message);
}

std::string parameterKey(const std::string& name)
{
    std::string key = name;
    if (key.starts_with("~/")) {
        key = key.substr(2);
    } else if (key.starts_with("~") || key.starts_with("/")) {
        key = key.substr(1);
    }
    std::ranges::replace(key, '/', '.');
    return key;
}

std::string portName(const std::string& topic)
{
    std::string name = topic.starts_with("/") ? topic.substr(1) : topic;
    std::ranges::replace(name, '/', '.');
    return name;
}

PluginHost::PluginHost(hypha::Cycle& cycle, std::string nodeName, std::string privateNamespace)
    : cycle_(cycle)
    , nodeName_(std::move(nodeName))
    , privateNamespace_(clean(std::move(privateNamespace)))
    , callerId_(this->nodeName_ + "/" + cycle.name())
{
    if (!this->privateNamespace_.starts_with("/")) {
        throw std::invalid_argument(
            "mrs: the private namespace must be absolute: " + this->privateNamespace_
        );
    }
    mrs::setNodeName(this->nodeName_);
    // A remap that is set but is not a list throws ParameterError.
    const std::optional<hypha::ConfigList> remap = hypha::config().optionalList("remap");
    if (!remap) {
        return;
    }
    for (const hypha::ConfigView entry : *remap) {
        const std::string from = entry.required<std::string>("from");
        const std::string to = entry.required<std::string>("to");
        this->remap_.emplace_back(
            this->resolveNamespace(this->privateNamespace_, from),
            this->resolveNamespace(this->privateNamespace_, to)
        );
    }
}

PluginHost::~PluginHost() = default;

std::string PluginHost::resolveNamespace(const std::string& parent, const std::string& name) const
{
    if (name.starts_with("/")) {
        return clean(name);
    }
    if (name == "~") {
        return this->privateNamespace_;
    }
    if (name.starts_with("~/")) {
        return clean(join(this->privateNamespace_, name.substr(2)));
    }
    if (name.starts_with("~")) {
        return clean(join(this->privateNamespace_, name.substr(1)));
    }
    return clean(join(parent, name));
}

std::string PluginHost::resolve(const std::string& ns, const std::string& name) const
{
    const std::string resolved = this->resolveNamespace(ns, name);
    for (const auto& [from, to] : this->remap_) {
        if (from == resolved) {
            return to;
        }
    }
    return resolved;
}

void PluginHost::refuseAfterStart(const std::string& what) const
{
    if (this->started_) {
        throw std::logic_error(
            "mrs: " + what
            + " is created after the cycle started; plugin endpoints belong in initialize()"
        );
    }
}

TimerId PluginHost::addTimer(
    hypha::Duration                             period,
    std::function<void(const ros::TimerEvent&)> fn,
    bool                                        autostart,
    bool                                        oneshot
)
{
    if (period.nanos() <= 0) {
        throw std::invalid_argument("mrs: a timer period must be positive");
    }
    Timer timer;
    timer.period = period;
    timer.fn = std::move(fn);
    timer.oneshot = oneshot;
    this->timers_.push_back(std::move(timer));
    const TimerId id = this->timers_.size() - 1;
    if (autostart) {
        this->startTimer(id);
    }
    return id;
}

void PluginHost::startTimer(TimerId id)
{
    Timer& timer = this->timers_.at(id);
    if (timer.running) {
        return;
    }
    timer.running = true;
    timer.due.reset();
    if (this->inStep_) {
        timer.due = this->stepTime_ + timer.period;
    }
}

void PluginHost::stopTimer(TimerId id)
{
    Timer& timer = this->timers_.at(id);
    timer.running = false;
    timer.due.reset();
}

void PluginHost::setPeriod(TimerId id, hypha::Duration period)
{
    if (period.nanos() <= 0) {
        throw std::invalid_argument("mrs: a timer period must be positive");
    }
    Timer& timer = this->timers_.at(id);
    timer.period = period;
    if (timer.running && this->inStep_) {
        timer.due = this->stepTime_ + period;
    }
}

bool PluginHost::timerRunning(TimerId id) const
{
    return this->timers_.at(id).running;
}

uint64_t PluginHost::timerMissedPeriods(TimerId id) const
{
    return this->timers_.at(id).missed;
}

HookId PluginHost::addHook(std::function<void()> fn)
{
    const HookId id = this->nextHook_++;
    this->hooks_.emplace(id, std::move(fn));
    return id;
}

void PluginHost::removeHook(HookId id)
{
    this->hooks_.erase(id);
}

hypha::Time PluginHost::now() const
{
    if (!this->inStep_) {
        throw std::logic_error("mrs: the step time is read outside a step");
    }
    return this->stepTime_;
}

void PluginHost::runTimers(hypha::Time now)
{
    // A callback may add or start timers; index, not iterators.
    for (size_t index = 0; index < this->timers_.size(); ++index) {
        Timer& timer = this->timers_[index];
        if (!timer.running) {
            continue;
        }
        if (!timer.due.has_value()) {
            timer.due = now + timer.period;
            continue;
        }
        if (now < *timer.due) {
            continue;
        }
        const hypha::Time expected = *timer.due;
        const int64_t     period = timer.period.nanos();
        const int64_t     late = (now - expected).nanos();
        const int64_t     skipped = late / period;
        timer.missed += static_cast<uint64_t>(skipped);
        timer.due = expected + hypha::Duration::fromNanos((skipped + 1) * period);
        ros::TimerEvent event;
        event.current_expected = ros::Time::fromHypha(expected);
        event.current_real = ros::Time::fromHypha(now);
        if (timer.lastExpected) {
            event.last_expected = ros::Time::fromHypha(*timer.lastExpected);
            event.last_real = ros::Time::fromHypha(*timer.lastReal);
        }
        timer.lastExpected = expected;
        timer.lastReal = now;
        if (timer.oneshot) {
            timer.running = false;
            timer.due.reset();
        }
        // Copy: the callback may add a timer, which can move the vector.
        const std::function<void(const ros::TimerEvent&)> fn = timer.fn;
        fn(event);
    }
}

void PluginHost::step(const hypha::CycleTime& time, const std::function<void()>& body)
{
    this->started_ = true;
    this->inStep_ = true;
    this->stepTime_ = time.now;
    mrs::setCurrentStepTime(time.now);

    struct Leave
    {
        PluginHost* host;

        ~Leave()
        {
            host->inStep_ = false;
            mrs::setCurrentStepTime(std::nullopt);
        }
    } leave{this};

    for (const std::function<void()>& dispatch : this->dispatchers_) {
        dispatch();
    }
    // Copy: a hook may remove itself.
    const std::map<HookId, std::function<void()>> hooks = this->hooks_;
    for (const auto& [id, hook] : hooks) {
        static_cast<void>(id);
        hook();
    }
    this->runTimers(time.now);
    if (body) {
        body();
    }
}

}  // namespace mrs
