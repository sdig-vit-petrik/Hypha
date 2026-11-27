#pragma once

/// @file
/// The endpoints of services: ServiceServer, ServiceClient<Srv> (outcomes on the node's
/// run thread) and BlockingServiceClient<Srv> (for a worker thread, a test or a tool).
/// Node::advertiseService and Node::serviceClient create them.

#include "hypha/backend/callbacks.h"
#include "hypha/backend/service.h"
#include "hypha/detail/report.h"
#include "hypha/service.h"

#include <algorithm>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <type_traits>
#include <utility>
#include <vector>

namespace hypha {

/// @brief The tag of a blocking client: `node.serviceClient<Srv>(name, hypha::blocking)`.
struct Blocking
{
};

inline constexpr Blocking blocking{};

/// @brief A client's timeout set in code: `node.serviceClient<Srv>(name, hypha::timeout(d))`.
///        `timeout_ms` in the client's table wins over it, and it over the policy's.
struct Timeout
{
    Duration value{};
};

inline Timeout timeout(Duration value)
{
    return Timeout{value};
}

/// @brief A server, created by Node::advertiseService. Move-only; destroying it ends every
///        request it holds: their clients read ServerLost.
class ServiceServer
{
  public:
    ServiceServer() = default;

    explicit ServiceServer(backend::UniqueServer held) : held_(std::move(held))
    {
    }

    bool isValid() const
    {
        return static_cast<bool>(this->held_);
    }

  private:
    backend::UniqueServer held_;
};

/// @brief A client whose outcomes the node's run delivers. Node::serviceClient creates it;
///        a server need not exist. Move-only.
template <typename Srv>
class ServiceClient
{
  public:
    using OnResult = std::function<void(const CallResult<Srv>&)>;

    ServiceClient() = default;

    explicit ServiceClient(backend::UniqueClient held) : held_(std::move(held))
    {
    }

    /// @brief Send @p request. On Sent, @p onResult runs once with the outcome on the run's
    ///        thread at a later turn, never inside call(); a throw from it fails the run. On
    ///        Busy or TooLarge nothing was sent and it never runs.
    /// @throws std::runtime_error when the library refuses the call itself.
    [[nodiscard]] CallStatus call(const typename Srv::Request& request, OnResult onResult)
    {
        const std::vector<uint8_t> bytes = codec::encode(request);
        auto*                      held = new OnResult(std::move(onResult));
        uint32_t                   status = HYPHA_CALL_BUSY;
        const Status               called = hypha_client_call(
            this->held_.get(), bytes.data(), bytes.size(), &deliver, held, &release, &status
        );
        if (called != Status::Ok) {
            backend::throwCallFailure("service call", called);
        }
        return static_cast<CallStatus>(status);
    }

    /// A ROS 1 `client.call(srv)` does not port as written.
    template <typename T>
        requires std::is_same_v<std::remove_cvref_t<T>, Srv>
    CallStatus call(T&&, OnResult = {})
    {
        static_assert(
            !std::is_same_v<T, T>,
            "call(Srv&) does not exist: pass srv.request, and read the result in onResult "
            "(ros1-migration-guide.md, services)"
        );
        return CallStatus::Busy;
    }

    /// True when a live server of the stream exists now.
    bool ready() const
    {
        return backend::clientStatus(this->held_.get()).ready != 0;
    }

    /// The ROS 1 spelling of ready().
    bool exists() const
    {
        return this->ready();
    }

    /// True while a call's outcome is still to come.
    bool pending() const
    {
        return backend::clientStatus(this->held_.get()).pending != 0;
    }

  private:
    static HyphaCallbackResult deliver(const HyphaCallResult* raw, void* context) noexcept
    {
        auto* onResult = static_cast<OnResult*>(context);
        if (raw == nullptr || onResult == nullptr || !*onResult) {
            return backend::ffi::continueRun();
        }
        try {
            (*onResult)(CallResult<Srv>::of(*raw));
        } catch (...) {
            detail::error("a service call's onResult threw; the run failed");
            return backend::ffi::failRun(HYPHA_STATUS_PANIC);
        }
        return backend::ffi::continueRun();
    }

    static void release(void* context) noexcept
    {
        delete static_cast<OnResult*>(context);
    }

    backend::UniqueClient held_;
};

/// @brief A client whose call waits on the calling thread, for a worker thread of an
///        ApplicationNode, a test or a tool. It is attached to no WaitSet. Move-only.
template <typename Srv>
class BlockingServiceClient
{
  public:
    BlockingServiceClient() = default;

