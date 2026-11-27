#pragma once

/// @file
/// Services: a request and its answer, typed by a ROS 1 `.srv`. A server's handler gets a
/// Request<Srv> and replies, refuses, or defers it into a Deferred<Srv> that any thread
/// answers later. A call ends in exactly one CallResult<Srv>. Node::advertiseService and
/// Node::serviceClient (hypha/service_client.h) create the endpoints.

#include "hypha/backend/ros1_codec.h"
#include "hypha/backend/service.h"
#include "hypha/hypha_ffi.h"
#include "hypha/status.h"
#include "hypha_base/clock.h"

#include <cstdint>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace hypha {

/// @brief A service that code fixes, such as `hypha::Service{"/uav1/arming"}` or
///        `hypha::Service{"~/set_exposure"}`. A port name instead reads
///        `[ports.servers.<port>]` or `[ports.clients.<port>]`.
struct Service
{
    std::string name;
};

/// @brief What call() did with a request.
enum class CallStatus : uint32_t
{
    /// Sent, or held until a server appears: exactly one outcome follows.
    Sent = HYPHA_CALL_SENT,
    /// The client has its limit of calls in flight, expired calls the server still holds
    /// included. Nothing was sent.
    Busy = HYPHA_CALL_BUSY,
    /// The request is above the service's bound. Nothing was sent.
    TooLarge = HYPHA_CALL_TOO_LARGE,
};

inline const char* toString(CallStatus status)
{
    switch (status) {
        case CallStatus::Sent:
            return "sent";
        case CallStatus::Busy:
            return "busy: the client has its limit of calls in flight";
        case CallStatus::TooLarge:
            return "too large: the request is above the service's bound";
    }
    return "unknown call status";
}

/// @brief What became of a call. Each sent call ends in exactly one.
enum class CallOutcome : uint32_t
{
    Answered = HYPHA_CALL_ANSWERED,
    /// refuse(), a request left unanswered, a response above its bound.
    Refused = HYPHA_CALL_REFUSED,
    /// No live server of the stream until the timeout.
    NoServer = HYPHA_CALL_NO_SERVER,
    /// Nothing was sent.
    NotSent = HYPHA_CALL_NOT_SENT,
    /// The server crashed or left, its handler failed, or its answer did not verify.
    ServerLost = HYPHA_CALL_SERVER_LOST,
    /// No answer within the timeout.
    Timeout = HYPHA_CALL_TIMEOUT,
};

/// @brief The one outcome of a call, as the library decided it.
template <typename Srv>
class CallResult
{
  public:
    using Response = typename Srv::Response;

    CallOutcome outcome() const
    {
        return this->outcome_;
    }

    /// True for an answered call: response() holds the server's answer.
    bool ok() const
    {
        return this->outcome_ == CallOutcome::Answered;
    }

    /// @throws std::logic_error unless ok().
    const Response& response() const
    {
        if (!this->ok()) {
            throw std::logic_error("hypha: CallResult::response() of a call that " + this->text_);
        }
        return this->response_;
    }

    /// True when the server may have run the request: answered, server lost, timeout.
    bool mayHaveRun() const
    {
        return this->mayHaveRun_;
    }

    /// True when a blind retry is safe: refused, no server, not sent.
    bool safeToRetry() const
    {
        return this->safeToRetry_;
    }

    /// What became of the call, as one line: `refused: not armed`,
    /// `timeout after 1 s (policy defaultService)`.
    const std::string& description() const
    {
        return this->text_;
    }

    /// The client's framework time of the call and of its outcome.
    Time sent() const
    {
        return this->sent_;
    }

    Time done() const
    {
        return this->done_;
    }

    /// The result the library delivered.
    static CallResult of(const HyphaCallResult& raw)
    {
        CallResult result;
        result.outcome_ = static_cast<CallOutcome>(raw.outcome);
        result.mayHaveRun_ = raw.may_have_run != 0;
        result.safeToRetry_ = raw.safe_to_retry != 0;
        result.text_.assign(raw.description, raw.description_len);
        result.sent_ = Time::fromNanos(raw.sent_nanos);
        result.done_ = Time::fromNanos(raw.done_nanos);
        const bool decoded = !result.ok()
                          || codec::decode(raw.response, raw.response_len, result.response_);
        if (!decoded) {
            result.outcome_ = CallOutcome::ServerLost;
            result.mayHaveRun_ = true;
            result.safeToRetry_ = false;
            result.text_ = std::string("server lost: invalid response: it does not decode as ")
                         + codec::dataType<Response>();
        }
        return result;
    }

