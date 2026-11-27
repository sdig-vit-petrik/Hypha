#pragma once

/// @file
/// Latest: an input that holds the newest value of its topic.

#include "hypha/backend/cycle.h"
#include "hypha/backend/session.h"
#include "hypha/cycle_input.h"

#include <cstdint>
#include <expected>
#include <optional>
#include <stdexcept>
#include <string>
#include <utility>

namespace hypha {

/// @brief An input that holds the newest value of its topic.
///
/// `*port` and `port->field` read the value while it is usable in this step, and throw
/// InputNotUsable when the input is stale or has no sample yet; lastKnown() reads the
/// newest value that was ever usable. setIgnored(true) does not make a stale value readable.
///
/// `Latest<View<M>>` reads the value in place (hypha/view.h): `*port` is a View<M> of the
/// bytes the step's input holds, valid in the step. sample() gives its ViewPtr; one kept
/// past the step owns a copy of its bytes from the step's end, which is logged.
template <typename M>
class Latest final : public Input<M>
{
    using Base = Input<M>;
    using Sample = typename detail::SampleOf<M>::type;

  public:
    /// @brief A port that reads `[ports.inputs.<name>]`: its topic and its limit keys.
    template <LatestOption... Options>
    Latest(Cycle& owner, std::string name, Options... options)
        : Latest(owner, std::move(name), detail::settingsOf<detail::InputSettings>(options...))
    {
    }

    /// @brief A port on a topic that code fixes. It has no table, so no limit keys.
    template <LatestOption... Options>
        requires(detail::FixedTopicOption<Options> && ...)
    Latest(Cycle& owner, Topic topic, Options... options)
        : Base(
              owner,
              detail::TopicSource::ofLiteral(std::move(topic)),
              std::string{},
              detail::settingsOf<detail::InputSettings>(options...),
              false
          )
    {
    }

    /// @brief A port on a topic that code fixes, with its limit keys in `[ports.inputs.<name>]`.
    template <LatestOption... Options>
        requires(detail::FixedTopicOption<Options> && ...)
    Latest(Cycle& owner, std::string name, Topic topic, Options... options)
        : Base(
              owner,
              detail::TopicSource::ofLiteral(std::move(topic)),
              std::move(name),
              detail::settingsOf<detail::InputSettings>(options...),
              false
          )
    {
    }

    /// @brief A port that reads the topic of another port (sharing()), with its limit keys
    ///        in `[ports.inputs.<name>]`.
    template <LatestOption... Options>
        requires(detail::SharingOption<Options> && ...)
    Latest(Cycle& owner, std::string name, SharedWith<M> shared, Options... options)
        : Base(
              owner,
              std::move(name),
              std::move(shared),
              detail::settingsOf<detail::InputSettings>(options...),
              false
          )
    {
    }

    /// @brief A port that reads the topic of another port (sharing()). It has no table,
    ///        so no limit keys.
    template <LatestOption... Options>
        requires(detail::SharingOption<Options> && ...)
    Latest(Cycle& owner, SharedWith<M> shared, Options... options)
        : Base(
              owner,
              std::string{},
              std::move(shared),
              detail::settingsOf<detail::InputSettings>(options...),
              false
          )
    {
    }

    /// @brief True once a usable value was captured.
    bool has() const
    {
        return this->hasHeld_;
    }

    /// @brief The value, usable in this step. @throws InputNotUsable unless usable().
    const M& operator*() const
    {
        if (!this->usable()) {
            this->throwNotUsable();
        }
        return this->heldValue();
    }

    /// @brief The ViewPtr of the value of a Latest<View<M>>, usable in this step: to pass
    ///        to code that takes a `ConstPtr`. @throws InputNotUsable unless usable().
    const Sample& sample() const
        requires(detail::IsView<M>::value)
    {
        if (!this->usable()) {
            this->throwNotUsable();
        }
        static_cast<void>(this->heldValue());
        return this->held_;
    }

    /// @throws InputNotUsable unless usable().
    const M* operator->() const
    {
        return &**this;
    }

    /// @brief The newest value that was ever usable, also when the input is stale now.
    /// @throws std::logic_error unless has().
    const M& lastKnown() const
    {
        if (!this->hasHeld_) {
            throw std::logic_error("hypha: the input has no value yet");
        }
        return this->heldValue();
    }

    /// @brief The value when usable() in this step, else @p fallback. @p fallback is an
    ///        object that outlives the use of the result, never a temporary.
    const M& valueOr(const M& fallback) const
    {
        return this->usable() ? this->heldValue() : fallback;
    }

    const M& valueOr(M&&) const = delete;

    /// @brief True when the value is new in this step. It implies has().
    bool fresh() const
    {
        return this->fresh_;
    }

    /// @brief When the held value was sent, on the host monotonic clock; no value unless has().
    std::optional<Time> sentAt() const
    {
        if (!this->hasHeld_) {
            return std::nullopt;
        }
        return Time::fromNanos(static_cast<int64_t>(this->heldStamp_.sentNanos));
    }

    /// @brief The send sequence its publisher gave the held value; no value unless has().
    ///        Each publisher counts its own, and it wraps.
    std::optional<uint64_t> sequence() const
    {
        if (!this->hasHeld_) {
            return std::nullopt;
        }
        return this->heldStamp_.publisherSequence;
    }

    /// @brief The publisher instance that sent the held value; no value unless has(). It
    ///        changes when a publisher restarts.
    std::optional<uint64_t> publisherId() const
    {
        if (!this->hasHeld_) {
            return std::nullopt;
        }
        return this->heldStamp_.publisherId;
    }

