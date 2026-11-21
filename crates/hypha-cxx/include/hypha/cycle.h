#pragma once

/// @file
/// The values and helpers that the cycle ports share: the rate, the input verdict, the
/// step time and the health records.

#include "hypha/backend/cycle.h"
#include "hypha/backend/session.h"
#include "hypha/backend/typed.h"
#include "hypha/message_stamp.h"
#include "hypha/node.h"
#include "hypha/status.h"
#include "hypha/subscriber.h"
#include "hypha_base/time.h"

#include <cmath>
#include <cstddef>
#include <cstdint>
#include <expected>
#include <optional>
#include <stdexcept>
#include <string>

namespace hypha {

/// @brief The cumulative health of one input binding: its counts and its supervision
///        report. The faults are those supervision found, also while setIgnored(true)
///        hides them from the port's stale() and stagnant(), as `~/state` reports them.
struct BindingHealth : ::HyphaCycleBindingHealth
{
    bool stale = false;  ///< The input is silent for longer than `silence_ms`.
    bool stagnant = false;  ///< The life sign has not changed for `stagnation_ms`.
    bool ignored = false;  ///< setIgnored(true) overrides the fault.
};

/// @brief The cumulative health of one cycle.
using CycleHealth = ::HyphaCycleHealth;

/// @brief A positive periodic frequency, converted to a framework duration. A rate must be
///        finite and give a period from 1 ns through INT64_MAX ns.
class Hz
{
  public:
    explicit constexpr Hz(double value) : value_(value)
    {
    }

    constexpr double value() const
    {
        return this->value_;
    }

  private:
    double value_;
};

/// @brief What a step may do with an input.
enum class InputVerdict : uint32_t {
    /// No sample has arrived, as of this step.
    NeverArrived = 0,
    /// The input is within its silence limit.
    Usable = 1,
    /// The newest sample is older than the silence limit of the port.
    Stale = 2,
};

/// @brief Thrown by `*port` and `port->` of a Latest input that is not usable in this
///        step: stale, or no sample yet.
class InputNotUsable : public std::runtime_error
{
  public:
    InputNotUsable(const std::string& what, InputVerdict verdict)
        : std::runtime_error(what), verdict_(verdict)
    {
    }

    /// @brief The port's verdict at the throw: NeverArrived or Stale.
    InputVerdict verdict() const
    {
        return this->verdict_;
    }

  private:
    InputVerdict verdict_;
};

/// @brief The time of one step.
struct CycleTime
{
    /// Framework time read once, when the step starts; hypha::now() returns it while the
    /// step runs.
    Time now;
    /// `now` as a ROS header stamp.
    hypha_msgs::RosTime stamp{};
    /// The time since the previous step started. Empty in the first step.
    std::optional<Duration> dt;
    /// Periods that passed without a step since the previous one.
    uint64_t missed{0};
};

namespace detail {

template <typename T>
void destroyCycleContext(void* context) noexcept
{
    delete static_cast<T*>(context);
}

template <typename M>
::HyphaStatus decodeView(const HyphaCycleSampleView& view, M& output)
{
    const bool decoded = codec::decode(view.payload, view.payload_len, output);
    if (!decoded) {
        return HYPHA_STATUS_INVALID_FRAME;
    }
    return HYPHA_STATUS_OK;
}

/// Where a captured sample came from, read from its frame header.
struct SampleStamp
{
    uint64_t sentNanos{0};
    uint64_t publisherId{0};
    uint64_t publisherSequence{0};
};

inline SampleStamp stampOf(const HyphaCycleSampleView& view)
{
    return SampleStamp{view.transport_sent_nanos, view.publisher_id, view.publisher_sequence};
}

/// Decode sample @p sampleIndex of @p binding into @p output; @p view receives the sample's
/// view also when it does not decode (HYPHA_STATUS_INVALID_FRAME).
template <typename M>
::HyphaStatus readSample(
    const HyphaCycleImage* image,
    uint32_t               binding,
    size_t                 sampleIndex,
    M&                     output,
    HyphaCycleSampleView&  view
)
{
    const ::HyphaStatus status = backend::cycleImageSample(image, binding, sampleIndex, &view);
    if (status != HYPHA_STATUS_OK) {
        return status;
    }
    return decodeView(view, output);
}

/// The verdict the library reports; an unknown value is refused.
inline std::expected<InputVerdict, ::HyphaStatus> verdictFromRaw(int32_t verdict)
{
    if (verdict == HYPHA_CYCLE_VERDICT_USABLE) {
        return InputVerdict::Usable;
    }
    if (verdict == HYPHA_CYCLE_VERDICT_NEVER_ARRIVED) {
        return InputVerdict::NeverArrived;
    }
    if (verdict == HYPHA_CYCLE_VERDICT_STALE) {
        return InputVerdict::Stale;
    }
    return std::unexpected(HYPHA_STATUS_INVALID_STATE);
}

/// @brief The step's time values, from the runtime's raw record. A stamp the runtime
///        marks invalid is refused with INVALID_STATE.
inline std::expected<CycleTime, ::HyphaStatus> cycleTimeFrom(const HyphaCycleTime& raw)
{
    CycleTime time;
    time.now = Time::fromNanos(raw.callback_start_nanos);
    if (raw.has_elapsed != 0) {
        time.dt = Duration::fromNanos(raw.elapsed_nanos);
    }
    time.missed = raw.missed_periods;
    const bool stampInvalid = raw.message_stamp_valid == 0;
    if (stampInvalid) {
        return std::unexpected(HYPHA_STATUS_INVALID_STATE);
    }
    time.stamp = toRosTime(Time::fromNanos(raw.message_stamp_nanos));
    return time;
}

/// @brief The cycle period for a rate, or zero when the rate is not representable.
inline Duration periodFromHz(Hz rate)
{
    const double rateValue = rate.value();
    const bool   validRate = std::isfinite(rateValue) && rateValue > 0.0;
    if (!validRate) {
        return Duration::fromNanos(0);
    }
    const long double periodNanos = 1.0e9L / static_cast<long double>(rateValue);
    const bool        representable = periodNanos >= 1.0L && periodNanos < std::ldexp(1.0L, 63);
    if (!representable) {
        return Duration::fromNanos(0);
    }
    return Duration::fromNanos(static_cast<int64_t>(periodNanos));
}

/// @brief Destroys a cycle that setup did not finish; a refusal is logged.
struct CycleSetupGuard
{
    HyphaCycle* cycle;
    bool        armed{true};

    ~CycleSetupGuard() noexcept
    {
        if (!this->armed) {
            return;
        }
        const ::HyphaStatus status = backend::cycleDestroy(this->cycle);
        if (status != HYPHA_STATUS_OK) {
            detail::warn(
                "cycle setup rollback failed with status "
                + std::to_string(static_cast<int>(status))
            );
        }
    }

    void release() noexcept
    {
        this->armed = false;
    }
};

}  // namespace detail

}  // namespace hypha
