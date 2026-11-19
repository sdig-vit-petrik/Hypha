/// mission: replaces MRS autostart and TestGeneric. It runs one scenario as a sequence
/// of steps over 10 Hz cycles of simulated time and ends the run: exit 0 when the
/// scenario passes, exit 70 naming the failed step otherwise.
///
/// Scenarios (MRS_SCENARIO):
///   estimation               the UAV sits on the ground and uav_state flows
///   takeoff                  takeoff through control_manager alone (no uav_manager)
///   takeoff_via_uav_manager  takeoff through uav_manager, and a second takeoff refused
///   takeoff_goto             TestGeneric::takeoff() and gotoAbs()
///   goto_outside_safety_area takeoff, then a goto outside the safety area is refused
#include <hypha/hypha.h>
#include <hypha_log/hypha_log.h>

#include <mrs_host/plugin_host.h>
#include <mrs_host/service.h>
#include <mrs_msgs/ControlManagerDiagnostics.h>
#include <mrs_msgs/String.h>
#include <mrs_msgs/UavState.h>
#include <mrs_msgs/Vec1.h>
#include <mrs_msgs/Vec4.h>
#include <std_srvs/SetBool.h>
#include <std_srvs/Trigger.h>

#include <cmath>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <vector>

HYPHA_LOG_MODULE_INIT("mission")

namespace {

/// One step of a scenario: start() once, then done() every cycle until it decides.
struct Step
{
    std::string                          name;
    double                               timeout;
    std::function<void()>                start;
    std::function<std::optional<bool>()> done;
};

class Mission final : public hypha::CycleNode
{
  public:
    hypha::Arrivals<mrs_msgs::UavState> uav_state{*this, "uav_state", perStep(64), unsupervised};
    hypha::Latest<mrs_msgs::ControlManagerDiagnostics> control_manager_diagnostics{
        *this, "control_manager_diagnostics", unsupervised
    };

    void setup(hypha::Node&) override
    {
        this->scenario_ =
            hypha::config().required<std::string>("scenario", hypha::constraints::nonempty());
        const std::string uav =
            hypha::config().required<std::string>("uav_name", hypha::constraints::nonempty());
        this->host_ =
            std::make_unique<mrs::PluginHost>(*this, uav + "_mission", "/" + uav + "/mission");
        const hypha::Duration timeout = hypha::Duration::fromSeconds(1.0);
        const std::string     cm = "/" + uav + "/control_manager/";
        this->arming_ = std::make_unique<mrs::Request<std_srvs::SetBool>>(
            *this->host_, "/" + uav + "/hw_api/arming", timeout
        );
        this->offboard_ = std::make_unique<mrs::Request<std_srvs::Trigger>>(
            *this->host_, "/" + uav + "/hw_api/offboard", timeout
        );
        this->toggleOutput_ = std::make_unique<mrs::Request<std_srvs::SetBool>>(
            *this->host_, cm + "toggle_output", timeout
        );
        this->switchController_ = std::make_unique<mrs::Request<mrs_msgs::String>>(
            *this->host_, cm + "switch_controller", timeout
        );
        this->switchTracker_ = std::make_unique<mrs::Request<mrs_msgs::String>>(
            *this->host_, cm + "switch_tracker", timeout
        );
        this->landoffTakeoff_ = std::make_unique<mrs::Request<mrs_msgs::Vec1>>(
            *this->host_, cm + "landoff_tracker/takeoff", timeout
        );
        this->goto_ =
            std::make_unique<mrs::Request<mrs_msgs::Vec4>>(*this->host_, cm + "goto", timeout);
        // uav_manager answers a takeoff when it is done, so its timeout is the takeoff's.
        const hypha::Duration takeoffTimeout = hypha::Duration::fromSeconds(20.0);
        this->takeoff_ = std::make_unique<mrs::Request<std_srvs::Trigger>>(
            *this->host_, "/" + uav + "/uav_manager/takeoff", takeoffTimeout
        );
        this->secondTakeoff_ = std::make_unique<mrs::Request<std_srvs::Trigger>>(
            *this->host_, "/" + uav + "/uav_manager/takeoff", timeout
        );
        this->build();
        this->setRate(hypha::Hz(10.0));
    }

    void step(const hypha::CycleTime& time) override
    {
        this->host_->step(time, [this, &time] {
            this->run(time.now);
        });
    }

