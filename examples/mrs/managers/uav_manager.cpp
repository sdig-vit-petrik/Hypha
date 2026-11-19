/// uav_manager: the takeoff path of upstream MRS UavManager (callbackTakeoff,
/// takeoffSrv, timerTakeoff, timerDiagnostics) as a native hypha CycleNode at 100 Hz.
/// Upstream's takeoff blocks on service calls and a 0.3 s sleep. Here a state machine over
/// steps does the calls and answers the request when the LandoffTracker took the takeoff.
#include <hypha/hypha.h>
#include <hypha_log/hypha_log.h>

#include <mrs_host/plugin_host.h>
#include <mrs_host/service.h>
#include <mrs_lib/param_loader.h>
#include <mrs_msgs/ControlManagerDiagnostics.h>
#include <mrs_msgs/ControllerDiagnostics.h>
#include <mrs_msgs/HwApiStatus.h>
#include <mrs_msgs/String.h>
#include <mrs_msgs/UavManagerDiagnostics.h>
#include <mrs_msgs/Vec1.h>
#include <nav_msgs/Odometry.h>
#include <ros/node_handle.h>
#include <std_srvs/SetBool.h>
#include <std_srvs/Trigger.h>

#include <memory>
#include <optional>
#include <string>

HYPHA_LOG_MODULE_INIT("uav_manager")

namespace {

enum class Takeoff {
    Idle,
    SwitchController,
    SwitchTracker,
    Wait,
    CallTakeoff,
    WaitingForTakeoff,
    TakingOff,
    AfterTracker,
    AfterController
};

class UavManager final : public hypha::CycleNode
{
  public:
    hypha::Latest<nav_msgs::Odometry>    odometry{*this, "odometry", unsupervised};
    hypha::Latest<mrs_msgs::HwApiStatus> hw_api_status{*this, "hw_api_status", unsupervised};
    hypha::Latest<mrs_msgs::ControlManagerDiagnostics> control_manager_diagnostics{
        *this, "control_manager_diagnostics", unsupervised
    };
    hypha::Latest<mrs_msgs::ControllerDiagnostics> controller_diagnostics{
        *this, "controller_diagnostics", unsupervised
    };
    hypha::Output<mrs_msgs::UavManagerDiagnostics> diagnostics{*this, "diagnostics"};

