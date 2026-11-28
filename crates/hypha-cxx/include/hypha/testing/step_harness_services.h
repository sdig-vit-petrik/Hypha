#pragma once

/// @file
/// The service ports in a testing::StepHarness: a test is the client of each Requests port
/// and the server of each Calls port, with no node and no bus.
///
///     auto takeoff = harness.request(node.takeoff, {});      // sent just before the step
///     harness.step(node, {.now = t});
///     ASSERT_TRUE(takeoff.deferred()) << takeoff.describe();
///     ASSERT_EQ(harness.calls(node.arming).size(), 1U);
///     harness.answer(node.arming, {.success = true});
///     harness.step(node, {.now = t + period});
///     EXPECT_TRUE(takeoff.response().success);
///
/// The library runs a harness step's services as a started cycle's on simulated time, so
/// the rules are the same: a step at T reads the requests sent before T, in the order of
/// send time, client and sequence, at most `per_step`, and only when it calls requests();
/// answers and calls leave after the step; a request the step read and left alone is
/// refused; a step that throws answers what it read as failed (the client reads
/// ServerLost) and its calls end NotSent, read by the next step with the outcomes it did
/// not read; a successful step drops and counts what it did not read. A call the test
/// leaves open past its timeout ends Timeout. The tables (`per_step`, `outstanding`,
/// `timeout_ms`) are read from the configuration a Node or a ConfigContext holds, refused
/// as a start refuses them; a port without a table keeps the defaults. Included by
/// hypha/testing.h.

#include "hypha/backend/service.h"
#include "hypha/cycle_port.h"
#include "hypha/cycle_service.h"

#include <cstdint>
#include <exception>
#include <optional>
#include <stdexcept>
#include <string>
#include <type_traits>
#include <utility>
#include <vector>

namespace hypha::testing {

/// @brief Who sent a request to a StepHarness, and when: by default the client node
///        `test`, just before the next step.
struct From
{
    std::string         client{"test"};
    std::optional<Time> sentAt;
};

/// @brief What became of a request a test delivered.
enum class RequestState {
    Queued,  ///< no step read it yet
    Deferred,  ///< a step read and deferred it; it has no answer yet
    Replied,  ///< answered with a response
    Refused,  ///< refuse(), left unanswered by its step, or a response above its bound
    Failed,  ///< the step that held it failed: its client reads ServerLost
};

/// @brief The answer to one request of a StepHarness, read when asked. It refers to the
///        harness, so it must not outlive it.
template <typename Srv>
class RequestProbe
{
  public:
    RequestState state() const
    {
        return this->read().state;
    }

    bool queued() const
    {
        return this->state() == RequestState::Queued;
    }

    bool deferred() const
    {
        return this->state() == RequestState::Deferred;
    }

    bool replied() const
    {
        return this->state() == RequestState::Replied;
    }

    bool refused() const
    {
        return this->state() == RequestState::Refused;
    }

    bool failed() const
    {
        return this->state() == RequestState::Failed;
    }

    /// @throws std::logic_error unless replied().
    typename Srv::Response response() const
    {
        const Answer           answer = this->read();
        typename Srv::Response response{};
        const bool             decoded =
            answer.state == RequestState::Replied
            && codec::decode(
                reinterpret_cast<const uint8_t*>(answer.bytes.data()), answer.bytes.size(), response
            );
        if (!decoded) {
            throw std::logic_error("hypha: response() of a request that is " + describe(answer));
        }
        return response;
    }

    /// The reason of a refused or failed request; empty otherwise.
    std::string reason() const
    {
        const Answer answer = this->read();
        const bool   replied = answer.state == RequestState::Replied;
        return replied ? std::string{} : answer.bytes;
    }

    /// The framework time the answer carries: that of the step it left after.
    Time answeredAt() const
    {
        return this->read().at;
    }

