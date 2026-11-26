#pragma once

/// @file
/// Cycle: the registration of ports and start().

#include "hypha/backend/cycle.h"
#include "hypha/backend/session.h"
#include "hypha/cycle.h"
#include "hypha/cycle_clock_driver.h"
#include "hypha/cycle_port.h"
#include "hypha/node.h"

#include <algorithm>
#include <concepts>
#include <cstddef>
#include <cstdint>
#include <expected>
#include <memory>
#include <optional>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace hypha {

/// @brief A periodic cycle built by composition: ports register with it.
class Cycle
{
  public:
    explicit Cycle(std::string name);
    Cycle(const Cycle&) = delete;
    Cycle& operator=(const Cycle&) = delete;
    Cycle(Cycle&&) = delete;
    Cycle& operator=(Cycle&&) = delete;
    virtual ~Cycle();

    /// @brief Set the period of the cycle. Call before start(); after it throws
    ///        std::logic_error, as a port built after start() does.
    void setRate(Hz rate);

    /// @brief Step once per sample of Arrivals @p port instead of a rate (before start(),
    ///        not with setRate()). Each call adds a trigger; a repeat adds nothing. Every
    ///        trigger needs its own silence limit; a fallback step with no sample runs
    ///        after the shortest of them without one. Throws for a port of another cycle.
    void setTrigger(const Port& port);

    /// @brief Make this rate cycle drive `/clock` (before start(); needs setRate(), not
    ///        setTrigger(); the process must not be on simulated time yet).
    void setClockDriver(ClockDriverOptions options);

    /// @brief Bind every port, commit the cycle and call @p step on each tick.
    /// @p step is a named functor: void operator()(const CycleTime&). A failure rolls the
    /// partial setup back.
    template <typename Step>
        requires std::invocable<Step&, const CycleTime&>
    std::expected<void, std::string> start(Node& node, Step step);

    /// @brief True when every input that counts for health is usable and not stagnant,
    ///        as of the last capture.
    bool inputsOk() const
    {
        return std::ranges::all_of(this->ports_, [](const Port* port) {
            return port->inputOk();
        });
    }

    /// @brief The cumulative health of the cycle. InvalidState before start() and after release.
    [[nodiscard]] std::expected<CycleHealth, Status> health() const
    {
        const bool notRunning = this->cycle_ == nullptr || !this->nodeOwnsCycle();
        if (notRunning) {
            return std::unexpected(Status::InvalidState);
        }
        CycleHealth         health{};
        const ::HyphaStatus status = backend::cycleHealth(this->cycle_, &health);
        if (status != HYPHA_STATUS_OK) {
            return std::unexpected(Status(status));
        }
        return health;
    }

    /// @brief The name of the cycle. A CycleNode takes the node name at start().
    const std::string& name() const
    {
        return this->name_;
    }

  protected:
    /// @brief The CycleNode base: a cycle named after its node at start(), which reads
    ///        the `[cycle]` table.
    Cycle();

    /// @brief True when setRate() gave the cycle a rate.
    bool hasRate() const
    {
        return this->period_.nanos() != 0 || this->rateSet_;
    }

    /// @brief True when the last start() failed because the configuration of the cycle
    ///        was refused.
    bool startRefusedByConfiguration() const
    {
        return this->startRefusedByConfiguration_;
    }

  private:
    friend class Port;
    friend class testing::StepHarness;

    template <typename Step>
    std::expected<void, std::string> startBound(Node& node, HyphaWaitSet* waitset, Step step);
    /// Idempotent.
    void release() noexcept;
    bool nodeOwnsCycle() const noexcept;
    void refuseAfterStart(const char* call) const;
    void addPort(Port* port);
    void removePort(Port* port) noexcept;

    std::string                        name_;
    bool                               classBased_{false};
    bool                               rateSet_{false};
    Duration                           period_{};
    std::vector<Port*>                 ports_;
    std::vector<const Port*>           triggers_;
    std::optional<ClockDriverOptions>  clockDriver_{};
    bool                               started_{false};
    bool                               startRefusedByConfiguration_{false};
    bool                               harnessLive_{false};
    HyphaCycle*                        cycle_{nullptr};
    uint64_t                           generation_{0};
    std::shared_ptr<detail::CycleLink> link_;
};