  private:
    friend class testing::StepHarness;

    /// The held value; a view of one is read only in the step that captured it.
    const M& heldValue() const
    {
        if constexpr (detail::IsView<M>::value) {
            if (!this->held_) {
                throw std::logic_error("hypha: a view of an input is read in its step only");
            }
        }
        return detail::valueOf(this->held_);
    }

    /// The step's view gives up its bytes; one the step kept gets a copy of them.
    void leaveStep() override
    {
        Base::leaveStep();
        if (detail::leaveSample(this->held_)) {
            this->viewCopies_.note(
                "a view of " + this->portText() + " outlived its step; its bytes were copied"
            );
        }
    }

    [[noreturn]] void throwNotUsable() const
    {
        const std::string&                       table = this->supervision_.name();
        const std::string                        port = table.empty() ? "source" : table;
        const std::expected<std::string, Status> topic = this->slot_->topic.resolve();
        const std::string topicText = topic ? *topic : this->slot_->topic.describe();
        const std::string subject = "hypha: input '" + port + "' (" + topicText + ")";
        const std::string ageMs = std::to_string(this->ageNanos_ / 1'000'000U);
        const bool        stale = this->verdict_ == InputVerdict::Stale;
        if (stale) {
            const std::string limitMs =
                std::to_string(this->supervision_.silenceNanos() / 1'000'000);
            throw InputNotUsable(
                subject + " is stale: its newest sample is " + ageMs + " ms old, silence_ms = "
                    + limitMs + "; read lastKnown() to use the old value",
                InputVerdict::Stale
            );
        }
        throw InputNotUsable(
            subject + " never arrived: no sample for " + ageMs + " ms", InputVerdict::NeverArrived
        );
    }

    Latest(Cycle& owner, std::string name, detail::InputSettings settings)
        : Base(owner, Base::portTopic(name, settings), name, settings, false)
    {
    }

    ::HyphaStatus bind(HyphaCycle* cycle) override
    {
        return this->bindSource(cycle, false);
    }

    ::HyphaStatus capture(const HyphaCycleImage* image) override
    {
        HyphaCycleBindingView view{};
        const ::HyphaStatus   status = backend::cycleImageBinding(image, this->index_, &view);
        if (status != HYPHA_STATUS_OK) {
            return status;
        }
        ::HyphaStatus held = this->captureHeld(image, view.held);
        // Reject undecodable held samples; each removes one, so the queue depth bounds the loop.
        const uint32_t rejectionsMax = this->depth_ + 1;
        for (uint32_t rejection = 0; rejection < rejectionsMax; ++rejection) {
            const bool undecodable = held == HYPHA_STATUS_INVALID_FRAME;
            if (!undecodable) {
                break;
            }
            const uint64_t      sequence = view.held.held_sequence;
            const ::HyphaStatus rejected =
                backend::cycleImageRejectSample(image, this->index_, sequence, &view);
            if (rejected != HYPHA_STATUS_OK) {
                return rejected;
            }
            held = this->captureHeld(image, view.held);
        }
        if (held != HYPHA_STATUS_OK) {
            return held;
        }
        const HyphaCycleBindingReport&                   report = view.health.capture;
        const std::expected<InputVerdict, ::HyphaStatus> mapped =
            detail::verdictFromRaw(report.verdict);
        if (!mapped.has_value()) {
            return mapped.error();
        }
        this->verdict_ = *mapped;
        this->ageNanos_ = view.age_nanos;
        // Invariant of a step: usable() implies has(). A usable verdict without a value
        // breaks it, so the capture fails the run before the step reads the port.
        const bool missingUsableSample = this->usable() && !this->hasHeld_;
        if (missingUsableSample) {
            return HYPHA_STATUS_INVALID_STATE;
        }
        const std::optional<uint64_t> alive =
            this->hasHeld_ ? this->aliveOf(detail::valueOf(this->held_)) : std::nullopt;
        return this->captureSupervised(image, alive);
    }

    ::HyphaStatus captureHeld(const HyphaCycleImage* image, const HyphaCycleHeldReport& report)
    {
        this->heldReport_ = report;
        this->fresh_ = false;
        this->hasHeld_ = report.has_held != 0;
        const bool sameHeld = this->decodedHeld_ && report.held_sequence == this->heldSequence_;
        // A view is lent again each step: the library may move the bytes between steps.
        const bool needDecode = this->hasHeld_ && (!sameHeld || detail::IsView<M>::value);
        if (!needDecode) {
            return HYPHA_STATUS_OK;
        }
        HyphaCycleSampleView view{};
        ::HyphaStatus        status = backend::cycleImageHeld(image, this->index_, &view);
        if (status != HYPHA_STATUS_OK) {
            return status;
        }
        // Decode beside the held value: a sample that does not decode leaves it whole.
        status = detail::decodeView(view, this->candidate_);
        if (status != HYPHA_STATUS_OK) {
            return status;
        }
        std::swap(this->held_, this->candidate_);
        this->heldStamp_ = detail::stampOf(view);
        this->heldSequence_ = report.held_sequence;
        this->decodedHeld_ = true;
        this->fresh_ = !sameHeld;
        return HYPHA_STATUS_OK;
    }

    uint64_t               ageNanos_{0};
    Sample                 held_{};
    Sample                 candidate_{};
    bool                   hasHeld_{false};
    bool                   decodedHeld_{false};
    uint64_t               heldSequence_{0};
    bool                   fresh_{false};
    detail::SampleStamp    heldStamp_{};
    detail::RecurringFault viewCopies_;
};

}  // namespace hypha
