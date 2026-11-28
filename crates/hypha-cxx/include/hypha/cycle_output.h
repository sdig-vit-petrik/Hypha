#pragma once

/// @file
/// Output: a port that registers a bounded number of messages per step.

#include "hypha/backend/cycle.h"
#include "hypha/backend/session.h"
#include "hypha/backend/typed.h"
#include "hypha/cycle.h"
#include "hypha/cycle_port.h"
#include "hypha/cycle_runtime.h"
#include "hypha/loan.h"
#include "hypha/options.h"

#include <cstdint>
#include <expected>
#include <memory>
#include <new>
#include <optional>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace hypha {

/// @brief An output: send() registers a message of the step, which goes out when the
///        step returns.
template <typename M>
class Output final : public Port
{
  public:
    /// @brief An output that reads its topic from `[ports.outputs.<name>]`.
    template <OutputOption... Options>
    Output(Cycle& owner, std::string name, Options... options)
        : Output(owner, std::move(name), detail::settingsOf<detail::EndpointSettings>(options...))
    {
    }

    /// @brief Register @p message for this step. The sent bytes, not @p message, carry
    ///        the next `header.seq`. A step sends at most perStep() messages (default 1).
    /// @throws std::logic_error outside a step. A send past perStep() throws a
    ///         std::runtime_error and fails the step, also when the step catches it.
    void send(const M& message)
    {
        detail::CaptureStore* harness = this->harnessStore();
        if (harness != nullptr) {
            this->keepForHarness(*harness, message);
            return;
        }
        HyphaCycleEmission* emission = this->liveEmission();
        if (emission == nullptr) {
            throw std::logic_error(
                "hypha: output '" + this->topic_.paramName + "': send() outside a step"
            );
        }
        const bool disabledOutputExceeded =
            !this->enabled_ && this->messagesSentThisStep_ >= this->settings_.perStep;
        if (disabledOutputExceeded) {
            this->failStep(
                HYPHA_STATUS_RESOURCE_LIMIT, "hypha: output exceeded its perStep() bound"
            );
        }
        if (this->enabled_) {
            this->stage(emission, message);
        }
        if (!this->enabled_) {
            this->messagesSentThisStep_ += 1;
        }
        this->sent_ = true;
        this->sentCount_ += 1;
    }

    /// @brief Reserve, in this step, a slot for @p head whose loaned field the step writes in
    ///        place (data(), @p capacityBytes), as Publisher<M>::loan() does. send(Loan&&)
    ///        registers it, shrunk, in the order of the step's sends, sent after a successful
    ///        step. It counts against perStep() until sent or dropped; dropped, or kept past
    ///        the step, it returns its slot. Several loans may be held beside send(const M&).
    /// @return The loan, or the refusal, which loses no message and does not fail the step:
    ///         PoolExhausted, LoanLimit, ResourceLimit past perStep(), TooLarge,
    ///         InvalidArgument for a filled loaned field, NotAvailable when disabled.
    /// @throws std::logic_error outside a step.
    [[nodiscard]] std::expected<Loan<M>, Status> loan(M head, size_t capacityBytes)
        requires LoanableMessage<M>
    {
        using Element = typename Loan<M>::Element;
        detail::CaptureStore* harness = this->harnessStore();
        HyphaCycleEmission*   emission = this->liveEmission();
        if (harness == nullptr && emission == nullptr) {
            throw std::logic_error(
                "hypha: output '" + this->topic_.paramName + "': loan() outside a step"
            );
        }
        if (!this->enabled()) {
            return std::unexpected(Status(HYPHA_STATUS_NOT_AVAILABLE));
        }
        const auto& field = head.*message_traits::LoanedField<M>::member;
        const bool  refused = !field.empty() || capacityBytes % sizeof(Element) != 0
                           || capacityBytes / sizeof(Element) > UINT32_MAX;
        if (refused) {
            return std::unexpected(Status(HYPHA_STATUS_INVALID_ARGUMENT));
        }
        detail::GapStream<detail::LoanedFieldType<M>> measured(
            nullptr, static_cast<size_t>(-1), &field, 0, capacityBytes
        );
        ros::serialization::serialize(measured, head);
        if (!measured.complete()) {
            return std::unexpected(Status(HYPHA_STATUS_INVALID_SCHEMA));
        }
        const size_t                      length = measured.length();
        uint8_t*                          bytes = nullptr;
        uint64_t                          token = 0;
        std::shared_ptr<detail::LentLink> link;
        // A StepHarness loan is memory of the loan's own.
        if (harness == nullptr) {
            const ::HyphaStatus lent =
                hypha_cycle_emission_lend(emission, this->index_, length, &bytes, &token);
            if (lent != HYPHA_STATUS_OK) {
                return std::unexpected(Status(lent));
            }
            this->link_->emission = emission;
            link = this->link_;
        }
        return Loan<M>(
            std::move(head),
            bytes,
            length,
            std::move(link),
            token,
            this,
            measured.gapOffset(),
            capacityBytes,
            length - capacityBytes
        );
    }