    /// A call that call() refused before anything was sent: NotSent with the reason.
    static CallResult notSent(CallStatus status)
    {
        CallResult result;
        result.outcome_ = CallOutcome::NotSent;
        result.safeToRetry_ = true;
        result.text_ = std::string("not sent: ") + toString(status);
        return result;
    }

  private:
    CallOutcome outcome_{CallOutcome::NotSent};
    bool        mayHaveRun_{false};
    bool        safeToRetry_{true};
    std::string text_;
    Response    response_{};
    Time        sent_{};
    Time        done_{};
};

/// @return CallResult::description().
template <typename Srv>
const std::string& describe(const CallResult<Srv>& result)
{
    return result.description();
}

/// @brief A request answered later, from any thread: Request::defer() fills it.
///
/// Move-only; a default one is empty. reply() or refuse() answer it once, and empty it.
/// Destroyed while it holds a request, it refuses it. Its server destroyed first, its
/// clients read ServerLost and a later reply() returns Disconnected.
template <typename Srv>
class Deferred
{
  public:
    Deferred() = default;

    explicit operator bool() const
    {
        return static_cast<bool>(this->held_);
    }

    /// @return Ok, Disconnected (the client or the server is gone), TooLarge (the client
    ///         is refused with the sizes), or InvalidState for an empty handle.
    Status reply(const typename Srv::Response& response)
    {
        const std::vector<uint8_t> bytes = codec::encode(response);
        return this->answer(HYPHA_ANSWER_REPLY, bytes.data(), bytes.size());
    }

    /// The client reads Refused with @p reason.
    Status refuse(std::string_view reason = {})
    {
        return this->answer(
            HYPHA_ANSWER_REFUSE, reinterpret_cast<const uint8_t*>(reason.data()), reason.size()
        );
    }

  private:
    template <typename>
    friend class Request;

    explicit Deferred(backend::UniqueRequest held) : held_(std::move(held))
    {
    }

    Status answer(uint32_t kind, const uint8_t* bytes, size_t length)
    {
        if (!this->held_) {
            return Status::InvalidState;
        }
        const Status status = hypha_request_answer(this->held_.get(), kind, bytes, length);
        this->held_.reset();
        return status;
    }

    backend::UniqueRequest held_;
};

/// @brief One request, borrowed by the server's handler for its call.
///
/// The handler replies, refuses, or defers it. Left without any of the three, the
/// library refuses it when the handler returns.
template <typename Srv>
class Request
{
  public:
    using Message = typename Srv::Request;

    Request(HyphaRequest* raw, Message message, std::string clientNode)
        : raw_(raw), message_(std::move(message)), clientNode_(std::move(clientNode))
    {
    }

    Request(const Request&) = delete;
    Request& operator=(const Request&) = delete;

    const Message& operator*() const
    {
        return this->message_;
    }

    const Message* operator->() const
    {
        return &this->message_;
    }

    /// The node that called; empty when the library cannot read it.
    const std::string& clientNode() const
    {
        return this->clientNode_;
    }

    /// @return Ok, Disconnected, TooLarge, or InvalidState when answered or deferred.
    Status reply(const typename Srv::Response& response)
    {
        const std::vector<uint8_t> bytes = codec::encode(response);
        return hypha_request_answer(this->raw_, HYPHA_ANSWER_REPLY, bytes.data(), bytes.size());
    }

    /// The client reads Refused with @p reason, as a ROS 1 callback that returns false.
    Status refuse(std::string_view reason = {})
    {
        return hypha_request_answer(
            this->raw_,
            HYPHA_ANSWER_REFUSE,
            reinterpret_cast<const uint8_t*>(reason.data()),
            reason.size()
        );
    }

    /// Take the request over, to answer it later from any thread.
    /// @throws std::logic_error when it was answered or deferred already.
    [[nodiscard]] Deferred<Srv> defer()
    {
        backend::UniqueRequest held;
        if (hypha_request_defer(this->raw_, held.put()) != HYPHA_STATUS_OK) {
            throw std::logic_error("hypha: Request::defer() of a request answered or deferred");
        }
        return Deferred<Srv>(std::move(held));
    }

  private:
    HyphaRequest* raw_;
    Message       message_;
    std::string   clientNode_;
};

}  // namespace hypha
