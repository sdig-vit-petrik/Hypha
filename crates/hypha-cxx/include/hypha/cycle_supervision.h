#pragma once

/// @file
/// The supervision of one input port: silence and stagnation checks.

#include "hypha/backend/cycle.h"
#include "hypha/backend/session.h"
#include "hypha/config.h"
#include "hypha/cycle.h"
#include "hypha/cycle_port.h"
#include "hypha/options.h"

#include <cstdint>
#include <stdexcept>
#include <string>
#include <utility>

namespace hypha {

namespace detail {

/// @brief The `silence_ms` of the table `ports.inputs.<table>` in nanoseconds, as start()
///        reads it: zero for `silence_ms = 0`, an input with no time limit. For a
///        testing::StepHarness, which starts nothing; a start decides it below the C ABI.
/// @throws std::logic_error without a loaded configuration.
/// @throws ParameterError when the key is missing, or outside 0 and [1 ms, @p limitMax].
inline int64_t tableSilenceNanos(const std::string& table, Duration limitMax)
{
    if (backend::configHandle() == nullptr) {
        throw std::logic_error(
            "hypha: input '" + table
            + "' reads its silence_ms and needs a loaded configuration: a Node or a ConfigContext"
        );
    }
    const ConfigView             ports("ports.inputs." + table);
    const std::optional<int64_t> millis = ports.optional<int64_t>("silence_ms");
    if (!millis.has_value()) {
        throw ParameterError(
            "hypha: parameter '" + ports.name("silence_ms")
            + "' is missing: an input needs a limit, 0 for none, or the tag hypha::unsupervised"
        );
    }
    constexpr int64_t nanosPerMilli = 1'000'000;
    const int64_t     millisMax = limitMax.nanos() / nanosPerMilli;
    const bool        outOfRange = *millis < 0 || *millis > millisMax;
    if (outOfRange) {
        throw ParameterError(
            "hypha: parameter '" + ports.name("silence_ms")
            + "' is outside its range: 0 (no "
              "limit) or [1, "
            + std::to_string(millisMax) + "] ms"
        );
    }
    return *millis * nanosPerMilli;
}

/// @brief What a testing::StepHarness step's clear() leaves of an input's @p facts: no
///        sample, NeverArrived, and not ok unless the input does not count for health.
inline void forgetJudgement(HyphaCycleBindingView& facts, bool countsForHealth)
{
    facts.health.capture.verdict = HYPHA_CYCLE_VERDICT_NEVER_ARRIVED;
    facts.supervision.faults &= ~HYPHA_FAULT_STALE;
    facts.ok = countsForHealth ? 0 : 1;
}

/// @brief The supervision of one input port. Latest and Arrivals hold one by composition.
class InputSupervision
{
  public:
    /// @p name is the port name, empty for none. @p readsSource marks a port whose table
    /// has no topic key.
    InputSupervision(std::string name, bool readsSource)
        : name_(std::move(name)), readsSource_(readsSource)
    {
    }

    /// @brief The image of the current step, or nullptr outside the step.
    const HyphaCycleImage* image() const
    {
        return this->image_;
    }

    bool named() const
    {
        return !this->name_.empty();
    }

    /// @brief The port's table name; empty for a port without one.
    const std::string& name() const
    {
        return this->name_;
    }

    /// @brief The silence limit that attached at start(), in nanoseconds; zero without one.
    int64_t silenceNanos() const
    {
        return this->silenceNanos_;
    }

    bool bound() const
    {
        return this->bound_;
    }

    /// @brief True once start() bound the port: untimed() is then the library's answer.
    bool decided() const
    {
        return this->decided_;
    }

    /// @brief The port runs without a silence limit, as start() decided it.
    bool untimed() const
    {
        return this->untimed_;
    }

    /// @brief The view of this binding: of this step in step(), else of the last finished
    ///        step of @p cycle. InvalidState before start() and after the cycle stops.
    ::HyphaStatus view(const HyphaCycle* cycle, HyphaCycleBindingView& out) const
    {
        if (this->image_ != nullptr) {
            return backend::cycleImageBinding(this->image_, this->index_, &out);
        }
        if (cycle != nullptr) {
            return backend::cycleBinding(cycle, this->index_, &out);
        }
        return HYPHA_STATUS_INVALID_STATE;
    }

