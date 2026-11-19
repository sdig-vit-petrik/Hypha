#pragma once

/// @file
/// Services over topics. A request and its result are mrs_bridge/Call samples on
/// <name>/request and <name>/result; the payload is the serialized service Request or
/// Response. Each request carries the caller's identity and sequence number, and a
/// client takes only the result that carries both. An empty result payload means the
/// server's handler refused the call (a ROS callback that returned false).

#include <hypha/backend/ros1_codec.h>

#include <mrs_bridge/Call.h>
#include <mrs_host/plugin_host.h>

#include <atomic>
#include <cstdint>
#include <functional>
#include <map>
#include <optional>
#include <stdexcept>
#include <string>
#include <utility>

namespace mrs {

namespace detail {
template <class M>
std::vector<uint8_t> encodePayload(const M& message)
{
    return hypha::codec::encode(message);
}

template <class M>
bool decodePayload(const std::vector<uint8_t>& bytes, M& out)
{
    return hypha::codec::decode(bytes, out);
}
}  // namespace detail

/// A service type from its request and response types.
template <class Req, class Res>
struct ServicePair
{
    using Request = Req;
    using Response = Res;
};

/// The server of service @p Srv. The handler gets a ticket and the request; it returns
/// the response, or std::nullopt to answer later with respond(ticket, ...), or
/// refuse(ticket).
template <class Srv>
class Serve
{
  public:
    using Request = typename Srv::Request;
    using Response = typename Srv::Response;
    using Handler = std::function<std::optional<Response>(uint64_t ticket, const Request&)>;

    Serve(PluginHost& host, const std::string& name, Handler handler)
        : host_(host)
        , name_(name)
        , requests_(host.arrivals<mrs_bridge::Call>(name + "/request"))
        , results_(host.output<mrs_bridge::Call>(name + "/result"))
        , handler_(std::move(handler))
    {
        this->hook_ = host.addHook([this] {
            this->process();
        });
    }

    Serve(const Serve&) = delete;
    Serve& operator=(const Serve&) = delete;

    ~Serve()
    {
        this->host_.removeHook(this->hook_);
    }

    /// Answer a request whose handler returned std::nullopt. Call inside a step.
    void respond(uint64_t ticket, const Response& response)
    {
        this->answer(ticket, detail::encodePayload(response));
    }

    /// Refuse a request whose handler returned std::nullopt. Call inside a step.
    void refuse(uint64_t ticket)
    {
        this->answer(ticket, {});
    }

    /// The requests that wait for respond() or refuse().
    size_t pending() const
    {
        return this->pending_.size();
    }

  private:
    struct Caller
    {
        std::string caller;
        uint64_t    id;
    };

    void process()
    {
        for (const mrs_bridge::Call& call : this->requests_.samples()) {
            const uint64_t ticket = ++this->nextTicket_;
            this->pending_.emplace(ticket, Caller{call.caller, call.id});
            Request request;
            if (!detail::decodePayload(call.payload, request)) {
                HYPHA_LOG_ERROR(
                    "mrs: service '{}': undecodable request from '{}'", this->name_, call.caller
                );
                this->refuse(ticket);
                continue;
            }
            const std::optional<Response> response = this->handler_(ticket, request);
            if (response.has_value()) {
                this->respond(ticket, *response);
            }
        }
    }

    void answer(uint64_t ticket, std::vector<uint8_t> payload)
    {
        const auto found = this->pending_.find(ticket);
        if (found == this->pending_.end()) {
            throw std::logic_error(
                "mrs: service '" + this->name_ + "': no pending request " + std::to_string(ticket)
            );
        }
        mrs_bridge::Call result;
        result.caller = found->second.caller;
        result.id = found->second.id;
        result.payload = std::move(payload);
        this->results_.send(result);
        this->pending_.erase(found);
    }

    PluginHost&                        host_;
    std::string                        name_;
    hypha::Arrivals<mrs_bridge::Call>& requests_;
    hypha::Output<mrs_bridge::Call>&   results_;
    Handler                            handler_;
    HookId                             hook_{0};
    uint64_t                           nextTicket_{0};
    std::map<uint64_t, Caller>         pending_;
};

enum class CallState { Idle, Pending, Done, Failed, TimedOut };

inline const char* toString(CallState state)
{
    switch (state) {
        case CallState::Idle:
            return "idle";
        case CallState::Pending:
            return "pending";
        case CallState::Done:
            return "done";
        case CallState::Failed:
            return "failed";
        case CallState::TimedOut:
            return "timed out";
    }
    return "unknown";
}

/// The client of service @p Srv. One call at a time; the timeout is in framework time.
template <class Srv>
class Request
{
  public:
    using RequestType = typename Srv::Request;
    using Response = typename Srv::Response;

    Request(PluginHost& host, const std::string& name, hypha::Duration timeout)
        : host_(host)
        , name_(name)
        , timeout_(timeout)
        , requests_(host.output<mrs_bridge::Call>(name + "/request"))
        , results_(host.arrivals<mrs_bridge::Call>(name + "/result"))
    {
        this->hook_ = host.addHook([this] {
            this->process();
        });
    }

    Request(const Request&) = delete;
    Request& operator=(const Request&) = delete;

    ~Request()
    {
        this->host_.removeHook(this->hook_);
    }

    /// Send @p request in this step. False while a call is pending. Call inside a step.
    bool send(const RequestType& request)
    {
        if (this->state_ == CallState::Pending) {
            return false;
        }
        mrs_bridge::Call call;
        call.caller = this->host_.callerId();
        // One sequence per process, so two clients of one service in one host never share an id.
        static std::atomic<uint64_t> sequence{0};
        this->sequence_ = ++sequence;
        call.id = this->sequence_;
        call.payload = detail::encodePayload(request);
        this->requests_.send(call);
        this->deadline_ = this->host_.now() + this->timeout_;
        this->state_ = CallState::Pending;
        return true;
    }

    /// Done, Failed and TimedOut stay until the next send().
    CallState state() const
    {
        return this->state_;
    }

    /// @throws std::logic_error unless state() is Done.
    const Response& response() const
    {
        if (this->state_ != CallState::Done) {
            throw std::logic_error(
                "mrs: service '" + this->name_ + "': no response (" + toString(this->state_) + ")"
            );
        }
        return this->response_;
    }

    const std::string& name() const
    {
        return this->name_;
    }

  private:
    void process()
    {
        for (const mrs_bridge::Call& call : this->results_.samples()) {
            const bool mine = this->state_ == CallState::Pending
                           && call.caller == this->host_.callerId() && call.id == this->sequence_;
            if (!mine) {
                continue;
            }
            if (call.payload.empty()) {
                this->state_ = CallState::Failed;
                continue;
            }
            Response response;
            if (!detail::decodePayload(call.payload, response)) {
                HYPHA_LOG_ERROR("mrs: service '{}': undecodable result", this->name_);
                this->state_ = CallState::Failed;
                continue;
            }
            this->response_ = std::move(response);
            this->state_ = CallState::Done;
        }
        if (this->state_ == CallState::Pending && this->host_.now() >= this->deadline_) {
            this->state_ = CallState::TimedOut;
        }
    }

    PluginHost&                        host_;
    std::string                        name_;
    hypha::Duration                    timeout_;
    hypha::Output<mrs_bridge::Call>&   requests_;
    hypha::Arrivals<mrs_bridge::Call>& results_;
    HookId                             hook_{0};
    uint64_t                           sequence_{0};
    CallState                          state_{CallState::Idle};
    hypha::Time                        deadline_{};
    Response                           response_{};
};

}  // namespace mrs
