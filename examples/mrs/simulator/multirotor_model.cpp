#include "multirotor_model.h"

#include <algorithm>
#include <cmath>

namespace mrs_sim {

ModelParams ModelParams::defaults()
{
    ModelParams params;
    params.computeAllocation();
    return params;
}

void ModelParams::computeAllocation()
{
    const double d = this->armLength / std::sqrt(2.0);
    // rows: collective thrust, roll torque (y f), pitch torque (-x f), yaw torque (km s f)
    this->allocation << 1.0, 1.0, 1.0, 1.0,  //
        d, -d, -d, d,  //
        -d, d, -d, d,  //
        this->km, this->km, -this->km, -this->km;
}

double ModelParams::commandOf(double thrust) const
{
    const double rpm = std::sqrt(std::max(thrust, 0.0) / this->kf);
    return std::clamp((rpm - this->rpmMin) / (this->rpmMax - this->rpmMin), 0.0, 1.0);
}

double ModelParams::hoverRpm() const
{
    return std::sqrt(this->mass * GRAVITY / 4.0 / this->kf);
}

double ModelParams::hoverThrottle() const
{
    return this->commandOf(this->mass * GRAVITY / 4.0);
}

MultirotorModel::MultirotorModel(ModelParams params) : params_(params)
{
    if (this->params_.allocation.isZero()) {
        this->params_.computeAllocation();
    }
}

MultirotorModel::Vector17 MultirotorModel::pack(const State& s) const
{
    Vector17 x;
    x << s.p, s.v, s.q.w(), s.q.x(), s.q.y(), s.q.z(), s.omega, s.rpm;
    return x;
}

State MultirotorModel::unpack(const Vector17& x) const
{
    State s;
    s.p = x.segment<3>(0);
    s.v = x.segment<3>(3);
    s.q = Eigen::Quaterniond(x(6), x(7), x(8), x(9));
    s.omega = x.segment<3>(10);
    s.rpm = x.segment<4>(13);
    return s;
}

Eigen::Vector3d MultirotorModel::linearAcceleration(const State& s) const
{
    const Eigen::Vector4d thrusts = this->params_.kf * s.rpm.array().square();
    const double          collective = thrusts.sum();
    const Eigen::Vector3d force =
        s.q.normalized() * Eigen::Vector3d(0.0, 0.0, collective) - this->params_.linearDrag * s.v;
    return force / this->params_.mass - Eigen::Vector3d(0.0, 0.0, GRAVITY);
}

MultirotorModel::Vector17
MultirotorModel::derivative(const Vector17& x, const Eigen::Vector4d& rpmTarget) const
{
    const State           s = this->unpack(x);
    const Eigen::Vector4d thrusts = this->params_.kf * s.rpm.array().square();
    const Eigen::Vector4d wrench = this->params_.allocation * thrusts;
    const Eigen::Vector3d torque = wrench.tail<3>();
    const Eigen::Vector3d inertia = this->params_.inertia;
    const Eigen::Vector3d angularMomentum = inertia.cwiseProduct(s.omega);
    const Eigen::Vector3d omegaDot =
        (torque - s.omega.cross(angularMomentum)).cwiseQuotient(inertia);
    const Eigen::Quaterniond omegaQuat(0.0, s.omega.x(), s.omega.y(), s.omega.z());
    const Eigen::Quaterniond qDot = s.q * omegaQuat;

    Vector17 dx;
    dx.segment<3>(0) = s.v;
    dx.segment<3>(3) = this->linearAcceleration(s);
    dx(6) = 0.5 * qDot.w();
    dx(7) = 0.5 * qDot.x();
    dx(8) = 0.5 * qDot.y();
    dx(9) = 0.5 * qDot.z();
    dx.segment<3>(10) = omegaDot;
    dx.segment<4>(13) = (rpmTarget - s.rpm) / this->params_.motorTimeConstant;
    return dx;
}

void MultirotorModel::step(double dt, const Eigen::Vector4d& motorCommand)
{
    Eigen::Vector4d rpmTarget;
    for (int i = 0; i < 4; ++i) {
        const double command =
            std::isfinite(motorCommand(i)) ? std::clamp(motorCommand(i), 0.0, 1.0) : 0.0;
        rpmTarget(i) = this->params_.rpmOf(command);
    }
    const Vector17 x = this->pack(this->state_);
    const Vector17 k1 = this->derivative(x, rpmTarget);
    const Vector17 k2 = this->derivative(x + 0.5 * dt * k1, rpmTarget);
    const Vector17 k3 = this->derivative(x + 0.5 * dt * k2, rpmTarget);
    const Vector17 k4 = this->derivative(x + dt * k3, rpmTarget);
    State          next = this->unpack(x + dt / 6.0 * (k1 + 2.0 * k2 + 2.0 * k3 + k4));
    next.q.normalize();

    // Ground plane: the UAV rests on it while the net vertical force points down.
    Eigen::Vector3d acceleration = this->linearAcceleration(next);
    if (next.p.z() <= 0.0 && acceleration.z() <= 0.0) {
        next.p.z() = 0.0;
        next.v.setZero();
        next.omega.setZero();
        acceleration.setZero();
    } else if (next.p.z() < 0.0) {
        next.p.z() = 0.0;
        next.v.z() = std::max(next.v.z(), 0.0);
    }
    this->state_ = next;
    this->worldAcceleration_ = acceleration;
    this->specificForce_ = next.q.conjugate() * (acceleration + Eigen::Vector3d(0.0, 0.0, GRAVITY));
}

}  // namespace mrs_sim