    /// `queued`, `deferred`, `replied`, `refused: <why>` or `failed: <why>`.
    std::string describe() const
    {
        return describe(this->read());
    }

  private:
    friend class hypha::detail::HarnessServices;

    struct Answer
    {
        RequestState state{RequestState::Queued};
        std::string  bytes;
        Time         at{};
    };

    RequestProbe(const HyphaStepServices* services, uint32_t server, uint64_t id)
        : services_(services), server_(server), id_(id)
    {
    }

    static std::string describe(const Answer& answer)
    {
        switch (answer.state) {
            case RequestState::Queued:
                return "queued";
            case RequestState::Deferred:
                return "deferred";
            case RequestState::Replied:
                return "replied";
            case RequestState::Refused:
                return "refused: " + answer.bytes;
            case RequestState::Failed:
                return "failed: " + answer.bytes;
        }
        return "unknown";
    }

    static void keep(const HyphaStepRequestState* raw, void* context)
    {
        auto* answer = static_cast<Answer*>(context);
        answer->state = static_cast<RequestState>(raw->state);
        answer->bytes.assign(reinterpret_cast<const char*>(raw->bytes), raw->bytes_len);
        answer->at = Time::fromNanos(raw->answered_nanos);
    }

    Answer read() const
    {
        Answer       answer;
        const Status status =
            hypha_step_services_request(this->services_, this->server_, this->id_, &keep, &answer);
        hypha::require(status, "StepHarness request probe");
        return answer;
    }

    const HyphaStepServices* services_;
    uint32_t                 server_;
    uint64_t                 id_;
};

}  // namespace hypha::testing

namespace hypha::detail {

/// @brief The service methods of testing::StepHarness, which derives from it.
class HarnessServices
{
  public:
    /// @brief Deliver @p request to the next step of @p port that reads it, from
    ///        @p from.client, sent at @p from.sentAt (by default just before that step).
    template <typename Srv>
    testing::RequestProbe<Srv>
    request(Requests<Srv>& port, const typename Srv::Request& request, testing::From from = {})
    {
        const std::vector<uint8_t> bytes = codec::encode(request);
        uint64_t                   id = 0;
        const Status               status = hypha_step_services_deliver(
            this->services_.get(),
            HarnessServices::indexOf(port),
            bytes.data(),
            bytes.size(),
            from.client.data(),
            from.client.size(),
            from.sentAt.value_or(Time{}).nanos(),
            from.sentAt.has_value() ? 1 : 0,
            &id
        );
        hypha::require(status, "StepHarness::request()");
        return testing::RequestProbe<Srv>(
            this->services_.get(), HarnessServices::indexOf(port), id
        );
    }

    /// @brief The calls the last step sent on @p port, in call order; a step that failed
    ///        sent none.
    template <typename Srv>
    std::vector<typename Srv::Request> calls(const Calls<Srv>& port) const
    {
        Sent<Srv>    sent;
        const Status status = hypha_step_services_calls(
            this->services_.get(), HarnessServices::indexOf(port), &decodeCall<Srv>, &sent
        );
        hypha::require(status, "StepHarness::calls()");
        if (sent.undecoded != 0) {
            throw std::logic_error("hypha: a call of '" + port.name() + "' does not decode");
        }
        return std::move(sent.calls);
    }

    /// @brief The oldest call in flight on @p port is answered with @p response; the next
    ///        step reads it, or with @p at the first step after @p at.
    /// @throws std::logic_error when the port has no call in flight.
    template <typename Srv>
    void
    answer(Calls<Srv>& port, const typename Srv::Response& response, std::optional<Time> at = {})
    {
        this->feed(port, CallOutcome::Answered, codec::encode(response), at);
    }

