#pragma once

/// @file
/// mrs::PluginHost: the endpoints of upstream MRS plugin code as ports and dividers of
/// the hosting manager's hypha cycle. A plugin creates them in initialize(), before the
/// cycle starts; creating one after the cycle started throws std::logic_error. An output
/// takes its topic from the node's TOML, [ports.outputs.<name>] with `topic`.

#include <hypha/cycle_transforms.h>
#include <hypha/hypha.h>

#include <mrs_host/qos_policies.h>
#include <ros/ros.h>

#include <cstdint>
#include <functional>
#include <map>
#include <memory>
#include <optional>
#include <stdexcept>
#include <string>
#include <typeinfo>
#include <utility>
#include <vector>

namespace mrs {

using TimerId = size_t;
using HookId = size_t;

/// How many required parameters were missing, and the last message, for tests.
uint64_t           paramErrorCount();
const std::string& lastParamError();
void               recordParamError(const std::string& message);

/// The TOML key of a ROS parameter name: '/' becomes '.', a leading '/' or '~/' is dropped.
std::string parameterKey(const std::string& name);

/// The port name of a resolved topic: no leading '/', '/' becomes '.'.
std::string portName(const std::string& topic);

class PluginHost
{
  public:
    /// @p privateNamespace is the ROS private namespace of the manager ("/uav1/control_manager"),
    /// against which "~name" resolves. The node's TOML may hold `remap = [{from, to}, ...]`.
    PluginHost(hypha::Cycle& cycle, std::string nodeName, std::string privateNamespace);
    PluginHost(const PluginHost&) = delete;
    PluginHost& operator=(const PluginHost&) = delete;
    ~PluginHost();

    const std::string& nodeName() const
    {
        return this->nodeName_;
    }

    const std::string& privateNamespace() const
    {
        return this->privateNamespace_;
    }

    /// The identity of this host's requests: node name and cycle name.
    const std::string& callerId() const
    {
        return this->callerId_;
    }

    /// ROS name resolution against @p ns ("/abs", "~private", "relative"), then the remap table.
    std::string resolve(const std::string& ns, const std::string& name) const;
    /// The namespace of a NodeHandle(parent, name): resolved like a name, without remapping.
    std::string resolveNamespace(const std::string& parent, const std::string& name) const;

    /// The newest value of @p topic (a resolved topic). One port per topic.
    template <class M>
    hypha::Latest<M>& latest(const std::string& topic);
    /// Every sample of @p topic in arrival order. One port per topic.
    template <class M>
    hypha::Arrivals<M>& arrivals(const std::string& topic, uint32_t perStep = 16);
    /// An output on @p topic (a resolved topic). One port per topic.
    template <class M>
    hypha::Output<M>& output(const std::string& topic, uint32_t perStep = 16, bool latch = false);
    /// Serve @p topic from a port the manager owns (its trigger), so a plugin that
    /// subscribes to it sees the sample of the step, not the previous one. Before start().
    template <class M>
    void adoptArrivals(const std::string& topic, hypha::Arrivals<M>& port);

    /// The cycle's TF input on /tf and /tf_static, created on the first call with
    /// @p cache. Every mrs_lib::Transformer of this host shares it. Before start().
    hypha::Transforms& transforms(hypha::Duration cache)
    {
        if (!this->transforms_) {
            this->refuseAfterStart("TF input");
            this->transforms_ = std::make_unique<hypha::Transforms>(this->cycle_, cache);
        }
        return *this->transforms_;
    }

    /// True when @p topic has an Arrivals port (own or adopted).
    bool hasArrivals(const std::string& topic) const
    {
        return this->arrivals_.contains(topic) || this->adopted_.contains(topic);
    }
    /// Run @p fn in step() once per new sample of @p topic, oldest first.
    template <class M>
    void onArrival(const std::string& topic, std::function<void(const M&)> fn);

    /// A timer. It fires on the first step at or after its due time; the due time then
    /// advances by whole periods past the step time, with no burst; skipped periods are
    /// counted. A timer started outside a step is due one period after the next step.
    TimerId addTimer(
        hypha::Duration                             period,
        std::function<void(const ros::TimerEvent&)> fn,
        bool                                        autostart,
        bool                                        oneshot = false
    );
    void     startTimer(TimerId id);
    void     stopTimer(TimerId id);
    void     setPeriod(TimerId id, hypha::Duration period);
    bool     timerRunning(TimerId id) const;
    uint64_t timerMissedPeriods(TimerId id) const;