    explicit BlockingServiceClient(backend::UniqueClient held) : held_(std::move(held))
    {
    }

    /// @brief Send @p request and wait for its outcome: at most the client's timeout, after
    ///        a server appears or not. A call refused before anything was sent ends NotSent.
    /// @throws std::runtime_error on a thread that runs the node (InvalidState): that
    ///         thread must deliver the answer; use ServiceClient::call there.
    CallResult<Srv> callAndWait(const typename Srv::Request& request)
    {
        const std::vector<uint8_t>     bytes = codec::encode(request);
        std::optional<CallResult<Srv>> result;
        uint32_t                       status = HYPHA_CALL_BUSY;
        const Status                   called = hypha_client_call(
            this->held_.get(), bytes.data(), bytes.size(), &keep, &result, nullptr, &status
        );
        if (called != Status::Ok) {
            backend::throwCallFailure("callAndWait", called);
        }
        if (!result) {
            return CallResult<Srv>::notSent(static_cast<CallStatus>(status));
        }
        return std::move(*result);
    }

    /// @brief Wait up to @p limit for a live server.
    /// @throws std::runtime_error on a thread that runs the node (InvalidState).
    bool waitForServer(Duration limit)
    {
        uint8_t      ready = 0;
        const auto   nanos = static_cast<uint64_t>(std::max<int64_t>(limit.nanos(), 0));
        const Status status = hypha_client_wait_for_server(this->held_.get(), nanos, &ready);
        if (status != Status::Ok) {
            backend::throwCallFailure("waitForServer", status);
        }
        return ready != 0;
    }

    bool ready() const
    {
        return backend::clientStatus(this->held_.get()).ready != 0;
    }

    bool exists() const
    {
        return this->ready();
    }

  private:
    static HyphaCallbackResult keep(const HyphaCallResult* raw, void* context) noexcept
    {
        auto* result = static_cast<std::optional<CallResult<Srv>>*>(context);
        if (raw != nullptr && result != nullptr) {
            try {
                result->emplace(CallResult<Srv>::of(*raw));
            } catch (...) {
                // An allocation failure leaves the result empty: the call reads NotSent.
            }
        }
        return backend::ffi::continueRun();
    }

    backend::UniqueClient held_;
};

namespace detail {

/// The handler of a server of @p Srv, owned by the library through its context.
template <typename Srv>
struct ServerState
{
    std::function<void(Request<Srv>&)> handler;
    std::string                        name;
    RecurringFault                     failures;

    static HyphaCallbackResult serve(const HyphaRequestView* view, void* context) noexcept
    {
        auto* state = static_cast<ServerState*>(context);
        if (view == nullptr || state == nullptr) {
            return backend::ffi::continueRun();
        }
        try {
            typename Srv::Request message;
            if (!codec::decode(view->payload, view->payload_len, message)) {
                const std::string why = std::string("invalid request: it does not decode as ")
                                      + codec::dataType<typename Srv::Request>();
                static_cast<void>(hypha_request_answer(
                    view->request,
                    HYPHA_ANSWER_REFUSE,
                    reinterpret_cast<const uint8_t*>(why.data()),
                    why.size()
                ));
                return backend::ffi::continueRun();
            }
            Request<Srv> request(
                view->request,
                std::move(message),
                std::string(view->client_node, view->client_node_len)
            );
            state->handler(request);
        } catch (...) {
            state->failures.note("the handler of " + state->name + " threw; the run failed");
            return backend::ffi::failRun(HYPHA_STATUS_PANIC);
        }
        return backend::ffi::continueRun();
    }

    static void release(void* context) noexcept
    {
        delete static_cast<ServerState*>(context);
    }
};

template <typename Srv>
backend::ServiceDeclaration declarationOf(std::string name, bool literal)
{
    backend::ServiceDeclaration declaration;
    declaration.name = std::move(name);
    declaration.literal = literal;
    declaration.typeName = Srv::TYPE_NAME;
    declaration.md5 = Srv::MD5;
    declaration.policy = ServicePolicyOf<Srv>::value;
    return declaration;
}

template <typename Srv>
ServiceServer serve(backend::ServiceDeclaration declaration, std::function<void(Request<Srv>&)> handler)
{
    auto* state = new ServerState<Srv>{std::move(handler), declaration.describe("servers"), {}};
    return ServiceServer(backend::createServer(
        declaration, &ServerState<Srv>::serve, state, &ServerState<Srv>::release
    ));
}

}  // namespace detail

}  // namespace hypha
