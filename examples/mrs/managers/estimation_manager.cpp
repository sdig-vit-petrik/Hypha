/// estimation_manager: upstream MRS EstimationManager (onInit, timerPublish,
/// timerPublishDiagnostics, timerCheckHealth) as a native hypha CycleNode, cut to one
/// state estimator, ground_truth, which runs inside it as upstream plugin code.
#include <hypha/hypha.h>
#include <hypha_log/hypha_log.h>

#include <mrs_host/plugin_host.h>
#include <mrs_lib/param_loader.h>
#include <mrs_lib/transformer.h>
#include <mrs_msgs/EstimationDiagnostics.h>
#include <mrs_msgs/HwApiCapabilities.h>
#include <mrs_msgs/UavState.h>
#include <mrs_uav_managers/state_estimator.h>
#include <nav_msgs/Odometry.h>
#include <pluginlib/class_list_macros.h>
#include <ros/node_handle.h>

#include <memory>
#include <string>
#include <vector>

HYPHA_LOG_MODULE_INIT("estimation_manager")

namespace {

using mrs_uav_managers::StateEstimator;
using mrs_uav_managers::estimation_manager::CommonHandlers_t;
using mrs_uav_managers::estimation_manager::PrivateHandlers_t;
using mrs_uav_managers::estimation_manager::Support;

/// The ported subset of the upstream StateMachine.
enum class SmState { Uninitialized, Initialized, ReadyForFlight };

const char* toString(SmState state)
{
    switch (state) {
        case SmState::Uninitialized:
            return "UNINITIALIZED_STATE";
        case SmState::Initialized:
            return "INITIALIZED_STATE";
        case SmState::ReadyForFlight:
            return "READY_FOR_FLIGHT_STATE";
    }
    return "UNKNOWN";
}

class EstimationManager final : public hypha::CycleNode
{
  public:
    /// The trigger: one step per simulator sample. silence_ms in its table.
    hypha::Arrivals<nav_msgs::Odometry>        ground_truth{*this, "ground_truth"};
    hypha::Latest<mrs_msgs::HwApiCapabilities> hw_api_capabilities{
        *this, "hw_api_capabilities", unsupervised
    };
    hypha::Output<mrs_msgs::UavState>              uav_state{*this, "uav_state"};
    hypha::Output<nav_msgs::Odometry>              odom_main{*this, "odom_main"};
    hypha::Output<mrs_msgs::EstimationDiagnostics> diagnostics{*this, "diagnostics"};

