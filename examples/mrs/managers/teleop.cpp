/// teleop: fly uav1 from the keyboard. A 20 Hz cycle on simulated time reads the terminal
/// without blocking and sends a control_manager/goto for every move key.
///
/// Keys: t takeoff; w/s forward/back, a/d left/right, r/f up/down, q/e yaw left/right
/// (or the arrow keys for w/a/s/d); space holds the current position; Ctrl-C quits.
#include <hypha/hypha.h>
#include <hypha_log/hypha_log.h>

#include <mrs_host/plugin_host.h>
#include <mrs_host/service.h>
#include <mrs_msgs/ControlManagerDiagnostics.h>
#include <mrs_msgs/HwApiStatus.h>
#include <mrs_msgs/UavState.h>
#include <mrs_msgs/Vec4.h>
#include <std_srvs/SetBool.h>
#include <std_srvs/Trigger.h>
#include <termios.h>
#include <unistd.h>

#include <array>
#include <cmath>
#include <cstdio>
#include <memory>
#include <numbers>
#include <optional>
#include <stdexcept>
#include <string>

HYPHA_LOG_MODULE_INIT("teleop")

namespace {

/// Puts the terminal into unbuffered, no-echo mode and restores it at exit. Ctrl-C
/// still raises SIGINT, so hypha's handler stops the node.
class RawTerminal
{
  public:
    void enable()
    {
        if (isatty(STDIN_FILENO) == 0) {
            throw std::runtime_error("teleop: standard input must be a terminal");
        }
        if (tcgetattr(STDIN_FILENO, &this->saved_) != 0) {
            throw std::runtime_error("teleop: cannot read the terminal settings");
        }
        termios raw = this->saved_;
        raw.c_lflag &= ~static_cast<tcflag_t>(ICANON | ECHO);
        raw.c_cc[VMIN] = 0;
        raw.c_cc[VTIME] = 0;
        if (tcsetattr(STDIN_FILENO, TCSANOW, &raw) != 0) {
            throw std::runtime_error("teleop: cannot set the terminal to raw mode");
        }
        this->enabled_ = true;
    }

    ~RawTerminal()
    {
        if (this->enabled_) {
            tcsetattr(STDIN_FILENO, TCSANOW, &this->saved_);
            std::fputs("\n", stdout);
        }
    }

    /// The next key, or 0 when none is waiting. Arrow keys map to w/a/s/d.
    char key() const
    {
        char c = 0;
        if (read(STDIN_FILENO, &c, 1) != 1) {
            return 0;
        }
        if (c != '\033') {
            return c;
        }
        std::array<char, 2> seq{};
        if (read(STDIN_FILENO, seq.data(), 2) != 2 || seq[0] != '[') {
            return 0;
        }
        switch (seq[1]) {
            case 'A':
                return 'w';
            case 'B':
                return 's';
            case 'C':
                return 'd';
            case 'D':
                return 'a';
            default:
                return 0;
        }
    }

  private:
    termios saved_{};
    bool    enabled_{false};
};

RawTerminal terminal;  // static storage: restored on every exit path that runs destructors

enum class Phase { Ground, Arming, Output, Offboard, TakingOff, Flying };

const char* toString(Phase phase)
{
    switch (phase) {
        case Phase::Ground:
            return "on the ground";
        case Phase::Arming:
            return "arming";
        case Phase::Output:
            return "output on";
        case Phase::Offboard:
            return "offboard";
        case Phase::TakingOff:
            return "taking off";
        case Phase::Flying:
            return "flying";
    }
    return "?";
}

double headingOf(const mrs_msgs::UavState& state)
{
    const auto& q = state.pose.orientation;
    return std::atan2(2.0 * (q.w * q.z + q.x * q.y), 1.0 - 2.0 * (q.y * q.y + q.z * q.z));
}

class Teleop final : public hypha::CycleNode
{
  public:
    hypha::Latest<mrs_msgs::UavState>                  uav_state{*this, "uav_state", unsupervised};
    hypha::Latest<mrs_msgs::ControlManagerDiagnostics> control_manager_diagnostics{
        *this, "control_manager_diagnostics", unsupervised
    };
    hypha::Latest<mrs_msgs::HwApiStatus> hw_api_status{*this, "hw_api_status", unsupervised};