    /// @brief Check, read and attach the limits of input @p index in one library call.
    ::HyphaStatus bind(HyphaCycle* cycle, uint32_t index, const InputSettings& settings, bool alive)
    {
        HyphaNode* node = backend::nodeHandle();
        if (node == nullptr) {
            return HYPHA_STATUS_INVALID_STATE;
        }
        const int64_t                    minNanos = Duration::fromMillis(1).nanos();
        const int64_t                    maxNanos = settings.limitMax.nanos();
        const HyphaSupervisionRanges     ranges{minNanos, maxNanos, minNanos, maxNanos};
        const uint8_t                    unsupervised = settings.unsupervised ? 1 : 0;
        const uint8_t                    stagnationFlag = alive ? 1 : 0;
        const uint8_t                    takesTopic = this->readsSource_ ? 0 : 1;
        backend::InputSupervisionRequest request;
        request.cycle = cycle;
        request.index = index;
        request.node = node;
        request.name = this->name_;
        request.unsupervised = unsupervised;
        request.checkStagnation = stagnationFlag;
        request.takesTopic = takesTopic;
        request.ranges = ranges;
        const backend::InputSupervisionResult result = backend::bindInputSupervision(request);
        this->detail_ = result.detail;
        if (result.status != HYPHA_STATUS_OK) {
            return result.status;
        }
        this->bound_ = result.attached;
        this->silenceNanos_ = result.silenceNanos;
        this->untimed_ = result.silenceNanos == 0;
        this->decided_ = true;
        this->index_ = index;
        return HYPHA_STATUS_OK;
    }

    /// @brief Keep @p image for this step, so view() and setIgnored() reach it.
    ::HyphaStatus capture(const HyphaCycleImage* image, uint32_t index)
    {
        this->image_ = image;
        this->index_ = index;
        if (!this->bound_) {
            return HYPHA_STATUS_OK;
        }
        HyphaCycleBindingView view{};
        return backend::cycleImageBinding(image, index, &view);
    }

    /// @brief The image is valid only inside the step. Called after the step.
    void endStep()
    {
        this->image_ = nullptr;
    }

    /// @brief Set the ignore flag of input @p index. @throws std::logic_error outside step().
    void setIgnored(uint32_t index, bool ignore)
    {
        const bool outsideStep = this->image_ == nullptr;
        if (outsideStep) {
            throw std::logic_error("hypha: setIgnored() outside step()");
        }
        const bool unsupervised = !this->bound_;
        if (unsupervised) {
            throw std::logic_error("hypha: setIgnored() needs a supervised port");
        }
        const ::HyphaStatus status = backend::cycleImageSetIgnored(
            this->image_, index, static_cast<uint8_t>(ignore ? 1 : 0)
        );
        if (status != HYPHA_STATUS_OK) {
            throw std::runtime_error(
                "hypha: setIgnored() failed with status " + std::to_string(static_cast<int>(status))
            );
        }
    }

    /// @brief Clear input @p index when the step returns. @throws std::logic_error outside
    ///        step() or for a trigger port, std::runtime_error when the library refuses.
    void clear(uint32_t index) const
    {
        if (this->image_ == nullptr) {
            throw std::logic_error("hypha: clear() outside step()");
        }
        const ::HyphaStatus status =
            backend::cycleImageClear(this->image_, HYPHA_CYCLE_CLEAR_INPUT, index);
        if (status == HYPHA_STATUS_INVALID_ARGUMENT) {
            throw std::logic_error(
                "hypha: clear() on a trigger port: its pending samples are the cycle's steps"
            );
        }
        if (status != HYPHA_STATUS_OK) {
            throw std::runtime_error(
                "hypha: clear() failed with status " + std::to_string(static_cast<int>(status))
            );
        }
    }

    /// @brief The port text of a start() error, with the failure detail when there is one.
    std::string describe(const TopicSource& topic) const
    {
        const std::string port =
            this->named() ? "input '" + this->name_ + "'" : describePort("input", topic);
        if (this->detail_.empty()) {
            return port;
        }
        return port + ": " + this->detail_;
    }

  private:
    friend class testing::StepHarness;

    std::string            name_;
    bool                   readsSource_{false};
    bool                   bound_{false};
    uint32_t               index_{0};
    const HyphaCycleImage* image_{nullptr};
    std::string            detail_;
    int64_t                silenceNanos_{0};
    bool                   decided_{false};
    bool                   untimed_{false};
};

}  // namespace detail

}  // namespace hypha