    void setup(hypha::Node&) override
    {
        const std::string uav =
            hypha::config().required<std::string>("uav_name", hypha::constraints::nonempty());
        this->nodeName_ = uav + "_estimation_manager";
        this->host_ = std::make_unique<mrs::PluginHost>(
            *this, this->nodeName_, "/" + uav + "/estimation_manager"
        );
        this->host_->adoptArrivals("/" + uav + "/hw_api/ground_truth", this->ground_truth);
        this->nh_ = ros::NodeHandle(*this->host_, "~");

        // timerInitialization() of upstream, cut to the ported subset.
        mrs_lib::ParamLoader param_loader(this->nh_, "EstimationManager");
        this->ch_ = std::make_shared<CommonHandlers_t>();
        this->ch_->nodelet_name = "EstimationManager";
        this->ch_->package_name = "mrs_uav_managers";
        param_loader.loadParam("uav_name", this->ch_->uav_name);
        param_loader.setPrefix(
            this->ch_->package_name + "/" + Support::toSnakeCase(this->ch_->nodelet_name) + "/"
        );
        param_loader.loadParam("frame_id/fcu", this->ch_->frames.fcu);
        this->ch_->frames.ns_fcu = this->ch_->uav_name + "/" + this->ch_->frames.fcu;
        param_loader.loadParam("frame_id/fcu_untilted", this->ch_->frames.fcu_untilted);
        this->ch_->frames.ns_fcu_untilted =
            this->ch_->uav_name + "/" + this->ch_->frames.fcu_untilted;
        param_loader.loadParam("frame_id/rtk_antenna", this->ch_->frames.rtk_antenna);
        this->ch_->frames.ns_rtk_antenna =
            this->ch_->uav_name + "/" + this->ch_->frames.rtk_antenna;
        // hypha port: no TF input. /tf comes from transform_manager, which steps on this
        // node's odom_main, so reading it would close a loop of triggered cycles that holds
        // until the clock driver times out.
        this->ch_->transformer = std::make_shared<mrs_lib::Transformer>("EstimationManager");
        this->ch_->transformer->retryLookupNewest(true);
        param_loader.loadParam("rate/diagnostics", this->ch_->desired_diagnostics_rate);
        // hypha port: upstream reads the rate from control_manager's diagnostics.
        param_loader.loadParam("rate/uav_state", this->ch_->desired_uav_state_rate);
        param_loader.loadParam("debug_topics/input", this->ch_->debug_topics.input);
        param_loader.loadParam("debug_topics/output", this->ch_->debug_topics.output);
        param_loader.loadParam("debug_topics/state", this->ch_->debug_topics.state);
        param_loader.loadParam("debug_topics/covariance", this->ch_->debug_topics.covariance);
        param_loader.loadParam("debug_topics/innovation", this->ch_->debug_topics.innovation);
        param_loader.loadParam("debug_topics/diagnostics", this->ch_->debug_topics.diag);
        param_loader.loadParam("debug_topics/correction", this->ch_->debug_topics.correction);
        param_loader.loadParam("debug_topics/correction_delay", this->ch_->debug_topics.corr_delay);

        param_loader.loadParam("state_estimators", this->estimatorNames_);
        param_loader.loadParam("initial_state_estimator", this->initialName_);
        if (this->estimatorNames_.size() != 1
            || this->estimatorNames_.front() != this->initialName_) {
            throw std::runtime_error(
                "estimation_manager: the port runs exactly one state estimator, the initial one"
            );
        }
        std::string address;
        param_loader.loadParam(this->initialName_ + "/address", address);
        if (!param_loader.loadedSuccessfully()) {
            throw std::runtime_error("estimation_manager: could not load all parameters");
        }
        this->estimator_ = mrs::PluginRegistry<StateEstimator>::create(address);

        auto ph = std::make_shared<PrivateHandlers_t>();
        ph->loadConfigFile = [](const std::string&) {
            return true;
        };  // the TOML holds every layer
        ph->param_loader = std::make_unique<mrs_lib::ParamLoader>(
            ros::NodeHandle(this->nh_, this->estimator_->getName()),
            "EstimationManager/" + this->estimator_->getName()
        );
        this->estimator_->initialize(this->nh_, this->ch_, ph);
        if (!ph->param_loader->loadedSuccessfully()) {
            throw std::runtime_error(
                "estimation_manager: estimator '" + this->initialName_
                + "' could not load its parameters"
            );
        }
        // The hw_api check runs on the first capabilities sample; here it reads its
        // parameters once against a hw_api that provides everything, so start-up
        // checks them like every other key.
        mrs_msgs::HwApiCapabilities everything;
        everything.produces_gnss = everything.produces_imu = everything.produces_distance_sensor =
            true;
        everything.produces_altitude = everything.produces_magnetometer_heading =
            everything.produces_position = true;
        everything.produces_orientation = everything.produces_velocity =
            everything.produces_angular_velocity = true;
        everything.produces_ground_truth = everything.produces_rtk = everything.produces_odometry =
            true;
        static_cast<void>(this->estimator_->isCompatibleWithHwApi(
            std::make_shared<const mrs_msgs::HwApiCapabilities>(everything)
        ));

        this->diagnosticsTimer_ = this->nh_.createTimer(
            ros::Rate(this->ch_->desired_diagnostics_rate), [this](const ros::TimerEvent&) {
                this->publishDiagnostics();
            }
        );
        this->setTrigger(this->ground_truth);
    }

    void step(const hypha::CycleTime& time) override
    {
        if (this->ground_truth.samples().empty()) {
            // Fallback: the simulator is silent. No new estimate is published, but the
            // plugin and diagnostics timers keep their time.
            if (this->healthy_) {
                HYPHA_LOG_ERROR(
                    "{}: no hw_api/ground_truth for the silence limit: the estimate is stale",
                    this->nodeName_
                );
            }
            this->healthy_ = false;
            this->host_->step(time);
            return;
        }
        if (!this->healthy_ && this->sm_ != SmState::Uninitialized) {
            HYPHA_LOG_INFO("{}: hw_api/ground_truth is back", this->nodeName_);
        }
        this->healthy_ = true;
        this->host_->step(time, [this] {
            this->checkHealth();
            this->publish();
        });
    }

