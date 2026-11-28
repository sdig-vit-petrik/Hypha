#pragma once

/// @file
/// Requests<Srv> and Calls<Srv>: a server and a client in a cycle. A step reads its requests
/// as an input and answers them; the answers leave after the step. A step makes calls as an
/// output, sent after the step; their outcomes are an input of a later step. Under replay
/// both follow the cut of every input, so a replay with services repeats exactly
/// (docs/design/native-services.md §2-§4).

#include "hypha/backend/service.h"
#include "hypha/cycle.h"
#include "hypha/cycle_port.h"
#include "hypha/service.h"
#include "hypha/service_client.h"

#include <cstdint>
#include <deque>
#include <optional>
#include <stdexcept>
#include <string>
#include <type_traits>
#include <utility>
#include <vector>

namespace hypha {

namespace detail {

/// @brief What a service port of a cycle shares: its declaration, its index, whether a
///        start bound it, and the refusal of a step outside a step or in a StepHarness.
class ServicePort
{
  protected:
    explicit ServicePort(backend::ServiceDeclaration declaration)
        : declaration_(std::move(declaration))
    {
    }

    /// The port's name, as its table names it, or the service of a literal port.
    const std::string& portName() const
    {
        return this->declaration_.name;
    }

    ::HyphaStatus bindWith(
        HyphaCycle* cycle, ::HyphaStatus (*bindFn)(HyphaCycle*, const HyphaServiceDesc*, uint32_t*)
    )
    {
        this->tableRefused_ = false;
        const HyphaServiceDesc desc = backend::descriptionOf(this->declaration_);
        const ::HyphaStatus    bound = bindFn(cycle, &desc, &this->index_);
        // A port with no table, or a table or policy that does not fit, is configuration.
        this->tableRefused_ =
            bound == HYPHA_STATUS_QOS_MISMATCH
            || (!this->declaration_.literal && bound == HYPHA_STATUS_INVALID_ARGUMENT);
        return bound;
    }

    /// @throws std::logic_error outside a step, or in a testing::StepHarness step.
    void requireStep(HyphaCycleEmission* emission, bool inHarness, const char* what) const
    {
        if (inHarness) {
            throw std::logic_error(
                std::string("hypha: ") + what + " of '" + this->portName()
                + "': a testing::StepHarness does not drive service ports yet"
            );
        }
        if (emission == nullptr) {
            throw std::logic_error(
                std::string("hypha: ") + what + " of '" + this->portName() + "' outside a step"
            );
        }
    }

    backend::ServiceDeclaration declaration_;
    uint32_t                    index_{0};
    bool                        tableRefused_{false};
};

}  // namespace detail

/// @brief A server in a cycle: `[ports.servers.<port>] service = "~/name"`.
///
///     for (hypha::Request<std_srvs::SetBool>& request : this->enable.requests()) {
///         request.reply({.success = true, .message = "enabled"});
///     }
///
/// requests() gives the step its requests, at most `per_step` of the table (default 8):
/// on simulated time those stamped before the step's time, in stamp order. Each is
/// answered by reply(), refuse() or defer(); the answer leaves after the step. A request the
/// step read and left alone is refused after it; a step that fails answers what it read as
/// failed (the client reads ServerLost). A step that never calls requests() reads none:
/// they wait for a later step.
template <typename Srv>
class Requests final
    : public Port
    , private detail::ServicePort
{
  public:
    /// A server whose service `[ports.servers.<port>]` names.
    Requests(Cycle& owner, std::string port)
        : Port(owner), ServicePort(detail::declarationOf<Srv>(std::move(port), false))
    {
    }

    /// A server of a service fixed in code.
    Requests(Cycle& owner, Service service)
        : Port(owner), ServicePort(detail::declarationOf<Srv>(std::move(service.name), true))
    {
    }

    /// @brief The requests of this step; the same ones on a later call in the same step.
    /// @throws std::logic_error outside a step.
    std::deque<Request<Srv>>& requests()
    {
        if (this->read_) {
            return this->requests_;
        }
        this->requireStep(this->liveEmission(), this->harnessStore() != nullptr, "requests()");
        this->read_ = true;
        const ::HyphaStatus status =
            hypha_cycle_requests(this->liveEmission(), this->index_, &Requests::collect, this);
        if (status != HYPHA_STATUS_OK) {
            this->failStep(status, "hypha: requests() of '" + this->portName() + "' failed");
        }
        return this->requests_;
    }

    /// The port's name: its table's, or the service of Requests(owner, Service{...}).
    const std::string& name() const
    {
        return this->portName();
    }

  private:
    static HyphaCallbackResult collect(const HyphaRequestView* view, void* context) noexcept
    {
        auto* port = static_cast<Requests*>(context);
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
            port->requests_.emplace_back(
                view->request,
                std::move(message),
                std::string(view->client_node, view->client_node_len)
            );
        } catch (...) {
            // Not kept: the library refuses it after the step as unanswered.
        }
        return backend::ffi::continueRun();
    }

    ::HyphaStatus bind(HyphaCycle* cycle) override
    {
        return this->bindWith(cycle, &hypha_cycle_bind_server);
    }

    bool bindRefusedByConfiguration() const override
    {
        return this->tableRefused_;
    }

    ::HyphaStatus capture(const HyphaCycleImage*) override
    {
        return HYPHA_STATUS_OK;
    }

    void endStep() override
    {
        this->leaveStep();
    }

    /// A request is the step's: it goes with the step.
    void leaveStep() override
    {
        this->requests_.clear();
        this->read_ = false;
    }

    std::string describeFailure() const override
    {
        return "server port '" + this->portName() + "'";
    }

    std::deque<Request<Srv>> requests_;
    bool                     read_{false};
};