    void setup(hypha::Node&) override
    {
        const std::string uav =
            hypha::config().required<std::string>("uav_name", hypha::constraints::nonempty());
        this->host_ = std::make_unique<mrs::PluginHost>(
            *this, uav + "_uav_manager", "/" + uav + "/uav_manager"
        );
        ros::NodeHandle      nh(*this->host_, "~");
        mrs_lib::ParamLoader param_loader(nh, "UavManager");
        param_loader.loadParam("uav_name", this->_uav_name_);
        const std::string yaml_prefix = "mrs_uav_managers/uav_manager/";
        param_loader.loadParam(yaml_prefix + "null_tracker", this->_null_tracker_name_);
        param_loader.loadParam(
            yaml_prefix + "takeoff/after_takeoff/tracker", this->_after_takeoff_tracker_name_
        );
        param_loader.loadParam(
            yaml_prefix + "takeoff/after_takeoff/controller", this->_after_takeoff_controller_name_
        );
        param_loader.loadParam(
            yaml_prefix + "takeoff/during_takeoff/controller", this->_takeoff_controller_name_
        );
        param_loader.loadParam(
            yaml_prefix + "takeoff/during_takeoff/tracker", this->_takeoff_tracker_name_
        );
        param_loader.loadParam(yaml_prefix + "takeoff/takeoff_height", this->_takeoff_height_);
        param_loader.loadParam(yaml_prefix + "require_gain_manager", this->_gain_manager_required_);
        param_loader.loadParam(
            yaml_prefix + "require_constraint_manager", this->_constraint_manager_required_
        );
        param_loader.loadParam(yaml_prefix + "diagnostics/rate", this->_diagnostics_rate_);
        if (!param_loader.loadedSuccessfully()) {
            throw std::runtime_error("uav_manager: could not load all parameters");
        }
        if (this->_gain_manager_required_ || this->_constraint_manager_required_) {
            throw std::runtime_error(
                "uav_manager: the gain and constraint managers are not ported; set require_* to "
                "false"
            );
        }

        const hypha::Duration timeout = hypha::Duration::fromSeconds(1.0);
        const std::string     cm = "/" + uav + "/control_manager/";
        this->switchController_ = std::make_unique<mrs::Request<mrs_msgs::String>>(
            *this->host_, cm + "switch_controller", timeout
        );
        this->switchTracker_ = std::make_unique<mrs::Request<mrs_msgs::String>>(
            *this->host_, cm + "switch_tracker", timeout
        );
        this->takeoffCall_ = std::make_unique<mrs::Request<mrs_msgs::Vec1>>(
            *this->host_, cm + "landoff_tracker/takeoff", timeout
        );
        this->toggleOutput_ = std::make_unique<mrs::Request<std_srvs::SetBool>>(
            *this->host_, cm + "toggle_output", timeout
        );
        this->arming_ = std::make_unique<mrs::Request<std_srvs::SetBool>>(
            *this->host_, "/" + uav + "/hw_api/arming", timeout
        );
        this->takeoffServer_ = std::make_unique<mrs::Serve<std_srvs::Trigger>>(
            *this->host_,
            "/" + uav + "/uav_manager/takeoff",
            [this](uint64_t ticket, const std_srvs::Trigger::Request&) {
                return this->callbackTakeoff(ticket);
            }
        );
        this->diagnosticsTimer_ =
            nh.createTimer(ros::Rate(this->_diagnostics_rate_), [this](const ros::TimerEvent&) {
                this->publishDiagnostics();
            });
        this->setRate(hypha::Hz(100.0));
    }

    void step(const hypha::CycleTime& time) override
    {
        this->host_->step(time, [this] {
            this->runTakeoff();
        });
    }

  private:
    std::optional<std_srvs::Trigger::Response> refuse(const std::string& message)
    {
        std_srvs::Trigger::Response res;
        res.success = false;
        res.message = message;
        HYPHA_LOG_WARN("takeoff refused: {}", message);
        return res;
    }

    /// The checks of upstream callbackTakeoff(); on success the state machine answers later.
    std::optional<std_srvs::Trigger::Response> callbackTakeoff(uint64_t ticket)
    {
        if (this->state_ != Takeoff::Idle) {
            return this->refuse("takeoff in progress");
        }
        const ros::Time now = ros::Time::now();
        if (!this->odometry.has()) {
            return this->refuse("can not takeoff, missing odometry!");
        }
        if (!this->hw_api_status.has()
            || (now - ros::Time(this->hw_api_status->stamp)).toSec() > 5.0) {
            return this->refuse("can not takeoff, missing HW API status!");
        }
        if (!this->hw_api_status->armed) {
            return this->refuse("can not takeoff, UAV not armed!");
        }
        if (!this->hw_api_status->offboard) {
            return this->refuse("can not takeoff, UAV not in offboard mode!");
        }
        if (!this->control_manager_diagnostics.has()) {
            return this->refuse("can not takeoff, missing control manager diagnostics!");
        }
        const mrs_msgs::ControlManagerDiagnostics& diag = *this->control_manager_diagnostics;
        if (diag.active_tracker != this->_null_tracker_name_) {
            return this->refuse(
                "can not takeoff, need '" + this->_null_tracker_name_ + "' to be active!"
            );
        }
        if (!this->controller_diagnostics.has()) {
            return this->refuse("can not takeoff, missing controller diagnostics!");
        }
        if (!diag.output_enabled) {
            return this->refuse("can not takeoff, Control Manager's output is disabled!");
        }
        this->ticket_ = ticket;
        this->enter(Takeoff::SwitchController);
        return std::nullopt;
    }

