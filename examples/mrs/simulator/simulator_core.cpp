#include "simulator_core.h"

#include <cstdio>

namespace mrs_sim {

SimulatorCore::SimulatorCore(SimulatorParams params)
    : params_(params), model_(params.model), controller_(params.rate, params.model)
{
}

void SimulatorCore::command(const AttitudeRateCommand& command, double stamp)
{
    this->command_ = command;
    this->commandStamp_ = stamp;
}

Answer SimulatorCore::arm(bool on)
{
    if (!on) {
        this->armed_ = false;
        this->offboard_ = false;
        return {true, "disarmed"};
    }
    const bool throttleUp =
        this->command_.has_value() && this->command_->throttle > this->params_.armThrottleLimit;
    if (throttleUp) {
        return {false, "arming refused: the throttle command is above the arming limit"};
    }
    this->armed_ = true;
    return {true, "armed"};
}

Answer SimulatorCore::offboard()
{
    if (!this->armed_) {
        return {false, "offboard refused: not armed"};
    }
    this->offboard_ = true;
    return {true, "offboard"};
}

void SimulatorCore::step(double now, double period)
{
    const bool fresh =
        this->command_.has_value() && now - this->commandStamp_ < this->params_.inputTimeout;
    const bool flying = this->armed_ && this->offboard_;
    if (flying && !fresh && !this->timedOut_) {
        std::fprintf(
            stderr,
            "mrs_simulator: no attitude-rate command for %.2f s: motors cut\n",
            this->params_.inputTimeout
        );
    }
    this->timedOut_ = flying && !fresh;
    const double dt = period / this->params_.substeps;
    for (int i = 0; i < this->params_.substeps; ++i) {
        if (flying && fresh) {
            this->motors_ =
                this->controller_.motorCommand(this->model_.state(), *this->command_, dt);
        } else {
            this->controller_.reset();
            this->motors_.setZero();
        }
        this->model_.step(dt, this->motors_);
    }
}

}  // namespace mrs_sim
