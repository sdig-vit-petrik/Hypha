#pragma once

/// @file
/// A native quadrotor model: rigid body, four first-order motors in x configuration,
/// linear drag and a ground plane, integrated with fixed-step RK4.

#include <Eigen/Dense>
#include <Eigen/Geometry>

namespace mrs_sim {

constexpr double GRAVITY = 9.81;

/// Read from the [model] table of mrs_simulator.toml. The control_manager platform
/// parameters (uav_mass, motor_params/a,b) are derived from the same values.
struct ModelParams
{
    double          mass{2.0};  ///< kg
    Eigen::Vector3d inertia{0.0123, 0.0123, 0.0224};  ///< kg m^2, diagonal
    double          armLength{0.25};  ///< m, centre to motor
    double          kf{2.534e-7};  ///< N per rpm^2: thrust = kf rpm^2
    double          km{0.016};  ///< m: yaw torque = km thrust
    double          rpmMin{0.0};
    double          rpmMax{8000.0};
    double          motorTimeConstant{0.03};  ///< s, first-order lag
    double          linearDrag{0.1};  ///< N per m/s
    Eigen::Matrix4d allocation{Eigen::Matrix4d::Zero()};  ///< motor thrusts -> [T, tau]

    /// The generic 2 kg quadrotor of the port.
    static ModelParams defaults();
    /// The allocation of an x configuration from armLength and km. Motors: 0 (+x,+y),
    /// 1 (-x,-y) spinning one way, 2 (+x,-y), 3 (-x,+y) the other.
    void computeAllocation();

    /// rpm of a motor command in [0, 1]: linear between rpmMin and rpmMax.
    double rpmOf(double command) const
    {
        return this->rpmMin + command * (this->rpmMax - this->rpmMin);
    }

    /// The motor command whose steady thrust is @p thrust (N, one motor).
    double commandOf(double thrust) const;
    double hoverRpm() const;
    /// The throttle (a motor command on all four) that holds the weight.
    double hoverThrottle() const;
};

struct State
{
    Eigen::Vector3d    p{Eigen::Vector3d::Zero()};
    Eigen::Vector3d    v{Eigen::Vector3d::Zero()};
    Eigen::Quaterniond q{Eigen::Quaterniond::Identity()};
    Eigen::Vector3d    omega{Eigen::Vector3d::Zero()};  ///< body frame
    Eigen::Vector4d    rpm{Eigen::Vector4d::Zero()};
};

class MultirotorModel
{
  public:
    explicit MultirotorModel(ModelParams params);

    /// Advance by @p dt with @p motorCommand in [0, 1] (clamped). RK4; ground plane z >= 0.
    void step(double dt, const Eigen::Vector4d& motorCommand);

    const State& state() const
    {
        return this->state_;
    }

    void setState(const State& state)
    {
        this->state_ = state;
    }

    const ModelParams& params() const
    {
        return this->params_;
    }

    /// Specific force in the body frame (what an accelerometer reads), from the last step.
    Eigen::Vector3d acceleration() const
    {
        return this->specificForce_;
    }

    /// Linear acceleration in the world frame, from the last step.
    Eigen::Vector3d worldAcceleration() const
    {
        return this->worldAcceleration_;
    }

  private:
    using Vector17 = Eigen::Matrix<double, 17, 1>;
    Vector17        pack(const State& state) const;
    State           unpack(const Vector17& x) const;
    Vector17        derivative(const Vector17& x, const Eigen::Vector4d& rpmTarget) const;
    Eigen::Vector3d linearAcceleration(const State& state) const;

    ModelParams     params_;
    State           state_;
    Eigen::Vector3d specificForce_{0.0, 0.0, GRAVITY};
    Eigen::Vector3d worldAcceleration_{Eigen::Vector3d::Zero()};
};

}  // namespace mrs_sim
