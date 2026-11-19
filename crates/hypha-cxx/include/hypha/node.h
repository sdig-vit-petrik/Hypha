#pragma once

#include "hypha/backend/introspection.h"
#include "hypha/backend/session.h"
#include "hypha/backend/timer.h"
#include "hypha/callback.h"
#include "hypha/options.h"
#include "hypha/publisher.h"
#include "hypha/raw.h"
#include "hypha/raw_api.h"
#include "hypha/status.h"
#include "hypha/subscriber.h"
#include "hypha/topic.h"
#include "hypha_base/clock.h"

#include <atomic>
#include <expected>
#include <functional>
#include <new>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

namespace hypha {

class Timer
{
  public:
    Timer() = default;

    explicit Timer(backend::TimerHandle handle)
        : held(std::make_shared<backend::TimerHandle>(std::move(handle)))
    {
    }

    bool isValid() const
    {
        return this->held && this->held->isValid();
    }

    /// @return Periods this timer never fired in, because a callback or the run
    ///         loop ran longer than one period.
    uint64_t missedCount() const
    {
        return this->held ? this->held->missedCount() : 0U;
    }

  private:
    std::shared_ptr<backend::TimerHandle> held;
};

/// @return False once the process has been asked to stop, e.g. by SIGINT.
inline bool ok()
{
    return backend::ok();
}

/// @return The name in effect: the `node.name` key of the parameter file wins over code.
inline std::string nodeName()
{
    return backend::nodeName();
}

/// @brief Ask a running Node::run() to return. The node keeps running.
///
/// The one operation another thread may call on a running node. A stop requested before
/// run() starts is kept: the next run returns at once.
inline void requestStop()
{
    backend::runStop();
}

/// @brief A node handle that cleans up after itself: constructing one joins the bus,
///        destroying one leaves it.
///
/// @code
/// int main()
/// {
///     hypha::Node node("my_node");
///     auto in = node.subscribe<Scan>(hypha::Topic{"/scan"}, onScan);
///     return node.run().ok() ? 0 : 1;
/// }   // the bus is released here
/// @endcode
///
/// A second Node in the same process shares the session.
class Node
{
  public:
    /// A handle that owns no session; its members use the process session when one exists.
    Node() = default;

    /// Leaves the bus when this node created the session. Leaving destroys every cycle of
    /// the session; endpoints and ports must not outlive their Node.
    ~Node()
    {
        if (this->ownsSession_) {
            backend::shutdown();
        }
    }

    Node(const Node&) = delete;
    Node& operator=(const Node&) = delete;
    Node(Node&&) = delete;
    Node& operator=(Node&&) = delete;

    explicit Node(const std::string& name) : Node(name, std::string{})
    {
    }

    /// A name is `[a-z][a-z0-9_]`, at most 64 characters; the `node.name` key of
    /// @p parameterFile overrides it. A second Node keeps the first name.
    /// @throws std::runtime_error when the node cannot be created.
    Node(const std::string& name, const std::string& parameterFile)
    {
        bool         created = false;
        const Status status = backend::init(name, parameterFile, &created);
        this->ownsSession_ = created;
        if (status != Status::Ok) {
            throw std::runtime_error(detail::failureText("node '" + name + "' creation", status));
        }
    }

    /// @brief A publisher on the topic of `[ports.outputs.<port>]`. Options: optionalTopic,
    ///        latched, maxPayloadDynamic(), a Qos. With optionalTopic a table without a
    ///        topic gives a disabled publisher. Create endpoints before run().
    /// @throws std::runtime_error naming the port when the endpoint is refused.
    template <typename M, PublisherOption... Options>
    [[nodiscard]] Publisher<M> advertise(std::string port, Options... options)
    {
        const detail::EndpointSettings settings =
            detail::settingsOf<detail::EndpointSettings>(options...);
        return Publisher<M>(
            detail::TopicSource::ofPort(std::move(port), true, settings.optionalTopic), settings
        );
    }

    /// @brief A publisher on a topic that code fixes: `advertise<M>(hypha::Topic{"/tf"})`.
    ///        Options: latched, maxPayloadDynamic(), a Qos.
    /// @throws std::runtime_error naming the topic when the endpoint is refused.
    template <typename M, PublisherOption... Options>
        requires(detail::FixedTopicOption<Options> && ...)
    [[nodiscard]] Publisher<M> advertise(Topic topic, Options... options)
    {
        return Publisher<M>(
            detail::TopicSource::ofLiteral(std::move(topic)),
            detail::settingsOf<detail::EndpointSettings>(options...)
        );
    }

    /// @brief A subscriber on the topic of `[ports.inputs.<port>]`. The run calls
    ///        @p callback for each sample. Options: optionalTopic, a Qos. With
    ///        optionalTopic a table without a topic gives a disabled subscriber. Create
    ///        endpoints before run().
    /// @throws std::runtime_error naming the port when the endpoint is refused.
    template <typename M, SubscriberOption... Options>
    [[nodiscard]] Subscriber<M>
    subscribe(std::string port, typename Subscriber<M>::Callback callback = {}, Options... options)
    {
        const detail::EndpointSettings settings =
            detail::settingsOf<detail::EndpointSettings>(options...);
        return Subscriber<M>(
            detail::TopicSource::ofPort(std::move(port), false, settings.optionalTopic),
            std::move(callback),
            settings
        );
    }

    /// @brief A subscriber on a topic, or a wildcard pattern, that code fixes. Option: a Qos.
    /// @throws std::runtime_error naming the topic when the endpoint is refused.
    template <typename M, SubscriberOption... Options>
        requires(detail::FixedTopicOption<Options> && ...)
    [[nodiscard]] Subscriber<M>
    subscribe(Topic topic, typename Subscriber<M>::Callback callback = {}, Options... options)
    {
        return Subscriber<M>(
            detail::TopicSource::ofLiteral(std::move(topic)),
            std::move(callback),
            detail::settingsOf<detail::EndpointSettings>(options...)
        );
    }

    /// @brief Run `callback` every `period` of framework time. Create timers before run().
    /// @throws std::runtime_error when the timer is refused: a period that is not
    ///         positive (InvalidArgument), or no session (InvalidState).
    [[nodiscard]] Timer timer(Duration period, std::function<void()> callback)
    {
        return Timer(require(backend::createTimer(period, std::move(callback)), "timer"));
    }

    /// @brief Run this node's callbacks on the calling thread until it stops.
    ///
    /// Attach every subscription and timer before this call: a subscription created
    /// during a run is refused.
    ///
    /// @return The outcome of both the call and the run; check ok().
    [[nodiscard]] RunResult run()
    {
        return backend::run();
    }

    /// @brief Run this node's callbacks for at most @p limit, then return.
    /// A limit of zero or less is refused with InvalidArgument.
    [[nodiscard]] RunResult runFor(Duration limit)
    {
        return backend::runFor(limit);
    }

  private:
    bool ownsSession_{false};
};

}  // namespace hypha