    /// @brief Register @p loan for this step with all capacity() bytes of its loaned field.
    void send(Loan<M>&& loan)
        requires LoanableMessage<M>
    {
        const size_t capacity = loan.capacity();
        this->send(std::move(loan), capacity);
    }

    /// @brief Register @p loan with the first @p usedBytes of its loaned field: the bytes of
    ///        send(const M&) for the same message, the next `header.seq`. It is consumed.
    /// @throws std::logic_error for a loan of another output or step. A size past capacity()
    ///         or of part of an element, or a head changed in length, fails the step.
    void send(Loan<M>&& loan, size_t usedBytes)
        requires LoanableMessage<M>
    {
        Loan<M>               consumed = std::move(loan);
        detail::CaptureStore* harness = this->harnessStore();
        const bool            ours = consumed.owner_ == this;
        const bool live = harness != nullptr ? consumed.link_ == nullptr : consumed.liveInStep();
        if (!ours || !live) {
            throw std::logic_error(
                "hypha: output '" + this->topic_.paramName
                + "': send() of a loan of another output or of an earlier step"
            );
        }
        const std::expected<size_t, Status> total = consumed.seal(usedBytes, [this] {
            return this->seq_++;
        });
        if (!total.has_value()) {
            this->failStep(
                static_cast<::HyphaStatus>(total.error()),
                "hypha: output '" + this->topic_.paramName
                    + "': a loan sent with a size or a head it was not cut for"
            );
        }
        if (harness != nullptr) {
            M message;
            if (!codec::decode(consumed.bytes_, *total, message)) {
                this->failStep(HYPHA_STATUS_INVALID_SCHEMA, "hypha: a loan did not decode");
            }
            this->keepForHarness(*harness, message);
            return;
        }
        consumed.link_.reset();
        const ::HyphaStatus sent =
            hypha_cycle_emission_send(this->liveEmission(), consumed.token_, *total);
        if (sent != HYPHA_STATUS_OK) {
            this->failStep(sent, "hypha: output send of a loan failed");
        }
        this->sent_ = true;
        this->sentCount_ += 1;
    }

    /// @brief True when the current (or last) step sent on this output.
    bool sent() const
    {
        return this->sent_;
    }

    /// @brief True when start() bound a publisher. In a testing::StepHarness it is true for
    ///        a port that the parameter file wires, as a start would.
    bool enabled() const
    {
        return this->enabled_ || this->enabledByHarness();
    }

    /// @brief The sends accepted across steps, those of a disabled output included. It wraps.
    uint64_t sentCount() const
    {
        return this->sentCount_;
    }

    /// Current connected subscriber count after start(); empty for a disabled output or
    /// when the transport cannot count.
    std::optional<size_t> subscriberCount() const
    {
        const bool live = this->liveCycle() != nullptr && this->enabled_;
        const bool unbound = !live || this->endpoint_ == nullptr;
        if (unbound) {
            return std::nullopt;
        }
        return backend::publisherSubscriberCount(this->endpoint_);
    }

    /// @brief True when the output is enabled and may have a subscriber. Use it to skip
    ///        building a message nobody reads.
    bool hasSubscribers() const
    {
        return this->enabled() && this->subscriberCount().value_or(1U) > 0U;
    }

  private:
    friend class testing::StepHarness;

    static void* newSent()
    {
        return new std::vector<M>();
    }

    static void deleteSent(void* sent)
    {
        delete static_cast<std::vector<M>*>(sent);
    }

    void keepForHarness(detail::CaptureStore& harness, const M& message)
    {
        const bool beyondBound = this->messagesSentThisStep_ >= this->settings_.perStep;
        if (beyondBound) {
            this->failStep(
                HYPHA_STATUS_RESOURCE_LIMIT, "hypha: output exceeded its perStep() bound"
            );
        }
        // A disabled output counts the send and keeps nothing, as in a started cycle.
        if (this->enabled()) {
            void* store = harness.storeFor(*this, &Output::newSent, &Output::deleteSent);
            static_cast<std::vector<M>*>(store)->push_back(message);
        }
        this->messagesSentThisStep_ += 1;
        this->sent_ = true;
        this->sentCount_ += 1;
    }