  private:
    void build()
    {
        if (this->scenario_ == "estimation") {
            this->steps_.push_back(this->estimation());
        } else if (this->scenario_ == "takeoff") {
            this->prepare();
            this->steps_.push_back(this->callString(
                "switch_controller Se3Controller", *this->switchController_, "Se3Controller"
            ));
            this->steps_.push_back(this->callString(
                "switch_tracker LandoffTracker", *this->switchTracker_, "LandoffTracker"
            ));
            this->steps_.push_back(
                this->call<mrs_msgs::Vec1>("landoff_tracker/takeoff", *this->landoffTakeoff_, [] {
                    mrs_msgs::Vec1::Request req;
                    req.goal = 2.0;
                    return req;
                }())
            );
            // uav_manager's rule: the LandoffTracker has no goal any more.
            this->steps_.push_back(this->wait("takeoff finished", 15.0, [this] {
                const auto* d = this->diagnostics();
                return d != nullptr && d->active_tracker == "LandoffTracker"
                    && !d->tracker_status.have_goal && std::abs(this->z() - 2.0) < 0.3;
            }));
            this->steps_.push_back(
                this->callString("switch_tracker LineTracker", *this->switchTracker_, "LineTracker")
            );
            this->steps_.push_back(this->wait("flying normally at 2 m", 5.0, [this] {
                const auto* d = this->diagnostics();
                return d != nullptr && d->flying_normally && std::abs(this->z() - 2.0) < 0.3;
            }));
        } else if (this->scenario_ == "takeoff_via_uav_manager") {
            this->prepare();
            this->steps_.push_back(this->takeoffViaUavManager(true));
            this->steps_.push_back(this->flyingNormallyWithLineTracker(20.0));
        } else if (this->scenario_ == "takeoff_goto") {
            this->prepare();
            this->steps_.push_back(this->takeoffViaUavManager(false));
            this->steps_.push_back(this->flyingNormallyWithLineTracker(20.0));
            this->steps_.push_back(
                this->call<mrs_msgs::Vec4>("goto [3.0, 2.0, 2.5, 0.0]", *this->goto_, [] {
                    mrs_msgs::Vec4::Request req;
                    req.goal = {3.0, 2.0, 2.5, 0.0};
                    return req;
                }())
            );
            this->steps_.push_back(this->reached(3.0, 2.0, 2.5, 30.0));
        } else if (this->scenario_ == "goto_outside_safety_area") {
            this->prepare();
            this->steps_.push_back(this->takeoffViaUavManager(false));
            this->steps_.push_back(this->flyingNormallyWithLineTracker(20.0));
            mrs_msgs::Vec4::Request outside;
            outside.goal = {100.0, 0.0, 2.0, 0.0};
            this->steps_.push_back(
                this->refused("goto [100.0, 0.0, 2.0, 0.0] is refused", *this->goto_, outside)
            );
        } else {
            throw std::runtime_error("mission: unknown scenario '" + this->scenario_ + "'");
        }
    }

    /// TestGeneric::takeoff() up to the takeoff call: ready, arm, output on, offboard.
    void prepare()
    {
        this->steps_.push_back(this->wait("wait until ready", 10.0, [this] {
            const auto* d = this->diagnostics();
            return d != nullptr && d->active_tracker == "NullTracker"
                && this->lastState_.has_value();
        }));
        this->steps_.push_back(this->call<std_srvs::SetBool>("hw_api/arming", *this->arming_, [] {
            std_srvs::SetBool::Request req;
            req.data = true;
            return req;
        }()));
        this->steps_.push_back(this->call<std_srvs::SetBool>(
            "control_manager/toggle_output", *this->toggleOutput_, [] {
                std_srvs::SetBool::Request req;
                req.data = true;
                return req;
            }()
        ));
        this->steps_.push_back(this->call<std_srvs::Trigger>(
            "hw_api/offboard", *this->offboard_, std_srvs::Trigger::Request{}
        ));
    }

    Step estimation()
    {
        return Step{
            "uav_state on the ground",
            3.0,
            [] {
            },
            [this]() -> std::optional<bool> {
                const hypha::Time end = *this->start_ + hypha::Duration::fromSeconds(2.0);
                if (this->now_ <= end) {
                    return std::nullopt;
                }
                const bool enough = this->groundSamples_ >= 190;
                this->detail_ = std::to_string(this->groundSamples_)
                              + " uav_state samples on the ground in 2 s"
                              + (enough ? "" : " (need 190)");
                return enough;
            }
        };
    }

