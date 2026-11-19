#pragma once

/// @file
/// The control law of the scan cycle: pure logic, no hypha name. Its tests need no
/// harness and no bus (test/test_command_law.cpp).

#include <optional>

namespace scan_cycle {

/// @brief The commanded acceleration, (setpoint - speed) * gain.
/// @param speed            The measured speed; empty when no usable, finite value exists.
/// @param setpoint         The wanted speed; empty when none is usable.
/// @param fallbackSetpoint Used in place of an empty @p setpoint.
/// @return 0.0 without a speed: no measurement means no motion.
inline double controlCommand(
    std::optional<double> speed,
    std::optional<double> setpoint,
    double                fallbackSetpoint,
    double                gain
)
{
    if (!speed.has_value()) {
        return 0.0;
    }
    return (setpoint.value_or(fallbackSetpoint) - *speed) * gain;
}

}  // namespace scan_cycle
