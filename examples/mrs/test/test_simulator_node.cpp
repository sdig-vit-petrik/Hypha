/// The simulator's command handling, without transport: arming, offboard, the input
/// timeout and the motor cut.
#include "check.h"
#include "simulator_core.h"

using mrs_sim::AttitudeRateCommand;
using mrs_sim::SimulatorCore;
using mrs_sim::SimulatorParams;

namespace {

constexpr double PERIOD = 0.01;

/// Step from @p from to @p to; when @p refresh, send @p command with each step's stamp.
double
run(SimulatorCore& sim, double from, double to, const AttitudeRateCommand& command, bool refresh)
{
    double t = from;
    while (t < to - 1e-9) {
        if (refresh) {
            sim.command(command, t);
        }
        t += PERIOD;
        sim.step(t, PERIOD);
    }
    return t;
}

void unarmed_ignores_commands()
{
    SimulatorCore sim(SimulatorParams::defaults());
    run(sim, 0.0, 1.0, AttitudeRateCommand{Eigen::Vector3d::Zero(), 0.8}, true);
    MRS_CHECK(sim.model().state().p.z() == 0.0);
}

void armed_offboard_climbs()
{
    const SimulatorParams params = SimulatorParams::defaults();
    SimulatorCore         sim(params);
    MRS_CHECK(sim.arm(true).success);
    MRS_CHECK(sim.offboard().success);
    run(sim,
        0.0,
        1.0,
        AttitudeRateCommand{Eigen::Vector3d::Zero(), params.model.hoverThrottle() + 0.1},
        true);
    MRS_CHECK(sim.model().state().p.z() > 0.2);
}

void a_stale_command_cuts_the_motors()
{
    const SimulatorParams params = SimulatorParams::defaults();
    SimulatorCore         sim(params);
    sim.arm(true);
    sim.offboard();
    sim.command(
        AttitudeRateCommand{Eigen::Vector3d::Zero(), params.model.hoverThrottle() + 0.1}, 0.0
    );
    double t = run(sim, 0.0, 0.5, {}, false);
    MRS_CHECK(!sim.lastMotorCommand().isZero());  // still young: the command flies
    t = run(sim, t, 1.1, {}, false);
    MRS_CHECK(sim.lastMotorCommand().isZero());
}

void arming_in_flight_is_refused()
{
    SimulatorCore sim(SimulatorParams::defaults());
    sim.command(AttitudeRateCommand{Eigen::Vector3d::Zero(), 0.5}, 0.0);
    sim.step(PERIOD, PERIOD);
    MRS_CHECK(!sim.arm(true).success);
    MRS_CHECK(!sim.armed());
}

void offboard_needs_arming()
{
    SimulatorCore sim(SimulatorParams::defaults());
    MRS_CHECK(!sim.offboard().success);
    sim.arm(true);
    MRS_CHECK(sim.offboard().success);
    sim.arm(false);
    MRS_CHECK(!sim.isOffboard());
}

}  // namespace

int main()
{
    unarmed_ignores_commands();
    armed_offboard_climbs();
    a_stale_command_cuts_the_motors();
    arming_in_flight_is_refused();
    offboard_needs_arming();
    return mrsCheckResult();
}