    /// uav_manager/takeoff; with @p second, a second request 0.1 s later must be refused.
    Step takeoffViaUavManager(bool second)
    {
        auto secondAt = std::make_shared<std::optional<hypha::Time>>();
        auto secondOk = std::make_shared<std::optional<bool>>();
        return Step{
            "uav_manager/takeoff",
            21.0,
            [this, secondAt] {
                this->takeoff_->send(std_srvs::Trigger::Request{});
                *secondAt = this->now_ + hypha::Duration::fromSeconds(0.1);
            },
            [this, second, secondAt, secondOk]() -> std::optional<bool> {
                if (second && secondAt->has_value() && this->now_ >= **secondAt) {
                    this->secondTakeoff_->send(std_srvs::Trigger::Request{});
                    secondAt->reset();
                }
                if (second && !secondOk->has_value()) {
                    const mrs::CallState s = this->secondTakeoff_->state();
                    if (s == mrs::CallState::Done) {
                        const auto& res = this->secondTakeoff_->response();
                        *secondOk = !res.success && res.message == "takeoff in progress";
                        HYPHA_LOG_INFO(
                            "mission: second takeoff answered: success {}, '{}'",
                            res.success,
                            res.message
                        );
                        if (!**secondOk) {
                            this->detail_ = "a_second_takeoff_while_taking_off_is_refused: got '"
                                          + res.message + "'";
                            return false;
                        }
                    } else if (s == mrs::CallState::Failed || s == mrs::CallState::TimedOut) {
                        this->detail_ =
                            std::string("a_second_takeoff_while_taking_off_is_refused: ")
                            + mrs::toString(s);
                        return false;
                    }
                }
                const mrs::CallState state = this->takeoff_->state();
                if (state == mrs::CallState::Pending) {
                    return std::nullopt;
                }
                if (state != mrs::CallState::Done) {
                    this->detail_ = mrs::toString(state);
                    return false;
                }
                this->detail_ = this->takeoff_->response().message;
                if (second && !secondOk->has_value()) {
                    this->detail_ = "the takeoff answered before the second request was refused";
                    return false;
                }
                return this->takeoff_->response().success;
            }
        };
    }

    /// Flying normally with LineTracker at uav_manager's takeoff height (1.5 m).
    Step flyingNormallyWithLineTracker(double timeout)
    {
        return this->wait("flying normally with LineTracker at 1.5 m", timeout, [this] {
            const auto* d = this->diagnostics();
            return d != nullptr && d->flying_normally && d->active_tracker == "LineTracker"
                && std::abs(this->z() - 1.5) < 0.3;
        });
    }

    /// gotoAbs(): within 0.1 m of the goal and flying normally for 1 s.
    Step reached(double x, double y, double z, double timeout)
    {
        auto since = std::make_shared<std::optional<hypha::Time>>();
        return this->wait("reach the goto goal", timeout, [this, x, y, z, since] {
            const auto* d = this->diagnostics();
            const bool  inside = this->lastState_.has_value() && d != nullptr && d->flying_normally
                              && std::hypot(
                                     this->lastState_->pose.position.x - x,
                                     this->lastState_->pose.position.y - y,
                                     this->lastState_->pose.position.z - z
                                 ) < 0.1;
            if (!inside) {
                since->reset();
                return false;
            }
            if (!since->has_value()) {
                *since = this->now_;
            }
            return (this->now_ - **since).seconds() >= 1.0;
        });
    }

    Step wait(const std::string& name, double timeout, std::function<bool()> predicate)
    {
        return Step{
            name,
            timeout,
            [] {
            },
            [predicate]() -> std::optional<bool> {
                if (predicate()) {
                    return true;
                }
                return std::nullopt;
            }
        };
    }

    template <class Srv>
    Step call(const std::string& name, mrs::Request<Srv>& client, typename Srv::Request request)
    {
        return Step{
            name,
            10.0,
            [&client, request] {
                client.send(request);
            },
            [this, &client]() -> std::optional<bool> {
                const mrs::CallState state = client.state();
                if (state == mrs::CallState::Pending || state == mrs::CallState::Idle) {
                    return std::nullopt;
                }
                if (state != mrs::CallState::Done) {
                    this->detail_ = mrs::toString(state);
                    return false;
                }
                this->detail_ = client.response().message;
                return client.response().success;
            }
        };
    }

    /// A call that must be answered with success = false.
    template <class Srv>
    Step refused(const std::string& name, mrs::Request<Srv>& client, typename Srv::Request request)
    {
        Step step = this->call<Srv>(name, client, request);
        step.done = [this, &client]() -> std::optional<bool> {
            const mrs::CallState state = client.state();
            if (state == mrs::CallState::Pending || state == mrs::CallState::Idle) {
                return std::nullopt;
            }
            if (state != mrs::CallState::Done) {
                this->detail_ = mrs::toString(state);
                return false;
            }
            this->detail_ = client.response().message;
            return !client.response().success;
        };
        return step;
    }

