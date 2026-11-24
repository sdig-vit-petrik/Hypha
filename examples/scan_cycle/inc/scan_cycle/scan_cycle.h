#pragma once

/// @file
/// ScanCycle: a software PLC cycle. The runtime freezes the inputs once per period and
/// judges their age; step() runs the control law on that image and sends one command.
/// - speed: Latest, required, `silence_ms` required. Not usable means command 0.0.
/// - setpoint: Latest, optionalTopic and advisory. Not usable means the fallback setpoint.

#include <hypha/hypha.h>

#include "scan_cycle/command_law.h"

#include <geometry_msgs/TwistStamped.h>
#include <std_msgs/Float64.h>

#include <cmath>
#include <cstdint>
#include <optional>
#include <string>

namespace scan_cycle {

class ScanCycle final : public hypha::CycleNode
{
  public:
    using Float64 = std_msgs::Float64;
    using Command = geometry_msgs::TwistStamped;

    Latest<Float64> speed{*this, "speed"};
    Latest<Float64> setpoint{*this, "setpoint", optionalTopic, advisory};
    Output<Command> command{*this, "command"};

    /// Reads every tuning value. The parameter file is loaded before the application is
    /// constructed, so a refused value stops the start with exit 78.
    ScanCycle()
        : fallbackSetpoint_(
              hypha::config().required<double>("fallback_setpoint", hypha::constraints::finite())
          )
        , gain_(hypha::config().required<double>("gain", hypha::constraints::finite()))
        , commandFrame_(
              hypha::config().required<std::string>("command_frame", hypha::constraints::nonempty())
          )
    {
    }

    void step(const hypha::CycleTime& time) override
    {
        // usable() is the age verdict of this step. It does not check the value.
        const bool speedUsable = this->speed.usable();
        const bool speedFinite = speedUsable && std::isfinite(this->speed->data);
        HYPHA_LOG_WARN_WHEN(!speedFinite, false, "speed unavailable: commanding no motion");
        if (!speedFinite) {
            ++this->noSpeedSteps_;
        }
        const bool setpointUsable = this->setpoint.usable();
        const bool setpointFinite = setpointUsable && std::isfinite(this->setpoint->data);
        const bool setpointNonFinite = setpointUsable && !setpointFinite;
        HYPHA_LOG_WARN_WHEN_THROTTLE(
            setpointNonFinite, false, 5.0, "setpoint value is not finite: using fallback setpoint"
        );
        const std::optional<double> speedValue =
            speedFinite ? std::optional{this->speed->data} : std::nullopt;
        const std::optional<double> setpointValue =
            setpointFinite ? std::optional{this->setpoint->data} : std::nullopt;
        const double lawOutput =
            controlCommand(speedValue, setpointValue, this->fallbackSetpoint_, this->gain_);
        // Finite inputs can still overflow in the product, so the output is checked too.
        const bool lawFinite = std::isfinite(lawOutput);
        HYPHA_LOG_WARN_WHEN_THROTTLE(
            !lawFinite, false, 5.0, "command is not finite ({}): commanding no motion", lawOutput
        );

        Command message;
        // The stamp of the step, not a clock read: equal in a live run and in a replay.
        message.header.stamp = time.stamp;
        message.header.frame_id = this->commandFrame_;
        message.twist.linear.x = lawFinite ? lawOutput : 0.0;
        this->command.send(message);
    }

    /// @brief The steps that commanded no motion for want of a usable, finite speed.
    uint64_t noSpeedSteps() const
    {
        return this->noSpeedSteps_;
    }

  private:
    double      fallbackSetpoint_;
    double      gain_;
    std::string commandFrame_;
    uint64_t    noSpeedSteps_{0};
};

}  // namespace scan_cycle
