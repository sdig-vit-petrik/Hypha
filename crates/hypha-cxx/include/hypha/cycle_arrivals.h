#pragma once

/// @file
/// Arrivals: an input that delivers every usable sample of its topic.

#include "hypha/backend/cycle.h"
#include "hypha/backend/session.h"
#include "hypha/cycle_input.h"

#include <compare>
#include <cstddef>
#include <cstdint>
#include <expected>
#include <functional>
#include <optional>
#include <span>
#include <stdexcept>
#include <string>
#include <tuple>
#include <type_traits>
#include <utility>
#include <vector>

namespace hypha {

/// @brief An input that delivers every usable sample of its topic, oldest first.
///
/// A step examines at most perStep() samples (default HYPHA_DEFAULT_ARRIVALS_PER_STEP).
/// A perStep() above the maxDepth of the port's policy is refused, at compile time when
/// it can be, else by std::invalid_argument.
template <typename M>
class Arrivals final : public Input<M>
{
    using Base = Input<M>;

    /// False when the default perStep() exceeds the maxDepth of QosOf<M>.
    template <typename... Options>
    static constexpr bool defaultPerStepFits =
        detail::hasTag<PerStep, Options...> || detail::hasTag<Qos, Options...>
        || HYPHA_DEFAULT_ARRIVALS_PER_STEP <= QosOf<M>::value.maxDepth;

  public:
    /// @brief A port that reads `[ports.inputs.<name>]`: its topic and its limit keys.
    template <ArrivalsOption... Options>
    Arrivals(Cycle& owner, std::string name, Options... options)
        : Arrivals(owner, std::move(name), detail::settingsOf<detail::InputSettings>(options...))
    {
        static_assert(
            defaultPerStepFits<Options...>,
            "hypha: the default perStep of an Arrivals port exceeds the maxDepth of "
            "QosOf<M>::value; give the port a perStep() within it"
        );
    }

    /// @brief A port on a topic that code fixes. It has no table, so no limit keys.
    template <ArrivalsOption... Options>
        requires(detail::FixedTopicOption<Options> && ...)
    Arrivals(Cycle& owner, Topic topic, Options... options)
        : Base(
              owner,
              detail::TopicSource::ofLiteral(std::move(topic)),
              std::string{},
              detail::settingsOf<detail::InputSettings>(options...),
              true
          )
    {
        static_assert(
            defaultPerStepFits<Options...>,
            "hypha: the default perStep of an Arrivals port exceeds the maxDepth of "
            "QosOf<M>::value; give the port a perStep() within it"
        );
    }

    /// @brief A port on a topic that code fixes, with its limit keys in `[ports.inputs.<name>]`.
    template <ArrivalsOption... Options>
        requires(detail::FixedTopicOption<Options> && ...)
    Arrivals(Cycle& owner, std::string name, Topic topic, Options... options)
        : Base(
              owner,
              detail::TopicSource::ofLiteral(std::move(topic)),
              std::move(name),
              detail::settingsOf<detail::InputSettings>(options...),
              true
          )
    {
        static_assert(
            defaultPerStepFits<Options...>,
            "hypha: the default perStep of an Arrivals port exceeds the maxDepth of "
            "QosOf<M>::value; give the port a perStep() within it"
        );
    }

    /// @brief A port that reads the topic of another port (sharing()), with its limit keys
    ///        in `[ports.inputs.<name>]`.
    template <ArrivalsOption... Options>
        requires(detail::SharingOption<Options> && ...)
    Arrivals(Cycle& owner, std::string name, SharedWith<M> shared, Options... options)
        : Base(
              owner,
              std::move(name),
              std::move(shared),
              detail::settingsOf<detail::InputSettings>(options...),
              true
          )
    {
    }

    /// @brief A port that reads the topic of another port (sharing()). It has no table,
    ///        so no limit keys.
    template <ArrivalsOption... Options>
        requires(detail::SharingOption<Options> && ...)
    Arrivals(Cycle& owner, SharedWith<M> shared, Options... options)
        : Base(
              owner,
              std::string{},
              std::move(shared),
              detail::settingsOf<detail::InputSettings>(options...),
              true
          )
    {
    }

    /// @brief The usable messages that arrived for this step, oldest first.
    std::span<const M> samples() const
    {
        return std::span<const M>(this->samples_.data(), this->sampleCount_);
    }

    /// @brief When sample @p index of samples() was sent, on the host monotonic clock.
    /// @throws std::out_of_range when @p index is not below samples().size().
    Time sentAt(size_t index) const
    {
        return Time::fromNanos(static_cast<int64_t>(this->stampAt(index).sentNanos));
    }

