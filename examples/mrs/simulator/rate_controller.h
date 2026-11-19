#pragma once

/// @file
/// The simulator's flight controller: PID on body rates, then the mixer, as a real
/// autopilot closes the rate loop under an `attitude_rate` command.

#include "multirotor_model.h"

namespace mrs_sim {

struct AttitudeRateCommand
{
    Eigen::Vector3d rates{Eigen::Vector3d::Zero()};  ///< body rates, rad/s
    double          throttle{0.0};  ///< [0, 1], a motor command on all four
};

/// Read from the [rate_controller] table of mrs_simulator.toml.
struct RateControllerParams
{
    Eigen::Vector3d kp{20.0, 20.0, 10.0};  ///< 1/s: angular acceleration per rate error
    Eigen::Vector3d ki{2.0, 2.0, 1.0};
    Eigen::Vector3d kd{0.0, 0.0, 0.0};
    double          integralLimit{2.0};  ///< rad/s s

    static RateControllerParams defaults()
    {
        return RateControllerParams{};
    }
};

class RateController
{
  public:
    RateController(RateControllerParams gains, ModelParams model);

    /// Motor commands in [0, 1] for @p command at @p state; @p dt is the controller period.
    Eigen::Vector4d motorCommand(const State& state, const AttitudeRateCommand& command, double dt);
    void            reset();

  private:
    RateControllerParams gains_;
    ModelParams          model_;
    Eigen::Matrix4d      mixer_;
    Eigen::Vector3d      integral_{Eigen::Vector3d::Zero()};
    Eigen::Vector3d      previousError_{Eigen::Vector3d::Zero()};
};

}  // namespace mrs_sim
