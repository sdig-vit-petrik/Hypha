#pragma once

/// @file
/// Output: a port that registers a bounded number of messages per step.

#include "hypha/backend/cycle.h"
#include "hypha/backend/session.h"
#include "hypha/backend/typed.h"
#include "hypha/cycle.h"
#include "hypha/cycle_port.h"
#include "hypha/cycle_runtime.h"
#include "hypha/options.h"

#include <cstdint>
#include <expected>
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

    /// @brief True when the current (or last) step sent on this output.
    bool sent() const
    {
        return this->sent_;
    }

    /// @brief True when start() bound a publisher.
    bool enabled() const
    {
        return this->enabled_;
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
        return this->enabled_ && this->subscriberCount().value_or(1U) > 0U;
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
        void* store = harness.storeFor(*this, &Output::newSent, &Output::deleteSent);
        static_cast<std::vector<M>*>(store)->push_back(message);
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

    ::HyphaStatus capture(const HyphaCycleImage*) override
    {
        return HYPHA_STATUS_OK;
    }

    void endStep() override
    {
        this->sent_ = false;
        this->messagesSentThisStep_ = 0;
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

    detail::TopicSource      topic_;
    detail::EndpointSettings settings_;
    uint32_t                 index_{0};
    HyphaPublisher*          endpoint_{nullptr};
    bool                     enabled_{false};
    bool                     tableRefused_{false};
    bool                     sent_{false};
    uint64_t                 sentCount_{0};
    uint32_t                 seq_{0};
    uint32_t                 messagesSentThisStep_{0};
};

}  // namespace hypha