    /// @brief The send sequence its publisher gave sample @p index of samples(). Each
    ///        publisher counts its own, and it wraps.
    /// @throws std::out_of_range when @p index is not below samples().size().
    uint64_t sequence(size_t index) const
    {
        return this->stampAt(index).publisherSequence;
    }

    /// @brief The publisher instance that sent sample @p index of samples().
    /// @throws std::out_of_range when @p index is not below samples().size().
    uint64_t publisherId(size_t index) const
    {
        return this->stampAt(index).publisherId;
    }

    /// @brief Cumulative samples the history overwrote before this binding read them.
    ///        It wraps.
    uint64_t lost() const
    {
        return this->lost_;
    }

  private:
    friend class testing::StepHarness;

    Arrivals(Cycle& owner, std::string name, detail::InputSettings settings)
        : Base(owner, Base::portTopic(name, settings), name, settings, true)
    {
    }

    std::optional<uint32_t> triggerIndex() const override
    {
        return this->index_;
    }

    ::HyphaStatus bind(HyphaCycle* cycle) override
    {
        const size_t budget = this->settings_.perStep.value_or(HYPHA_DEFAULT_ARRIVALS_PER_STEP);
        this->samples_.reserve(budget);
        this->decoded_.reserve(budget);
        this->stamps_.reserve(budget);
        this->decodedStamps_.reserve(budget);
        return this->bindSource(cycle, true);
    }

    ::HyphaStatus capture(const HyphaCycleImage* image) override
    {
        HyphaCycleBindingView view{};
        const ::HyphaStatus   status = backend::cycleImageBinding(image, this->index_, &view);
        if (status != HYPHA_STATUS_OK) {
            return status;
        }
        const ::HyphaStatus read = this->captureSamples(image, view);
        if (read != HYPHA_STATUS_OK) {
            return read;
        }
        this->lost_ = view.health.capture.lost_total;
        this->heldReport_ = view.held;
        return this->captureSupervision(image);
    }

    /// A sample that does not decode is reported to the library and left out.
    ::HyphaStatus captureSamples(const HyphaCycleImage* image, HyphaCycleBindingView& view)
    {
        const size_t sampleCount = view.health.capture.sample_count;
        // Decode into the second buffer: a failed capture keeps the previous batch whole.
        if (this->decoded_.size() < sampleCount) {
            this->decoded_.resize(sampleCount);
        }
        if (this->decodedStamps_.size() < sampleCount) {
            this->decodedStamps_.resize(sampleCount);
        }
        size_t kept = 0;
        for (size_t sampleIndex = 0; sampleIndex < sampleCount; ++sampleIndex) {
            HyphaCycleSampleView sample{};
            const ::HyphaStatus  read = detail::readSample(
                image, this->index_, sampleIndex, this->decoded_[kept], sample
            );
            if (read == HYPHA_STATUS_INVALID_FRAME) {
                const ::HyphaStatus rejected =
                    backend::cycleImageRejectSample(image, this->index_, sample.sequence, &view);
                if (rejected != HYPHA_STATUS_OK) {
                    return rejected;
                }
                continue;
            }
            if (read != HYPHA_STATUS_OK) {
                return read;
            }
            this->decodedStamps_[kept] = detail::stampOf(sample);
            ++kept;
        }
        const std::expected<InputVerdict, ::HyphaStatus> mapped =
            detail::verdictFromRaw(view.health.capture.verdict);
        if (!mapped.has_value()) {
            return mapped.error();
        }
        this->samples_.swap(this->decoded_);
        this->stamps_.swap(this->decodedStamps_);
        this->sampleCount_ = kept;
        this->verdict_ = *mapped;
        return HYPHA_STATUS_OK;
    }

    /// Pushes the alive value of the newest sample, or the last one when this step has none.
    ::HyphaStatus captureSupervision(const HyphaCycleImage* image)
    {
        const std::span<const M> batch = this->samples();
        const bool               sawSample = !batch.empty();
        if (sawSample) {
            this->lastAlive_ = this->aliveOf(batch.back());
        }
        return this->captureSupervised(image, this->lastAlive_);
    }

    const detail::SampleStamp& stampAt(size_t index) const
    {
        const bool outOfRange = index >= this->sampleCount_ || index >= this->stamps_.size();
        if (outOfRange) {
            throw std::out_of_range("hypha: no sample " + std::to_string(index) + " in this step");
        }
        return this->stamps_[index];
    }

    std::vector<M>          samples_;
    std::vector<M>          decoded_;
    std::vector<detail::SampleStamp> stamps_;
    std::vector<detail::SampleStamp> decodedStamps_;
    size_t                  sampleCount_{0};
    uint64_t                lost_{0};
    std::optional<uint64_t> lastAlive_;
};

}  // namespace hypha

#include "hypha/cycle_arrivals_merge.h"