    Output(Cycle& owner, std::string name, detail::EndpointSettings settings)
        : Port(owner)
        , topic_(detail::TopicSource::ofPort(std::move(name), true, settings.optionalTopic))
        , settings_(settings)
    {
    }

    ::HyphaStatus bind(HyphaCycle* cycle) override
    {
        this->tableRefused_ = false;
        const std::expected<std::string, Status> resolved = this->topic_.resolve();
        if (!resolved) {
            this->tableRefused_ = true;
            return static_cast<::HyphaStatus>(resolved.error());
        }
        if (resolved->empty()) {
            this->enabled_ = false;
            return HYPHA_STATUS_OK;
        }
        const Status topicStatus = validateTopicName(*resolved);
        if (topicStatus != Status::Ok) {
            this->tableRefused_ = true;
            return static_cast<::HyphaStatus>(topicStatus);
        }
        // One loan per message of a step: each is held until the step ends.
        const HyphaPublisherOptions options =
            backend::typedPublisherOptions<M>(this->settings_, this->settings_.perStep);
        backend::PublisherHandle<M> publisher(*resolved, options);
        HyphaPublisher*             endpoint = publisher.transfer();
        if (endpoint == nullptr) {
            return HYPHA_STATUS_INVALID_STATE;
        }
        const ::HyphaStatus bound = backend::cycleBindOutput(
            cycle,
            endpoint,
            this->settings_.diagnostic ? HYPHA_CYCLE_OUTPUT_FAILURE_DROP_POOL_EXHAUSTED
                                       : HYPHA_CYCLE_OUTPUT_FAILURE_FAIL_RUN,
            this->settings_.perStep,
            &this->index_
        );
        if (bound != HYPHA_STATUS_OK) {
            return bound;
        }
        this->endpoint_ = endpoint;
        this->enabled_ = true;
        return HYPHA_STATUS_OK;
    }

    bool bindRefusedByConfiguration() const override
    {
        return this->tableRefused_;
    }

    const detail::TopicSource* topicSource() const override
    {
        return &this->topic_;
    }

    ::HyphaStatus capture(const HyphaCycleImage*) override
    {
        return HYPHA_STATUS_OK;
    }

    void endStep() override
    {
        this->sent_ = false;
        this->messagesSentThisStep_ = 0;
    }

    /// The step's loans are over: one kept past it finds another generation.
    void leaveStep() override
    {
        this->link_->emission = nullptr;
        this->link_->generation += 1;
    }

    std::string describeFailure() const override
    {
        return detail::describePort("output", this->topic_);
    }

    /// @brief Serialize the message into the step's buffer, reserved at its exact length.
    void stage(HyphaCycleEmission* emission, const M& message)
    {
        try {
            const size_t        length = codec::encodedLength(message);
            uint8_t*            payload = nullptr;
            const ::HyphaStatus reserved =
                backend::cycleEmissionReserve(emission, this->index_, length, &payload);
            if (reserved != HYPHA_STATUS_OK) {
                throw std::runtime_error(
                    "hypha: output send failed: " + std::string(Status(reserved).name())
                );
            }
            if (payload == nullptr) {
                // A diagnostic output had no buffer free.
                return;
            }
            if (!codec::detail::encodeWithSeq(message, payload, length, this->seq_++)) {
                this->failStep(
                    HYPHA_STATUS_INVALID_SCHEMA,
                    "hypha: output message did not serialize to its length"
                );
            }
            const ::HyphaStatus committed = backend::cycleEmissionCommit(emission);
            if (committed != HYPHA_STATUS_OK) {
                throw std::runtime_error(
                    "hypha: output commit failed: " + std::string(Status(committed).name())
                );
            }
        } catch (const detail::StepFailure&) {
            throw;
        } catch (const std::bad_alloc&) {
            this->failStep(HYPHA_STATUS_RESOURCE_LIMIT, "hypha: output allocation failed");
        } catch (const std::exception& error) {
            this->failStep(HYPHA_STATUS_PANIC, std::string("hypha: ") + error.what());
        }
    }

    detail::TopicSource               topic_;
    detail::EndpointSettings          settings_;
    uint32_t                          index_{0};
    HyphaPublisher*                   endpoint_{nullptr};
    bool                              enabled_{false};
    bool                              tableRefused_{false};
    bool                              sent_{false};
    uint64_t                          sentCount_{0};
    uint32_t                          seq_{0};
    uint32_t                          messagesSentThisStep_{0};
    std::shared_ptr<detail::LentLink> link_ = std::make_shared<detail::LentLink>();
};

}  // namespace hypha