    /// A service of this process, callable directly: a process calling itself calls the
    /// function, never its own topics. NodeHandle::advertiseService registers each one.
    template <class Req, class Res>
    void registerLocalService(const std::string& name, std::function<bool(Req&, Res&)> fn)
    {
        this->local_[name] = LocalService{
            &typeid(Req),
            &typeid(Res),
            std::make_shared<std::function<bool(Req&, Res&)>>(std::move(fn))
        };
    }

    /// Call the local service @p name. @throws std::logic_error when it does not exist.
    template <class Req, class Res>
    bool callLocal(const std::string& name, Req& req, Res& res) const
    {
        const auto found = this->local_.find(name);
        if (found == this->local_.end() || *found->second.request != typeid(Req)
            || *found->second.response != typeid(Res)) {
            throw std::logic_error("mrs: no local service '" + name + "' of that type");
        }
        return (*std::static_pointer_cast<std::function<bool(Req&, Res&)>>(found->second.fn))(
            req, res
        );
    }

    /// A hook that step() runs after the subscriber callbacks, before the timers.
    HookId addHook(std::function<void()> fn);
    void   removeHook(HookId id);

    /// Run one step: callbacks, hooks, timers, then @p body, all at the step's time.
    void step(const hypha::CycleTime& time, const std::function<void()>& body = {});

    /// True once the first step ran. Creating an endpoint then throws std::logic_error.
    bool started() const
    {
        return this->started_;
    }

    /// The scheduled time of the running step. @throws std::logic_error outside a step.
    hypha::Time now() const;

    bool inStep() const
    {
        return this->inStep_;
    }

  private:
    struct Port
    {
        const std::type_info*        type;
        std::unique_ptr<hypha::Port> port;
    };

    /// The first port of a topic: later ports of the topic share its subscriber.
    struct FirstPort
    {
        const std::type_info* type;
        const void*           port;
    };

    struct LocalService
    {
        const std::type_info* request;
        const std::type_info* response;
        std::shared_ptr<void> fn;
    };

    struct Timer
    {
        hypha::Duration                             period;
        std::function<void(const ros::TimerEvent&)> fn;
        bool                                        running{false};
        bool                                        oneshot{false};
        std::optional<hypha::Time>                  due;
        std::optional<hypha::Time>                  lastExpected;
        std::optional<hypha::Time>                  lastReal;
        uint64_t                                    missed{0};
    };

    void refuseAfterStart(const std::string& what) const;
    /// A new input port of @p topic with @p options: the first one subscribes, the others
    /// share it.
    template <class M, template <class> class PortType, class... Options>
    std::unique_ptr<PortType<M>> inputPort(const std::string& topic, Options... options);
    void                         runTimers(hypha::Time now);