/// @brief A client in a cycle: `[ports.clients.<port>] service = "/uav1/arming"`.
///
///     if (this->arming.call({.data = true}) != hypha::CallStatus::Sent) { ... }
///     // a later step
///     if (auto r = this->arming.result()) { if (!r->ok()) HYPHA_LOG_WARN(...); }
///
/// call() stages a call, sent after the step succeeds and stamped with its time; a step
/// that fails sends nothing and the call ends NotSent. Each sent call ends in exactly one
/// outcome, which one later step reads with result() or results(); an outcome a successful
/// step does not read is dropped and counted (unreadResults()), and a failed step passes it
/// on. The calls in flight are at most `outstanding` of the table (default 1): another
/// call() returns Busy. The timeout is the policy's (1 s),
/// hypha::timeout() in code, or `timeout_ms` of the table, counted in framework time.
template <typename Srv>
class Calls final
    : public Port
    , private detail::ServicePort
{
  public:
    /// A client of the service `[ports.clients.<port>]` names.
    Calls(Cycle& owner, std::string port, Timeout timeout = {})
        : Port(owner), ServicePort(declared(std::move(port), false, timeout))
    {
    }

    /// A client of a service fixed in code.
    Calls(Cycle& owner, Service service, Timeout timeout = {})
        : Port(owner), ServicePort(declared(std::move(service.name), true, timeout))
    {
    }

    /// @brief Stage a call with @p request: Sent (an outcome follows in a later step), or
    ///        Busy or TooLarge (nothing is sent and no outcome follows).
    /// @throws std::logic_error outside a step.
    [[nodiscard]] CallStatus call(const typename Srv::Request& request)
    {
        this->requireStep(this->liveEmission(), this->harnessStore() != nullptr, "call()");
        const std::vector<uint8_t> bytes = codec::encode(request);
        uint32_t                   status = HYPHA_CALL_BUSY;
        const ::HyphaStatus        called = hypha_cycle_call(
            this->liveEmission(), this->index_, bytes.data(), bytes.size(), &status
        );
        if (called != HYPHA_STATUS_OK) {
            this->failStep(called, "hypha: call() of '" + this->portName() + "' failed");
        }
        this->lastStatus_ = static_cast<CallStatus>(status);
        return this->lastStatus_;
    }

    /// A ROS 1 `client.call(srv)` does not port as written.
    template <typename T>
        requires std::is_same_v<std::remove_cvref_t<T>, Srv>
    CallStatus call(T&&)
    {
        static_assert(
            !std::is_same_v<T, T>,
            "call(Srv&) does not exist: pass srv.request, and read the result in a later step "
            "(ros1-migration-guide.md, services)"
        );
        return CallStatus::Busy;
    }

    /// @brief The next outcome this step holds, in call order; empty when none is left.
    /// @throws std::logic_error outside a step.
    std::optional<CallResult<Srv>> result()
    {
        this->requireStep(this->liveEmission(), this->harnessStore() != nullptr, "result()");
        std::optional<CallResult<Srv>> next;
        uint8_t                        found = 0;
        const ::HyphaStatus            status = hypha_cycle_next_result(
            this->liveEmission(), this->index_, &Calls::keep, &next, &found
        );
        if (status != HYPHA_STATUS_OK) {
            this->failStep(status, "hypha: result() of '" + this->portName() + "' failed");
        }
        return next;
    }

    /// @brief Every outcome this step still holds, in call order.
    std::vector<CallResult<Srv>> results()
    {
        std::vector<CallResult<Srv>> all;
        while (std::optional<CallResult<Srv>> next = this->result()) {
            all.push_back(std::move(*next));
        }
        return all;
    }

    /// @brief The outcomes that steps which succeeded did not read and dropped, since the
    ///        start; a failed step drops none, and its next step holds them.
    /// @throws std::logic_error outside a step.
    uint64_t unreadResults()
    {
        this->requireStep(this->liveEmission(), this->harnessStore() != nullptr, "unreadResults()");
        uint64_t            unread = 0;
        const ::HyphaStatus status =
            hypha_cycle_unread_results(this->liveEmission(), this->index_, &unread);
        if (status != HYPHA_STATUS_OK) {
            this->failStep(status, "hypha: unreadResults() of '" + this->portName() + "' failed");
        }
        return unread;
    }

    /// What the last call() returned; Sent before the first.
    CallStatus lastStatus() const
    {
        return this->lastStatus_;
    }

    /// The port's name: its table's, or the service of Calls(owner, Service{...}).
    const std::string& name() const
    {
        return this->portName();
    }

  private:
    static backend::ServiceDeclaration declared(std::string name, bool literal, Timeout timeout)
    {
        backend::ServiceDeclaration declaration =
            detail::declarationOf<Srv>(std::move(name), literal);
        declaration.timeout = timeout.value;
        return declaration;
    }

    static HyphaCallbackResult keep(const HyphaCallResult* raw, void* context) noexcept
    {
        auto* next = static_cast<std::optional<CallResult<Srv>>*>(context);
        try {
            next->emplace(CallResult<Srv>::of(*raw));
        } catch (...) {
            // An allocation failure: result() reads nothing more in this step.
        }
        return backend::ffi::continueRun();
    }

    ::HyphaStatus bind(HyphaCycle* cycle) override
    {
        return this->bindWith(cycle, &hypha_cycle_bind_client);
    }

    bool bindRefusedByConfiguration() const override
    {
        return this->tableRefused_;
    }

    ::HyphaStatus capture(const HyphaCycleImage*) override
    {
        return HYPHA_STATUS_OK;
    }

    void endStep() override
    {
    }

    std::string describeFailure() const override
    {
        return "client port '" + this->portName() + "'";
    }

    CallStatus lastStatus_{CallStatus::Sent};
};

}  // namespace hypha