    Step callString(
        const std::string& name, mrs::Request<mrs_msgs::String>& client, const std::string& value
    )
    {
        mrs_msgs::String::Request req;
        req.value = value;
        return this->call<mrs_msgs::String>(name, client, req);
    }

    const mrs_msgs::ControlManagerDiagnostics* diagnostics() const
    {
        return this->control_manager_diagnostics.has() ? &*this->control_manager_diagnostics
                                                       : nullptr;
    }

    double z() const
    {
        return this->lastState_ ? this->lastState_->pose.position.z : 0.0;
    }

    void run(hypha::Time now)
    {
        this->now_ = now;
        if (!this->start_) {
            this->start_ = now;
            HYPHA_LOG_INFO(
                "mission: scenario '{}' starts at {:.2f} s", this->scenario_, now.seconds()
            );
        }
        const hypha::Time window = *this->start_ + hypha::Duration::fromSeconds(2.0);
        for (const mrs_msgs::UavState& sample : this->uav_state.samples()) {
            this->lastState_ = sample;
            const hypha::Time stamp = hypha::Time::fromNanos(
                static_cast<int64_t>(sample.header.stamp.secs) * 1000000000
                + sample.header.stamp.nsecs
            );
            if (stamp >= *this->start_ && stamp <= window
                && std::abs(sample.pose.position.z) < 0.01) {
                ++this->groundSamples_;
            }
        }
        if (this->done_) {
            return;
        }
        if (this->current_ >= this->steps_.size()) {
            this->done_ = true;
            HYPHA_LOG_INFO("mission PASSED: scenario '{}'", this->scenario_);
            hypha::requestStop();
            return;
        }
        Step& step = this->steps_[this->current_];
        if (!this->stepStart_) {
            this->stepStart_ = now;
            this->detail_.clear();
            HYPHA_LOG_INFO("mission: step '{}'", step.name);
            step.start();
            return;
        }
        const std::optional<bool> result = step.done();
        if (result.has_value() && *result) {
            HYPHA_LOG_INFO(
                "mission: step '{}' passed at {:.2f} s{}",
                step.name,
                now.seconds(),
                this->detail_.empty() ? "" : ": " + this->detail_
            );
            ++this->current_;
            this->stepStart_.reset();
            return;
        }
        const bool timedOut = (now - *this->stepStart_).seconds() > step.timeout;
        if ((result.has_value() && !*result) || timedOut) {
            this->done_ = true;
            const std::string why = timedOut && !result.has_value()
                                      ? "timed out after " + std::to_string(step.timeout) + " s"
                                      : this->detail_;
            const std::string what =
                "step '" + step.name + "': " + why
                + (this->lastState_ ? " (z " + std::to_string(this->z()) + " m)" : "");
            HYPHA_LOG_ERROR("mission FAILED: {}", what);
            this->failRun(what);
        }
    }

    std::string                                      scenario_;
    std::unique_ptr<mrs::PluginHost>                 host_;
    std::unique_ptr<mrs::Request<std_srvs::SetBool>> arming_;
    std::unique_ptr<mrs::Request<std_srvs::Trigger>> offboard_;
    std::unique_ptr<mrs::Request<std_srvs::SetBool>> toggleOutput_;
    std::unique_ptr<mrs::Request<mrs_msgs::String>>  switchController_;
    std::unique_ptr<mrs::Request<mrs_msgs::String>>  switchTracker_;
    std::unique_ptr<mrs::Request<mrs_msgs::Vec1>>    landoffTakeoff_;
    std::unique_ptr<mrs::Request<mrs_msgs::Vec4>>    goto_;
    std::unique_ptr<mrs::Request<std_srvs::Trigger>> takeoff_;
    std::unique_ptr<mrs::Request<std_srvs::Trigger>> secondTakeoff_;
    std::vector<Step>                                steps_;
    size_t                                           current_{0};
    std::optional<hypha::Time>                       stepStart_;
    std::optional<hypha::Time>                       start_;
    hypha::Time                                      now_{};
    std::optional<mrs_msgs::UavState>                lastState_;
    uint64_t                                         groundSamples_{0};
    std::string                                      detail_;
    bool                                             done_{false};
};

}  // namespace

int main(int argc, char** argv)
{
    return hypha::runNode<Mission>(argc, argv, "uav1_mission");
}
