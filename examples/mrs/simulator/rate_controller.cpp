#include "rate_controller.h"

#include <algorithm>

namespace mrs_sim {

RateController::RateController(RateControllerParams gains, ModelParams model)
    : gains_(gains), model_(model)
{
    if (this->model_.allocation.isZero()) {
        this->model_.computeAllocation();
    }
    this->mixer_ = this->model_.allocation.inverse();
}

void RateController::reset()
{
    this->integral_.setZero();
    this->previousError_.setZero();
}

Eigen::Vector4d
RateController::motorCommand(const State& state, const AttitudeRateCommand& command, double dt)
{
    const Eigen::Vector3d error = command.rates - state.omega;
    this->integral_ = (this->integral_ + error * dt)
                          .cwiseMax(-this->gains_.integralLimit)
                          .cwiseMin(this->gains_.integralLimit);
    const Eigen::Vector3d derivative =
        dt > 0.0 ? Eigen::Vector3d((error - this->previousError_) / dt) : Eigen::Vector3d::Zero();
    this->previousError_ = error;
    const Eigen::Vector3d angularAcceleration = this->gains_.kp.cwiseProduct(error)
                                              + this->gains_.ki.cwiseProduct(this->integral_)
                                              + this->gains_.kd.cwiseProduct(derivative);
    const Eigen::Vector3d inertia = this->model_.inertia;
    const Eigen::Vector3d torque = inertia.cwiseProduct(angularAcceleration)
                                 + state.omega.cross(inertia.cwiseProduct(state.omega));

    // The throttle is a motor command on all four: its thrust is the collective.
    const double throttle = std::clamp(command.throttle, 0.0, 1.0);
    const double rpm = this->model_.rpmOf(throttle);
    const double collective = 4.0 * this->model_.kf * rpm * rpm;
    if (throttle <= 0.0) {
        this->reset();
        return Eigen::Vector4d::Zero();
    }
    Eigen::Vector4d wrench;
    wrench << collective, torque;
    const Eigen::Vector4d thrusts = this->mixer_ * wrench;
    Eigen::Vector4d       motors;
    for (int i = 0; i < 4; ++i) {
        motors(i) = this->model_.commandOf(thrusts(i));
    }
    return motors;
}

}  // namespace mrs_sim
