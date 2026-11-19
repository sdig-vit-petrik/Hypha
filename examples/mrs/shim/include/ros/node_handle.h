#pragma once

/// @file
/// ros::NodeHandle over mrs::PluginHost: a namespace, timers and services on the host's
/// cycle. Topics are created through mrs_lib::SubscribeHandler and PublisherHandler.

#include <mrs_host/plugin_host.h>
#include <mrs_host/service.h>
#include <ros/ros.h>

#include <functional>
#include <memory>
#include <string>

namespace ros {

/// A timer of the host's cycle. Copyable; copies control the same timer.
class Timer
{
  public:
    Timer() = default;

    Timer(mrs::PluginHost* host, mrs::TimerId id) : host_(host), id_(id)
    {
    }

    void start()
    {
        this->host().startTimer(this->id_);
    }

    void stop()
    {
        this->host().stopTimer(this->id_);
    }

    void setPeriod(const Duration& period, bool = true)
    {
        this->host().setPeriod(this->id_, period.toHypha());
    }

    bool hasStarted() const
    {
        return this->host_ != nullptr && this->host_->timerRunning(this->id_);
    }

    bool isValid() const
    {
        return this->host_ != nullptr;
    }

  private:
    mrs::PluginHost& host() const
    {
        if (this->host_ == nullptr) {
            throw std::logic_error("ros::Timer: used before createTimer()");
        }
        return *this->host_;
    }

    mrs::PluginHost* host_{nullptr};
    mrs::TimerId     id_{0};
};

/// Holds the server of an advertised service.
class ServiceServer
{
  public:
    ServiceServer() = default;

    explicit ServiceServer(std::shared_ptr<void> server) : server_(std::move(server))
    {
    }

    void shutdown()
    {
        this->server_.reset();
    }

  private:
    std::shared_ptr<void> server_;
};

class NodeHandle
{
  public:
    NodeHandle() = default;

    /// A handle in namespace @p ns ("~" is the host's private namespace).
    NodeHandle(mrs::PluginHost& host, const std::string& ns)
        : host_(&host), ns_(host.resolveNamespace(host.privateNamespace(), ns))
    {
    }

    /// A child handle: @p ns relative to @p parent's namespace.
    NodeHandle(const NodeHandle& parent, const std::string& ns)
        : host_(parent.host_), ns_(parent.host().resolveNamespace(parent.ns_, ns))
    {
    }

    const std::string& getNamespace() const
    {
        return this->ns_;
    }

    /// The topic or service name @p name resolves to, remapping included.
    std::string resolveName(const std::string& name) const
    {
        return this->host().resolve(this->ns_, name);
    }

    mrs::PluginHost& host() const
    {
        if (this->host_ == nullptr) {
            throw std::logic_error("ros::NodeHandle: default-constructed handle used");
        }
        return *this->host_;
    }

    bool valid() const
    {
        return this->host_ != nullptr;
    }

    template <class Class>
    Timer createTimer(
        const Rate& rate,
        void (Class::*callback)(const TimerEvent&),
        Class* object,
        bool   oneshot = false,
        bool   autostart = true
    ) const
    {
        return this->createTimer(rate.expectedCycleTime(), callback, object, oneshot, autostart);
    }

    template <class Class>
    Timer createTimer(
        const Duration& period,
        void (Class::*callback)(const TimerEvent&),
        Class* object,
        bool   oneshot = false,
        bool   autostart = true
    ) const
    {
        const mrs::TimerId id = this->host().addTimer(
            period.toHypha(),
            [object, callback](const TimerEvent& event) {
                (object->*callback)(event);
            },
            autostart,
            oneshot
        );
        return Timer(this->host_, id);
    }

    Timer createTimer(
        const Rate&                            rate,
        std::function<void(const TimerEvent&)> callback,
        bool                                   oneshot = false,
        bool                                   autostart = true
    ) const
    {
        return this->createTimer(rate.expectedCycleTime(), std::move(callback), oneshot, autostart);
    }

    Timer createTimer(
        const Duration&                        period,
        std::function<void(const TimerEvent&)> callback,
        bool                                   oneshot = false,
        bool                                   autostart = true
    ) const
    {
        return Timer(
            this->host_,
            this->host().addTimer(period.toHypha(), std::move(callback), autostart, oneshot)
        );
    }

    /// A service server: `bool callback(Srv::Request&, Srv::Response&)`. A false return
    /// refuses the call.
    template <class Class, class Req, class Res>
    ServiceServer advertiseService(
        const std::string& name, bool (Class::*callback)(Req&, Res&), Class* object
    ) const
    {
        using Server = mrs::Serve<mrs::ServicePair<Req, Res>>;
        auto                                   holder = std::make_shared<std::unique_ptr<Server>>();
        std::weak_ptr<std::unique_ptr<Server>> self = holder;
        *holder = std::make_unique<Server>(
            this->host(),
            this->resolveName(name),
            [object, callback, self](uint64_t ticket, const Req& request) -> std::optional<Res> {
                Req copy = request;
                Res response;
                if ((object->*callback)(copy, response)) {
                    return response;
                }
                if (const auto server = self.lock()) {
                    (**server).refuse(ticket);
                }
                return std::nullopt;
            }
        );
        this->host().template registerLocalService<Req, Res>(
            this->resolveName(name), [object, callback](Req& req, Res& res) {
                return (object->*callback)(req, res);
            }
        );
        return ServiceServer(holder);
    }

  private:
    mrs::PluginHost* host_{nullptr};
    std::string      ns_;
};

}  // namespace ros