  private:
    /// The hw_api check of upstream timerInitialization(), over steps instead of a sleep loop.
    void checkHardware()
    {
        if (!this->hw_api_capabilities.has()) {
            ROS_INFO_THROTTLE(
                1.0,
                "[%s]: %s hw_api capabilities",
                this->nodeName_.c_str(),
                Support::waiting_for_string.c_str()
            );
            return;
        }
        const auto capabilities =
            std::make_shared<const mrs_msgs::HwApiCapabilities>(*this->hw_api_capabilities);
        if (!this->estimator_->isCompatibleWithHwApi(capabilities)) {
            this->failRun(
                "estimator '" + this->initialName_ + "' is not compatible with the hw_api"
            );
            return;
        }
        this->sm_ = SmState::Initialized;
        HYPHA_LOG_INFO("{}: initialized", this->nodeName_);
    }

    /// timerCheckHealth() of upstream, cut to one estimator.
    void checkHealth()
    {
        if (this->sm_ == SmState::Uninitialized) {
            this->checkHardware();
            return;
        }
        if (this->estimator_->isReady()) {
            this->estimator_->start();
        }
        if (this->sm_ == SmState::Initialized && this->estimator_->isRunning()) {
            this->estimator_->setActive(true);
            this->sm_ = SmState::ReadyForFlight;
            HYPHA_LOG_INFO(
                "{}: {} is running: {}", this->nodeName_, this->initialName_, toString(this->sm_)
            );
        }
    }

    /// timerPublish() of upstream.
    void publish()
    {
        if (this->sm_ != SmState::ReadyForFlight) {
            return;
        }
        const std::optional<mrs_msgs::UavState> state = this->estimator_->getUavState();
        if (!state.has_value() || !Support::noNans(state->pose.orientation)) {
            return;
        }
        mrs_msgs::UavState uav_state = *state;
        uav_state.estimator_iteration = this->estimatorSwitchCount_;
        this->uav_state.send(uav_state);

        nav_msgs::Odometry        odom_main = Support::uavStateToOdom(uav_state);
        const std::vector<double> pose_covariance = this->estimator_->getPoseCovariance();
        for (size_t i = 0; i < pose_covariance.size() && i < odom_main.pose.covariance.size();
             i++) {
            odom_main.pose.covariance[i] = pose_covariance[i];
        }
        const std::vector<double> twist_covariance = this->estimator_->getTwistCovariance();
        for (size_t i = 0; i < twist_covariance.size() && i < odom_main.twist.covariance.size();
             i++) {
            odom_main.twist.covariance[i] = twist_covariance[i];
        }
        this->odom_main.send(odom_main);
        this->last_ = uav_state;
    }

    /// timerPublishDiagnostics() of upstream, without the AGL estimator.
    void publishDiagnostics()
    {
        if (this->sm_ != SmState::ReadyForFlight || !this->last_.has_value() || !this->healthy_) {
            return;
        }
        const mrs_msgs::UavState&       uav_state = *this->last_;
        mrs_msgs::EstimationDiagnostics diag;
        diag.header.stamp = uav_state.header.stamp;
        diag.header.frame_id = this->estimator_->getFrameId();
        diag.child_frame_id = uav_state.child_frame_id;
        diag.pose = uav_state.pose;
        diag.velocity = uav_state.velocity;
        diag.acceleration = uav_state.acceleration;
        diag.sm_state = toString(this->sm_);
        diag.max_flight_z = this->estimator_->getMaxFlightZ();
        diag.estimator_iteration = this->estimatorSwitchCount_;
        diag.estimation_rate = this->ch_->desired_uav_state_rate;
        diag.running_state_estimators = this->estimatorNames_;
        diag.switchable_state_estimators = {};
        diag.current_state_estimator = this->estimator_->getName();
        diag.estimator_horizontal = uav_state.estimator_horizontal;
        diag.estimator_vertical = uav_state.estimator_vertical;
        diag.estimator_heading = uav_state.estimator_heading;
        diag.estimator_agl_height = "NOT_USED";
        diag.agl_height = std::nanf("");
        this->diagnostics.send(diag);
    }

    std::string                       nodeName_;
    std::unique_ptr<mrs::PluginHost>  host_;
    ros::NodeHandle                   nh_;
    std::shared_ptr<CommonHandlers_t> ch_;
    std::unique_ptr<StateEstimator>   estimator_;
    std::vector<std::string>          estimatorNames_;
    std::string                       initialName_;
    ros::Timer                        diagnosticsTimer_;
    SmState                           sm_{SmState::Uninitialized};
    bool                              healthy_{true};
    int                               estimatorSwitchCount_{0};
    std::optional<mrs_msgs::UavState> last_;
};

}  // namespace

int main(int argc, char** argv)
{
    return hypha::runNode<EstimationManager>(argc, argv, "uav1_estimation_manager");
}