inline Cycle::Cycle(std::string name) : name_(std::move(name))
{
}

inline Cycle::Cycle() : classBased_(true)
{
}

inline Cycle::~Cycle()
{
    this->release();
}

inline void Cycle::setRate(Hz rate)
{
    this->refuseAfterStart("setRate()");
    this->rateSet_ = true;
    this->period_ = detail::periodFromHz(rate);
}

inline void Cycle::setTrigger(const Port& port)
{
    this->refuseAfterStart("setTrigger()");
    if (std::ranges::find(this->ports_, &port) == this->ports_.end()) {
        throw std::invalid_argument("hypha: cycle '" + this->name_ + "': foreign trigger port");
    }
    if (std::ranges::find(this->triggers_, &port) == this->triggers_.end()) {
        this->triggers_.push_back(&port);
    }
}

inline void Cycle::setClockDriver(ClockDriverOptions options)
{
    this->refuseAfterStart("setClockDriver()");
    this->clockDriver_ = std::move(options);
}

inline void Cycle::refuseAfterStart(const char* call) const
{
    if (this->started_) {
        throw std::logic_error("hypha: cycle '" + this->name_ + "': " + call + " after start()");
    }
}

inline void Cycle::addPort(Port* port)
{
    if (this->started_) {
        throw std::logic_error(
            "hypha: cycle '" + this->name_ + "': a port is created after start()"
        );
    }
    this->ports_.push_back(port);
}

inline void Cycle::removePort(Port* port) noexcept
{
    std::erase(this->ports_, port);
    std::erase(this->triggers_, port);
}

template <typename Step>
    requires std::invocable<Step&, const CycleTime&>
std::expected<void, std::string> Cycle::start(Node& node, Step step)
{
    if (this->harnessLive_) {
        throw std::logic_error("hypha: cycle '" + this->name_ + "': a StepHarness holds it");
    }
    try {
        return this->startBound(node, backend::defaultWaitSet(), std::move(step));
    } catch (const std::bad_alloc&) {
        return std::unexpected(detail::cycleFailure(this->name_, "", HYPHA_STATUS_RESOURCE_LIMIT));
    } catch (...) {
        return std::unexpected(detail::cycleFailure(this->name_, "", HYPHA_STATUS_PANIC));
    }
}

template <typename Step>
std::expected<void, std::string> Cycle::startBound(Node& node, HyphaWaitSet* waitset, Step step)
{
    if (this->started_) {
        return std::unexpected("hypha: cycle '" + this->name_ + "': already started");
    }
    this->startRefusedByConfiguration_ = false;
    const bool takesNodeName = this->classBased_ && this->name_.empty();
    if (takesNodeName) {
        this->name_ = backend::nodeName();
    }
    // The cycle joins the session's node, which owns it from here on.
    static_cast<void>(node);
    if (waitset == nullptr) {
        return std::unexpected(detail::cycleFailure(this->name_, "", HYPHA_STATUS_INVALID_STATE));
    }

    using Runner = detail::CycleRunner<Step>;
    std::shared_ptr<detail::CycleLink> link = std::make_shared<detail::CycleLink>();
    link->name = this->name_;
    link->ports = this->ports_;
    std::unique_ptr<Runner> runner = std::make_unique<Runner>(std::move(step), link);
    std::expected<HyphaCycle*, ::HyphaStatus> created = backend::createCycle(
        waitset,
        this->name_,
        static_cast<uint64_t>(this->period_.nanos()),
        &Runner::run,
        runner.get(),
        &detail::destroyCycleContext<Runner>
    );
    if (!created) {
        return std::unexpected(detail::cycleFailure(this->name_, "", created.error()));
    }
    static_cast<void>(runner.release());
    HyphaCycle*             raw = *created;
    detail::CycleSetupGuard guard{raw};

    const ::HyphaStatus classBased = backend::cycleSetClassBased(raw, this->classBased_ ? 1 : 0);
    if (classBased != HYPHA_STATUS_OK) {
        return std::unexpected(detail::cycleFailure(this->name_, "", classBased));
    }

    for (Port* port : this->ports_) {
        const ::HyphaStatus bound = port->bind(raw);
        if (bound != HYPHA_STATUS_OK) {
            this->startRefusedByConfiguration_ =
                bound == HYPHA_STATUS_QOS_MISMATCH || port->bindRefusedByConfiguration();
            return std::unexpected(
                detail::cycleFailure(this->name_, port->describeFailure(), bound)
            );
        }
    }
    for (const Port* trigger : this->triggers_) {
        const std::optional<uint32_t> index = trigger->triggerIndex();
        if (!index.has_value()) {
            return std::unexpected(
                "hypha: cycle '" + this->name_ + "': the trigger must be an Arrivals port"
            );
        }
        const ::HyphaStatus set = backend::cycleSetTrigger(raw, *index);
        if (set != HYPHA_STATUS_OK) {
            return std::unexpected(
                detail::cycleFailure(this->name_, trigger->describeFailure(), set)
            );
        }
    }
    if (this->clockDriver_.has_value()) {
        const ::HyphaStatus set = detail::bindClockDriver(raw, *this->clockDriver_);
        if (set != HYPHA_STATUS_OK) {
            return std::unexpected(detail::cycleFailure(this->name_, "clock driver", set));
        }
    }
    const ::HyphaStatus committed = backend::cycleCommit(raw);
    if (committed != HYPHA_STATUS_OK) {
        this->startRefusedByConfiguration_ = committed == HYPHA_STATUS_INVALID_ARGUMENT;
        return std::unexpected(detail::cycleFailure(this->name_, "", committed));
    }
    guard.release();
    this->cycle_ = raw;
    this->generation_ = backend::sessionGeneration();
    this->link_ = link;
    this->started_ = true;
    return {};
}