    void setup(hypha::Node&) override
    {
        const hypha::ConfigView config = hypha::config();
        const std::string       uav =
            config.required<std::string>("uav_name", hypha::constraints::nonempty());
        this->step_ = config.required<double>("step_m", hypha::constraints::positive());
        this->yawStep_ = config.required<double>("yaw_step_deg", hypha::constraints::positive())
                       * std::numbers::pi / 180.0;
        this->host_ =
            std::make_unique<mrs::PluginHost>(*this, uav + "_teleop", "/" + uav + "/teleop");
        const hypha::Duration timeout = hypha::Duration::fromSeconds(1.0);
        this->arming_ = std::make_unique<mrs::Request<std_srvs::SetBool>>(
            *this->host_, "/" + uav + "/hw_api/arming", timeout
        );
        this->offboard_ = std::make_unique<mrs::Request<std_srvs::Trigger>>(
            *this->host_, "/" + uav + "/hw_api/offboard", timeout
        );
        this->toggleOutput_ = std::make_unique<mrs::Request<std_srvs::SetBool>>(
            *this->host_, "/" + uav + "/control_manager/toggle_output", timeout
        );
        // uav_manager answers a takeoff when it is done.
        this->takeoff_ = std::make_unique<mrs::Request<std_srvs::Trigger>>(
            *this->host_, "/" + uav + "/uav_manager/takeoff", hypha::Duration::fromSeconds(20.0)
        );
        this->goto_ = std::make_unique<mrs::Request<mrs_msgs::Vec4>>(
            *this->host_, "/" + uav + "/control_manager/goto", timeout
        );
        this->setRate(hypha::Hz(20.0));
        terminal.enable();
        std::puts(
            "MRS teleop  |  t takeoff  |  w/s a/d (arrows) move  r/f up/down  q/e yaw  "
            "space hold  |  Ctrl-C quit"
        );
    }

    void step(const hypha::CycleTime& time) override
    {
        this->host_->step(time, [this] {
            for (char key = terminal.key(); key != 0; key = terminal.key()) {
                this->onKey(key);
            }
            this->advance();
            this->sendGoal();
            this->printStatus();
        });
    }

  private:
    void onKey(char key)
    {
        if (key == 't') {
            if (this->phase_ != Phase::Ground) {
                this->message_ = "takeoff: already " + std::string(toString(this->phase_));
            } else if (!this->ready()) {
                this->message_ = "takeoff: control_manager is not ready yet";
            } else {
                std_srvs::SetBool::Request req;
                req.data = true;
                this->arming_->send(req);
                this->phase_ = Phase::Arming;
                this->message_.clear();
            }
            return;
        }
        if (this->phase_ != Phase::Flying) {
            return;
        }
        if (!this->goal_) {
            this->goal_ = this->here();  // the first key, or after a refused first goal
        }
        if (!this->goal_) {
            return;
        }
        auto&        goal = *this->goal_;
        const double c = std::cos(goal[3]);
        const double s = std::sin(goal[3]);
        auto         move = [&](double forward, double left) {
            goal[0] += c * forward - s * left;
            goal[1] += s * forward + c * left;
        };
        switch (key) {
            case 'w':
                move(this->step_, 0.0);
                break;
            case 's':
                move(-this->step_, 0.0);
                break;
            case 'a':
                move(0.0, this->step_);
                break;
            case 'd':
                move(0.0, -this->step_);
                break;
            case 'r':
                goal[2] += this->step_;
                break;
            case 'f':
                goal[2] -= this->step_;
                break;
            case 'q':
                goal[3] += this->yawStep_;
                break;
            case 'e':
                goal[3] -= this->yawStep_;
                break;
            case ' ':
                this->goal_ = this->here();
                break;
            default:
                return;
        }
        this->dirty_ = true;
    }

    /// One takeoff call per phase; a refused or failed call returns to the ground.
    void advance()
    {
        switch (this->phase_) {
            case Phase::Ground:
            case Phase::Flying:
                return;
            case Phase::Arming:
                if (this->answered(*this->arming_)) {
                    std_srvs::SetBool::Request req;
                    req.data = true;
                    this->toggleOutput_->send(req);
                    this->phase_ = Phase::Output;
                }
                return;
            case Phase::Output:
                if (this->answered(*this->toggleOutput_)) {
                    this->offboard_->send(std_srvs::Trigger::Request{});
                    this->phase_ = Phase::Offboard;
                }
                return;
            case Phase::Offboard:
                // uav_manager refuses a takeoff until hw_api/status reports offboard.
                if (this->answered(*this->offboard_) && this->hw_api_status.has()
                    && this->hw_api_status->offboard) {
                    this->takeoff_->send(std_srvs::Trigger::Request{});
                    this->phase_ = Phase::TakingOff;
                }
                return;
            case Phase::TakingOff:
                // uav_manager answers before the climb ends: fly once LineTracker holds.
                if (this->answered(*this->takeoff_) && this->control_manager_diagnostics.has()
                    && this->control_manager_diagnostics->flying_normally
                    && this->control_manager_diagnostics->active_tracker == "LineTracker") {
                    this->phase_ = Phase::Flying;
                    this->message_ = "airborne: fly with w/a/s/d, r/f, q/e";
                }
                return;
        }
    }