    /// @brief The oldest call in flight on @p port ends in @p outcome (Refused, NoServer,
    ///        NotSent, ServerLost or Timeout) with @p reason, as answer() delivers it.
    /// @throws std::logic_error when the port has no call in flight, or for Answered.
    template <typename Srv>
    void outcome(
        Calls<Srv>&         port,
        CallOutcome         outcome,
        std::string_view    reason = {},
        std::optional<Time> at = {}
    )
    {
        if (outcome == CallOutcome::Answered) {
            throw std::logic_error("hypha: outcome(Answered) has no response; use answer()");
        }
        const auto* first = reinterpret_cast<const uint8_t*>(reason.data());
        this->feed(port, outcome, std::vector<uint8_t>(first, first + reason.size()), at);
    }

  protected:
    HarnessServices() = default;

    /// Bind the service ports of the cycle @p name among @p ports.
    void bindServices(const std::vector<Port*>& ports, const std::string& name)
    {
        HyphaStepServices* created = nullptr;
        const Status       status =
            hypha_step_services_create(backend::configHandle(), name.data(), name.size(), &created);
        hypha::require(status, "StepHarness services");
        this->services_ = backend::UniqueStepServices(created);
        for (Port* port : ports) {
            if (port->bindStepServices(created)) {
                this->ports_.push_back(port);
            }
        }
    }

    /// Run @p body as a step at @p now, its service ports on the step's emission.
    template <typename Body>
    void runServices(Time now, Body&& body)
    {
        if (this->ports_.empty()) {
            body();
            return;
        }
        using Step = Run<std::remove_reference_t<Body>>;
        Step         run{this, &body, nullptr};
        const Status status =
            hypha_step_services_step(this->services_.get(), now.nanos(), &Step::step, &run);
        hypha::require(status, "StepHarness step");
        if (run.error) {
            std::rethrow_exception(run.error);
        }
    }

  private:
    template <typename Body>
    struct Run
    {
        HarnessServices*   owner;
        Body*              body;
        std::exception_ptr error;

        static uint8_t step(HyphaCycleEmission* emission, void* context)
        {
            auto* run = static_cast<Run*>(context);
            for (Port* port : run->owner->ports_) {
                port->setEmission(emission);
            }
            uint8_t succeeded = 1;
            try {
                (*run->body)();
            } catch (...) {
                run->error = std::current_exception();
                succeeded = 0;
            }
            for (Port* port : run->owner->ports_) {
                port->setEmission(nullptr);
            }
            return succeeded;
        }
    };

    template <typename ServicePortOf>
    static uint32_t indexOf(const ServicePortOf& port)
    {
        return static_cast<const detail::ServicePort&>(port).index_;
    }

    template <typename Srv>
    struct Sent
    {
        std::vector<typename Srv::Request> calls;
        size_t                             undecoded{0};
    };

    template <typename Srv>
    static void decodeCall(const uint8_t* request, size_t len, int64_t, void* context)
    {
        auto*                 sent = static_cast<Sent<Srv>*>(context);
        typename Srv::Request decoded{};
        if (!codec::decode(request, len, decoded)) {
            ++sent->undecoded;
            return;
        }
        sent->calls.push_back(std::move(decoded));
    }

    template <typename Srv>
    void feed(
        Calls<Srv>&                 port,
        CallOutcome                 outcome,
        const std::vector<uint8_t>& bytes,
        std::optional<Time>         at
    )
    {
        const Status status = hypha_step_services_feed(
            this->services_.get(),
            HarnessServices::indexOf(port),
            static_cast<uint32_t>(outcome),
            bytes.data(),
            bytes.size(),
            at.value_or(Time{}).nanos(),
            at.has_value() ? 1 : 0
        );
        if (status == Status::InvalidState) {
            throw std::logic_error(
                "hypha: client port '" + port.name()
                + "' has no call in flight: a step's call() is sent after the step "
                  "(StepHarness::calls())"
            );
        }
        hypha::require(status, "StepHarness outcome");
    }

    backend::UniqueStepServices services_;
    std::vector<Port*>          ports_;
};

}  // namespace hypha::detail