inline bool Cycle::nodeOwnsCycle() const noexcept
{
    return backend::nodeHandle() != nullptr && backend::sessionGeneration() == this->generation_;
}

inline void Cycle::release() noexcept
{
    HyphaCycle* cycle = this->cycle_;
    if (cycle == nullptr) {
        return;
    }
    // Set before the destroy: a refused destroy must not leave ticks that reach the ports.
    this->link_->released = true;
    this->link_->ports.clear();
    this->cycle_ = nullptr;
    // A session that left the bus, or is shutting down, destroys the cycle itself.
    if (!this->nodeOwnsCycle()) {
        return;
    }
    const ::HyphaStatus status = backend::cycleDestroy(cycle);
    if (status == HYPHA_STATUS_OK) {
        return;
    }
    detail::warn("a cycle was released while its WaitSet runs; it fails at its next tick");
}

inline Port::Port(Cycle& owner) : owner_(owner)
{
    this->owner_.addPort(this);
}

inline Port::~Port()
{
    this->owner_.release();
    this->owner_.removePort(this);
}

inline ::HyphaStatus Port::bindInputSource(
    HyphaCycle*                  cycle,
    HyphaInputSource*            source,
    bool                         queued,
    const detail::InputSettings& settings,
    uint32_t&                    index
) const
{
    HyphaCycleBindingOptions raw{};
    raw.mode = queued ? HYPHA_CYCLE_INPUT_MODE_QUEUED : HYPHA_CYCLE_INPUT_MODE_LATEST;
    raw.optional = settings.countsForHealth ? 0 : 1;
    if (queued) {
        raw.max_samples_per_iteration = settings.perStep.value_or(HYPHA_DEFAULT_ARRIVALS_PER_STEP);
    }
    const ::HyphaStatus bound = backend::cycleBindInput(cycle, source, &raw, &index);
    const bool          nothingMore = bound != HYPHA_STATUS_OK || !settings.hostClock;
    if (nothingMore) {
        return bound;
    }
    return backend::cycleSetHostClock(cycle, index);
}

inline bool Port::started() const
{
    return this->owner_.started_;
}

inline void Port::refuseAfterStart(const char* call) const
{
    if (this->owner_.started_) {
        throw std::logic_error(
            std::string("hypha: cycle '") + this->owner_.name_ + "': " + call + " after start()"
        );
    }
}

inline HyphaCycle* Port::liveCycle() const
{
    const bool live = this->owner_.cycle_ != nullptr && this->owner_.nodeOwnsCycle();
    return live ? this->owner_.cycle_ : nullptr;
}

}  // namespace hypha
