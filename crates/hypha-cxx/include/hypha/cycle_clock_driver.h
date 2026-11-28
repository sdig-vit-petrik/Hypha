#pragma once

/// @file
/// ClockDriverOptions: the settings of a cycle that drives `/clock` (Cycle::setClockDriver).

#include "hypha/backend/cycle.h"
#include "hypha_base/time.h"

#include <cstdint>
#include <optional>
#include <string>
#include <vector>

namespace hypha {

/// @brief Settings of a cycle that drives `/clock` for a simulator (Cycle::setClockDriver).
struct ClockDriverOptions
{
    /// The first `/clock` value, sent with no step: the first step runs one period later.
    /// From zero up to, not including, 2^32 seconds: `/clock` carries seconds in 32 bits, and
    /// start() refuses a value outside it, naming the range.
    Time start{};
    /// A participant that acknowledges nothing for this long fails the run with Timeout.
    Duration timeout = Duration::fromSeconds(5.0);
    /// Simulated seconds per host second. Empty: as fast as the participants allow.
    std::optional<double> realTimeFactor{};
    /// Node names that must acknowledge the start value before the first step; at least
    /// one. Any other node that reports on `/clock_progress` joins the participants.
    std::vector<std::string> participants{};
};

namespace detail {

/// @brief Hand @p options to an uncommitted cycle as its clock driver settings.
inline ::HyphaStatus bindClockDriver(HyphaCycle* cycle, const ClockDriverOptions& options)
{
    HyphaClockDriverOptions raw{};
    raw.start_nanos = options.start.nanos();
    const int64_t timeout = options.timeout.nanos();
    raw.timeout_nanos = timeout > 0 ? static_cast<uint64_t>(timeout) : 0;
    raw.real_time_factor = options.realTimeFactor.value_or(0.0);
    const bool zeroFactor = options.realTimeFactor.has_value() && *options.realTimeFactor == 0.0;
    if (zeroFactor) {
        return HYPHA_STATUS_INVALID_ARGUMENT;
    }
    ::HyphaStatus status = backend::cycleSetClockDriver(cycle, &raw);
    for (const std::string& name : options.participants) {
        if (status != HYPHA_STATUS_OK) {
            break;
        }
        status = backend::cycleExpectParticipant(cycle, name);
    }
    return status;
}

}  // namespace detail

}  // namespace hypha