    void enter(Takeoff state)
    {
        this->state_ = state;
        this->entered_ = ros::Time::now();
        this->sent_ = false;
    }

    /// One call of the sequence: send once, then true (done, success), false (failed), nullopt.
    template <class Srv>
    std::optional<bool> callStep(mrs::Request<Srv>& client, const typename Srv::Request& request)
    {
        if (!this->sent_) {
            this->sent_ = client.send(request);
            return std::nullopt;
        }
        switch (client.state()) {
            case mrs::CallState::Pending:
            case mrs::CallState::Idle:
                return std::nullopt;
            case mrs::CallState::Done:
                this->lastMessage_ = client.response().message;
                return client.response().success;
            case mrs::CallState::Failed:
            case mrs::CallState::TimedOut:
                this->lastMessage_ = mrs::toString(client.state());
                return false;
        }
        return false;
    }

    /// Any failure during the takeoff: output off, disarm, answer naming the step.
    void abort(const std::string& message)
    {
        this->toggleOutput_->send(std_srvs::SetBool::Request{});  // data = false
        this->arming_->send(std_srvs::SetBool::Request{});  // data = false
        std_srvs::Trigger::Response res;
        res.success = false;
        res.message = message + " (" + this->lastMessage_ + ")";
        HYPHA_LOG_ERROR("takeoff failed: {}", res.message);
        this->takeoffServer_->respond(this->ticket_, res);
        this->state_ = Takeoff::Idle;
    }

    void runTakeoff()
    {
        mrs_msgs::String::Request name;
        switch (this->state_) {
            case Takeoff::Idle:
                return;
            case Takeoff::SwitchController: {
                name.value = this->_takeoff_controller_name_;
                const std::optional<bool> r = this->callStep(*this->switchController_, name);
                if (r && *r) {
                    this->enter(Takeoff::SwitchTracker);
                } else if (r) {
                    this->abort(
                        "could not activate '" + this->_takeoff_controller_name_ + "' for takeoff"
                    );
                }
                return;
            }
            case Takeoff::SwitchTracker: {
                name.value = this->_takeoff_tracker_name_;
                const std::optional<bool> r = this->callStep(*this->switchTracker_, name);
                if (r && *r) {
                    this->enter(Takeoff::Wait);
                } else if (r) {
                    this->abort(
                        "could not activate '" + this->_takeoff_tracker_name_ + "' for takeoff"
                    );
                }
                return;
            }
            case Takeoff::Wait:
                // Upstream sleeps 0.3 s here.
                if ((ros::Time::now() - this->entered_).toSec() >= 0.3) {
                    this->enter(Takeoff::CallTakeoff);
                }
                return;
            case Takeoff::CallTakeoff: {
                mrs_msgs::Vec1::Request req;
                req.goal = this->_takeoff_height_;
                const std::optional<bool> r = this->callStep(*this->takeoffCall_, req);
                if (r && *r) {
                    std_srvs::Trigger::Response res;
                    res.success = true;
                    res.message = "taking off";
                    this->takeoffServer_->respond(this->ticket_, res);
                    ++this->number_of_takeoffs_;
                    HYPHA_LOG_INFO("taking off to {:.2f} m", this->_takeoff_height_);
                    this->enter(Takeoff::WaitingForTakeoff);
                } else if (r) {
                    this->abort("takeoff was not successful");
                }
                return;
            }
            case Takeoff::WaitingForTakeoff:
            case Takeoff::TakingOff:
                this->timerTakeoff();
                return;
            case Takeoff::AfterTracker: {
                name.value = this->_after_takeoff_tracker_name_;
                const std::optional<bool> r = this->callStep(*this->switchTracker_, name);
                if (r) {
                    if (!*r) {
                        HYPHA_LOG_ERROR(
                            "could not switch to '{}' after takeoff: {}",
                            this->_after_takeoff_tracker_name_,
                            this->lastMessage_
                        );
                    }
                    this->enter(Takeoff::AfterController);
                }
                return;
            }
            case Takeoff::AfterController: {
                name.value = this->_after_takeoff_controller_name_;
                const std::optional<bool> r = this->callStep(*this->switchController_, name);
                if (r) {
                    if (!*r) {
                        HYPHA_LOG_ERROR(
                            "could not switch to '{}' after takeoff: {}",
                            this->_after_takeoff_controller_name_,
                            this->lastMessage_
                        );
                    }
                    HYPHA_LOG_INFO(
                        "takeoff finished: {} with {}",
                        this->_after_takeoff_tracker_name_,
                        this->_after_takeoff_controller_name_
                    );
                    this->state_ = Takeoff::Idle;
                }
                return;
            }
        }
    }