    /// True once @p client answered success; on any failure back to the ground.
    template <class Srv>
    bool answered(mrs::Request<Srv>& client)
    {
        const mrs::CallState state = client.state();
        if (state == mrs::CallState::Pending || state == mrs::CallState::Idle) {
            return false;
        }
        if (state == mrs::CallState::Done && client.response().success) {
            return true;
        }
        this->message_ =
            std::string(toString(this->phase_)) + " failed: "
            + (state == mrs::CallState::Done ? client.response().message : mrs::toString(state));
        HYPHA_LOG_WARN("teleop: {}", this->message_);
        this->phase_ = Phase::Ground;
        return false;
    }

    /// One goto in flight; keys pressed meanwhile are merged into the next one.
    void sendGoal()
    {
        const mrs::CallState state = this->goto_->state();
        if (state == mrs::CallState::Pending) {
            return;
        }
        if (this->sent_) {
            this->sent_ = false;
            if (state != mrs::CallState::Done) {
                this->message_ = std::string("goto: ") + mrs::toString(state);
            } else if (!this->goto_->response().success) {
                this->message_ = "goto refused: " + this->goto_->response().message;
                this->goal_ = this->lastAccepted_;
            } else {
                this->lastAccepted_ = this->goal_;
                this->message_.clear();
            }
        }
        if (!this->dirty_ || !this->goal_) {
            return;
        }
        mrs_msgs::Vec4::Request req;
        req.goal = {(*this->goal_)[0], (*this->goal_)[1], (*this->goal_)[2], (*this->goal_)[3]};
        this->goto_->send(req);
        this->sent_ = true;
        this->dirty_ = false;
    }

    bool ready() const
    {
        return this->uav_state.has() && this->control_manager_diagnostics.has()
            && this->control_manager_diagnostics->active_tracker == "NullTracker";
    }

    std::optional<std::array<double, 4>> here() const
    {
        if (!this->uav_state.has()) {
            return std::nullopt;
        }
        const auto& p = this->uav_state->pose.position;
        return std::array<double, 4>{p.x, p.y, p.z, headingOf(*this->uav_state)};
    }

    void printStatus() const
    {
        std::string line = std::string("\r\033[K[") + toString(this->phase_) + "]";
        if (this->uav_state.has()) {
            const auto&          p = this->uav_state->pose.position;
            std::array<char, 96> buffer{};
            std::snprintf(
                buffer.data(),
                buffer.size(),
                " pos %6.2f %6.2f %5.2f  hdg %4.0f deg",
                p.x,
                p.y,
                p.z,
                headingOf(*this->uav_state) * 180.0 / std::numbers::pi
            );
            line += buffer.data();
        }
        if (this->goal_) {
            std::array<char, 64> buffer{};
            std::snprintf(
                buffer.data(),
                buffer.size(),
                "  goal %6.2f %6.2f %5.2f",
                (*this->goal_)[0],
                (*this->goal_)[1],
                (*this->goal_)[2]
            );
            line += buffer.data();
        }
        if (this->control_manager_diagnostics.has()) {
            line += "  " + this->control_manager_diagnostics->active_tracker;
        }
        if (!this->message_.empty()) {
            line += "  | " + this->message_;
        }
        std::fputs(line.c_str(), stdout);
        std::fflush(stdout);
    }

    std::unique_ptr<mrs::PluginHost>                 host_;
    std::unique_ptr<mrs::Request<std_srvs::SetBool>> arming_;
    std::unique_ptr<mrs::Request<std_srvs::Trigger>> offboard_;
    std::unique_ptr<mrs::Request<std_srvs::SetBool>> toggleOutput_;
    std::unique_ptr<mrs::Request<std_srvs::Trigger>> takeoff_;
    std::unique_ptr<mrs::Request<mrs_msgs::Vec4>>    goto_;
    double                                           step_{0.0};
    double                                           yawStep_{0.0};
    Phase                                            phase_{Phase::Ground};
    std::optional<std::array<double, 4>>             goal_;
    std::optional<std::array<double, 4>>             lastAccepted_;
    bool                                             dirty_{false};
    bool                                             sent_{false};
    std::string                                      message_;
};

}  // namespace

int main(int argc, char** argv)
{
    return hypha::runNode<Teleop>(argc, argv, "uav1_teleop");
}
