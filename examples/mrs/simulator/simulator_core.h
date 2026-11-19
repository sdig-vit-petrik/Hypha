#pragma once

/// @file
/// The simulator without transport: the model, the flight controller, arming,
/// offboard and the input timeout. mrs_simulator.cpp puts the hw_api topics around it.

#include "multirotor_model.h"
#include "rate_controller.h"

#include <optional>
#include <string>

namespace mrs_sim {

struct SimulatorParams
{
    ModelParams          model{ModelParams::defaults()};
    RateControllerParams rate{RateControllerParams::defaults()};
    double               inputTimeout{1.0};  ///< s: an older command cuts the motors
    int                  substeps{10};  ///< model steps per clock step
    double               armThrottleLimit{0.1};  ///< arming is refused above this throttle

    static SimulatorParams defaults()
    {
        return SimulatorParams{};
    }
};

struct Answer
{
    bool        success{false};
    std::string message;
};

class SimulatorCore
{
  public:
    explicit SimulatorCore(SimulatorParams params);

    /// The newest attitude-rate command, stamped @p stamp (s).
    void   command(const AttitudeRateCommand& command, double stamp);
    Answer arm(bool on);
    Answer offboard();

    /// Advance to @p now over @p period: the controller runs on the newest command when
    /// armed, offboard and the command is younger than the input timeout; else the
    /// motors get zero.
    void step(double now, double period);

    bool armed() const
    {
        return this->armed_;
    }

    bool isOffboard() const
    {
        return this->offboard_;
    }

    const MultirotorModel& model() const
    {
        return this->model_;
    }

    const Eigen::Vector4d& lastMotorCommand() const
    {
        return this->motors_;
    }

    const SimulatorParams& params() const
    {
        return this->params_;
    }

  private:
    SimulatorParams                    params_;
    MultirotorModel                    model_;
    RateController                     controller_;
    std::optional<AttitudeRateCommand> command_;
    double                             commandStamp_{0.0};
    bool                               armed_{false};
    bool                               offboard_{false};
    bool                               timedOut_{false};
    Eigen::Vector4d                    motors_{Eigen::Vector4d::Zero()};
};

}  // namespace mrs_sim