    /// timerTakeoff(): wait for the LandoffTracker to take the goal, then to reach it.
    void timerTakeoff()
    {
        if (!this->control_manager_diagnostics.has()) {
            return;
        }
        const mrs_msgs::ControlManagerDiagnostics& diag = *this->control_manager_diagnostics;
        if (this->state_ == Takeoff::WaitingForTakeoff) {
            if (diag.active_tracker == this->_takeoff_tracker_name_
                && diag.tracker_status.have_goal) {
                this->state_ = Takeoff::TakingOff;
            }
            return;
        }
        if (diag.active_tracker != this->_takeoff_tracker_name_ || !diag.tracker_status.have_goal) {
            this->enter(Takeoff::AfterTracker);
        }
    }

    /// timerDiagnostics(), without the GPS fields.
    void publishDiagnostics()
    {
        mrs_msgs::UavManagerDiagnostics diag;
        diag.stamp = ros::Time::now();
        diag.uav_name = this->_uav_name_;
        switch (this->state_) {
            case Takeoff::Idle:
                diag.state = this->number_of_takeoffs_ > 0
                               ? mrs_msgs::UavManagerDiagnostics::STATE_FLYING
                               : mrs_msgs::UavManagerDiagnostics::STATE_IDLING;
                break;
            default:
                diag.state = mrs_msgs::UavManagerDiagnostics::STATE_TAKING_OFF;
                break;
        }
        this->diagnostics.send(diag);
    }

    std::unique_ptr<mrs::PluginHost>                 host_;
    std::unique_ptr<mrs::Request<mrs_msgs::String>>  switchController_;
    std::unique_ptr<mrs::Request<mrs_msgs::String>>  switchTracker_;
    std::unique_ptr<mrs::Request<mrs_msgs::Vec1>>    takeoffCall_;
    std::unique_ptr<mrs::Request<std_srvs::SetBool>> toggleOutput_;
    std::unique_ptr<mrs::Request<std_srvs::SetBool>> arming_;
    std::unique_ptr<mrs::Serve<std_srvs::Trigger>>   takeoffServer_;
    ros::Timer                                       diagnosticsTimer_;

    std::string _uav_name_;
    std::string _null_tracker_name_;
    std::string _after_takeoff_tracker_name_;
    std::string _after_takeoff_controller_name_;
    std::string _takeoff_controller_name_;
    std::string _takeoff_tracker_name_;
    double      _takeoff_height_{0.0};
    bool        _gain_manager_required_{true};
    bool        _constraint_manager_required_{true};
    double      _diagnostics_rate_{1.0};

    Takeoff     state_{Takeoff::Idle};
    ros::Time   entered_;
    bool        sent_{false};
    uint64_t    ticket_{0};
    std::string lastMessage_;
    int         number_of_takeoffs_{0};
};

}  // namespace

int main(int argc, char** argv)
{
    return hypha::runNode<UavManager>(argc, argv, "uav1_uav_manager");
}