    hypha::Cycle&                                                         cycle_;
    std::string                                                           nodeName_;
    std::string                                                           privateNamespace_;
    std::string                                                           callerId_;
    std::vector<std::pair<std::string, std::string>>                      remap_;
    std::map<std::string, FirstPort>                                      firstPorts_;
    std::map<std::string, Port>                                           latest_;
    std::map<std::string, Port>                                           arrivals_;
    std::map<std::string, Port>                                           outputs_;
    std::map<std::string, std::pair<const std::type_info*, hypha::Port*>> adopted_;
    std::vector<std::function<void()>>                                    dispatchers_;
    std::map<HookId, std::function<void()>>                               hooks_;
    HookId                                                                nextHook_{0};
    std::vector<Timer>                                                    timers_;
    std::map<std::string, LocalService>                                   local_;
    std::unique_ptr<hypha::Transforms>                                    transforms_;
    bool                                                                  started_{false};
    bool                                                                  inStep_{false};
    hypha::Time                                                           stepTime_{};
};

template <class M, template <class> class PortType, class... Options>
std::unique_ptr<PortType<M>> PluginHost::inputPort(const std::string& topic, Options... options)
{
    const auto found = this->firstPorts_.find(topic);
    if (found == this->firstPorts_.end()) {
        auto port = std::make_unique<PortType<M>>(this->cycle_, hypha::Topic{topic}, options...);
        const hypha::Input<M>& first = *port;
        this->firstPorts_.emplace(topic, FirstPort{&typeid(M), &first});
        return port;
    }
    if (*found->second.type != typeid(M)) {
        throw std::logic_error("mrs: topic '" + topic + "' is subscribed with two message types");
    }
    return std::make_unique<PortType<M>>(
        this->cycle_,
        hypha::sharing(*static_cast<const hypha::Input<M>*>(found->second.port)),
        options...
    );
}

template <class M>
hypha::Latest<M>& PluginHost::latest(const std::string& topic)
{
    auto found = this->latest_.find(topic);
    if (found != this->latest_.end()) {
        if (*found->second.type != typeid(M)) {
            throw std::logic_error("mrs: topic '" + topic + "' is read with two message types");
        }
        return static_cast<hypha::Latest<M>&>(*found->second.port);
    }
    this->refuseAfterStart("subscriber '" + topic + "'");
    auto port = this->inputPort<M, hypha::Latest>(topic, hypha::advisory, hypha::unsupervised);
    hypha::Latest<M>& ref = *port;
    this->latest_.emplace(topic, Port{&typeid(M), std::move(port)});
    return ref;
}

template <class M>
void PluginHost::adoptArrivals(const std::string& topic, hypha::Arrivals<M>& port)
{
    this->refuseAfterStart("adopted port '" + topic + "'");
    if (this->hasArrivals(topic)) {
        throw std::logic_error("mrs: topic '" + topic + "' already has an Arrivals port");
    }
    this->adopted_.emplace(
        topic, std::pair<const std::type_info*, hypha::Port*>{&typeid(M), &port}
    );
}

template <class M>
hypha::Arrivals<M>& PluginHost::arrivals(const std::string& topic, uint32_t perStep)
{
    const auto adopted = this->adopted_.find(topic);
    if (adopted != this->adopted_.end()) {
        if (*adopted->second.first != typeid(M)) {
            throw std::logic_error("mrs: topic '" + topic + "' is read with two message types");
        }
        return static_cast<hypha::Arrivals<M>&>(*adopted->second.second);
    }
    auto found = this->arrivals_.find(topic);
    if (found != this->arrivals_.end()) {
        if (*found->second.type != typeid(M)) {
            throw std::logic_error("mrs: topic '" + topic + "' is read with two message types");
        }
        return static_cast<hypha::Arrivals<M>&>(*found->second.port);
    }
    this->refuseAfterStart("subscriber '" + topic + "'");
    auto port = this->inputPort<M, hypha::Arrivals>(
        topic, hypha::advisory, hypha::unsupervised, hypha::perStep(perStep)
    );
    hypha::Arrivals<M>& ref = *port;
    this->arrivals_.emplace(topic, Port{&typeid(M), std::move(port)});
    return ref;
}

template <class M>
hypha::Output<M>& PluginHost::output(const std::string& topic, uint32_t perStep, bool latch)
{
    auto found = this->outputs_.find(topic);
    if (found != this->outputs_.end()) {
        if (*found->second.type != typeid(M)) {
            throw std::logic_error(
                "mrs: topic '" + topic + "' is published with two message types"
            );
        }
        return static_cast<hypha::Output<M>&>(*found->second.port);
    }
    this->refuseAfterStart("publisher '" + topic + "'");
    const std::string                name = portName(topic);
    const std::string                key = "ports.outputs." + name + ".topic";
    const std::optional<std::string> configured = hypha::config().optional<std::string>(key);
    if (!configured.has_value() || *configured != topic) {
        throw std::runtime_error(
            "mrs: the output '" + topic + "' needs [ports.outputs." + name + "] with topic = \""
            + topic + "\" in the node's TOML" + (configured ? ", not \"" + *configured + "\"" : "")
        );
    }
    auto port = std::make_unique<hypha::Output<M>>(
        this->cycle_, name, hypha::perStep(perStep), hypha::latched(latch)
    );
    hypha::Output<M>& ref = *port;
    this->outputs_.emplace(topic, Port{&typeid(M), std::move(port)});
    return ref;
}

template <class M>
void PluginHost::onArrival(const std::string& topic, std::function<void(const M&)> fn)
{
    hypha::Arrivals<M>& port = this->arrivals<M>(topic);
    this->dispatchers_.push_back([&port, fn = std::move(fn)] {
        for (const M& sample : port.samples()) {
            fn(sample);
        }
    });
}

}  // namespace mrs
