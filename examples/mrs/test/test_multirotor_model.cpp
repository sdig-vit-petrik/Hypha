/// The native multirotor model and its rate controller.
#include "check.h"
#include "multirotor_model.h"
#include "rate_controller.h"

#include <cmath>

using mrs_sim::AttitudeRateCommand;
using mrs_sim::ModelParams;
using mrs_sim::MultirotorModel;
using mrs_sim::RateController;
using mrs_sim::RateControllerParams;

namespace {

constexpr double DT = 0.001;

void rest_on_the_ground()
{
    MultirotorModel model(ModelParams::defaults());
    for (int i = 0; i < 2000; ++i) {
        model.step(DT, Eigen::Vector4d::Zero());
    }
    MRS_CHECK(model.state().p.z() == 0.0);
    MRS_CHECK(model.state().v.norm() == 0.0);
}

MultirotorModel hoveringAt(double z)
{
    const ModelParams params = ModelParams::defaults();
    MultirotorModel   model(params);
    mrs_sim::State    state = model.state();
    state.p = Eigen::Vector3d(0.0, 0.0, z);
    state.rpm = Eigen::Vector4d::Constant(params.hoverRpm());
    model.setState(state);
    return model;
}

void hover_holds_altitude()
{
    const ModelParams         params = ModelParams::defaults();
    MultirotorModel           model = hoveringAt(1.0);
    RateController            controller(RateControllerParams::defaults(), params);
    const AttitudeRateCommand hover{Eigen::Vector3d::Zero(), params.hoverThrottle()};
    for (int i = 0; i < 5000; ++i) {
        model.step(DT, controller.motorCommand(model.state(), hover, DT));
    }
    MRS_CHECK(std::abs(model.state().p.z() - 1.0) < 0.01);
}

void climbs_above_hover()
{
    const ModelParams         params = ModelParams::defaults();
    MultirotorModel           model(params);
    RateController            controller(RateControllerParams::defaults(), params);
    const AttitudeRateCommand climb{Eigen::Vector3d::Zero(), params.hoverThrottle() + 0.1};
    for (int i = 0; i < 1000; ++i) {
        model.step(DT, controller.motorCommand(model.state(), climb, DT));
    }
    MRS_CHECK(model.state().p.z() > 0.2);
}

void a_rate_step_settles()
{
    const ModelParams         params = ModelParams::defaults();
    MultirotorModel           model = hoveringAt(10.0);
    RateController            controller(RateControllerParams::defaults(), params);
    const AttitudeRateCommand roll{Eigen::Vector3d(1.0, 0.0, 0.0), params.hoverThrottle()};
    for (int i = 0; i < 500; ++i) {
        model.step(DT, controller.motorCommand(model.state(), roll, DT));
    }
    MRS_CHECK(std::abs(model.state().omega.x() - 1.0) < 0.05);
    MRS_CHECK(std::abs(model.state().omega.y()) < 0.05);
    MRS_CHECK(std::abs(model.state().omega.z()) < 0.05);
}

void motors_lag()
{
    const ModelParams params = ModelParams::defaults();
    MultirotorModel   model = hoveringAt(10.0);
    mrs_sim::State    state = model.state();
    state.rpm = Eigen::Vector4d::Constant(params.rpmMin);
    model.setState(state);
    const int steps = static_cast<int>(std::round(params.motorTimeConstant / DT));
    for (int i = 0; i < steps; ++i) {
        model.step(DT, Eigen::Vector4d::Ones());
    }
    const double fraction =
        (model.state().rpm(0) - params.rpmMin) / (params.rpmMax - params.rpmMin);
    MRS_CHECK(fraction > 0.60 && fraction < 0.67);
}

void the_imu_reads_gravity_at_rest()
{
    MultirotorModel model(ModelParams::defaults());
    model.step(DT, Eigen::Vector4d::Zero());
    MRS_CHECK(std::abs(model.acceleration().z() - 9.81) < 1e-6);
}

}  // namespace

int main()
{
    rest_on_the_ground();
    hover_holds_altitude();
    climbs_above_hover();
    a_rate_step_settles();
    motors_lag();
    the_imu_reads_gravity_at_rest();
    return mrsCheckResult();
}
