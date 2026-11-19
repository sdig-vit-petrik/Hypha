/// control_manager: upstream MRS ControlManager as a native hypha CycleNode, cut to the
/// paths takeoff and goto use. The cycle is triggered by uav_state; a step without uav_state
/// is upstream's timeoutUavState: the failsafe controller takes over and keeps commanding.
#include <hypha/hypha.h>
#include <hypha_log/hypha_log.h>

#include <mrs_host/plugin_host.h>
#include <mrs_host/service.h>
#include <mrs_lib/attitude_converter.h>
#include <mrs_lib/param_loader.h>
#include <mrs_lib/safety_zone/safety_zone.h>
#include <mrs_lib/transformer.h>
#include <mrs_msgs/ControlManagerDiagnostics.h>
#include <mrs_msgs/ControllerDiagnostics.h>
#include <mrs_msgs/HwApiAttitudeRateCmd.h>
#include <mrs_msgs/HwApiCapabilities.h>
#include <mrs_msgs/HwApiStatus.h>
#include <mrs_msgs/String.h>
#include <mrs_msgs/TrackerCommand.h>
#include <mrs_msgs/UavState.h>
#include <mrs_msgs/Vec1.h>
#include <mrs_msgs/Vec4.h>
#include <mrs_uav_managers/control_manager/common.h>
#include <mrs_uav_managers/controller.h>
#include <mrs_uav_managers/tracker.h>
#include <pluginlib/class_list_macros.h>
#include <ros/node_handle.h>
#include <std_srvs/SetBool.h>
#include <std_srvs/Trigger.h>

#include <map>
#include <memory>
#include <optional>
#include <string>
#include <tuple>
#include <variant>
#include <vector>

HYPHA_LOG_MODULE_INIT("control_manager")

namespace {

using mrs_uav_managers::Controller;
using mrs_uav_managers::Tracker;
using namespace mrs_uav_managers::control_manager;

struct TrackerParams
{
    std::string address;
    std::string name_space;
    bool        human_switchable{false};
};

struct ControllerParams
{
    std::string               address;
    std::string               name_space;
    double                    eland_threshold{0.0};
    double                    failsafe_threshold{0.0};
    double                    odometry_innovation_threshold{0.0};
    bool                      human_switchable{false};
    ControlOutputModalities_t outputs;
};

class ControlManager final : public hypha::CycleNode
{
  public:
    /// The trigger: one step per uav_state. silence_ms = safety/odometry_max_missing_time.
    hypha::Arrivals<mrs_msgs::UavState>        uav_state{*this, "uav_state"};
    hypha::Latest<mrs_msgs::HwApiStatus>       hw_api_status{*this, "hw_api_status", unsupervised};
    hypha::Latest<mrs_msgs::HwApiCapabilities> hw_api_capabilities{
        *this, "hw_api_capabilities", unsupervised
    };
    hypha::Output<mrs_msgs::HwApiAttitudeRateCmd> attitude_rate_cmd{*this, "attitude_rate_cmd"};
    hypha::Output<mrs_msgs::ControlManagerDiagnostics> diagnostics{*this, "diagnostics"};
    hypha::Output<mrs_msgs::TrackerCommand>            tracker_cmd{*this, "tracker_cmd"};
    hypha::Output<mrs_msgs::ControllerDiagnostics>     controller_diagnostics{
        *this, "controller_diagnostics"
    };

    void setup(hypha::Node&) override;
    void step(const hypha::CycleTime& time) override;

  private:
    void loadParameters();
    void loadPlugins(mrs_lib::ParamLoader& param_loader);
    void advertiseServices();
    void checkHwApi();
    void control(const mrs_msgs::UavState& state);
    void silent();
    void updateHwApiStatus();

    std::tuple<bool, std::string> switchTracker(const std::string& tracker_name);
    std::tuple<bool, std::string> switchController(const std::string& controller_name);
    std::tuple<bool, std::string> setReference(const mrs_msgs::ReferenceStamped& reference_in);
    bool                          isPointInSafetyArea2d(const mrs_msgs::ReferenceStamped& point);
    bool                          isPointInSafetyArea3d(const mrs_msgs::ReferenceStamped& point);
    bool                          isPathToPointInSafetyArea3d(
        const mrs_msgs::ReferenceStamped& start, const mrs_msgs::ReferenceStamped& end
    );
    double                        getMinZ(const std::string& frame_id);
    double                        getMaxZ(const std::string& frame_id);
    std::tuple<bool, std::string> eland();
    std::tuple<bool, std::string> failsafe();
    void                          toggleOutput(bool input);
    void                          updateTrackers();
    void                          updateControllers(const mrs_msgs::UavState& uav_state);
    void                          publish();
    void                          publishDiagnostics();
    void                          initializeControlOutput();
    bool                          isFlyingNormally() const;
    double                        getMass() const;

    std::string                                    nodeName_;
    std::unique_ptr<mrs::PluginHost>               host_;
    ros::NodeHandle                                nh_;
    std::unique_ptr<mrs::Serve<std_srvs::SetBool>> service_toggle_output_;
    std::unique_ptr<mrs::Serve<mrs_msgs::String>>  service_switch_tracker_;
    std::unique_ptr<mrs::Serve<mrs_msgs::String>>  service_switch_controller_;
    std::unique_ptr<mrs::Serve<mrs_msgs::Vec4>>    service_goto_;

    std::string                             _uav_name_;
    std::string                             _body_frame_;
    bool                                    _profiler_enabled_{false};
    double                                  _uav_mass_{0.0};
    double                                  _initial_body_disturbance_x_{0.0};
    double                                  _initial_body_disturbance_y_{0.0};
    int                                     _state_input_{-1};  ///< 0: uav_state (INPUT_UAV_STATE)
    double                                  _min_throttle_null_tracker_{0.0};
    std::string                             _ehover_tracker_name_;
    std::string                             _failsafe_controller_name_;
    std::string                             _eland_controller_name_;
    double                                  _uav_state_max_missing_time_{0.0};
    std::string                             _tracker_error_action_;
    std::string                             _null_tracker_name_;
    std::string                             _landoff_tracker_name_;
    std::vector<std::string>                _tracker_names_;
    std::vector<std::string>                _controller_names_;
    std::map<std::string, TrackerParams>    trackers_;
    std::map<std::string, ControllerParams> controllers_;
    ControlOutputModalities_t               _hw_api_inputs_;  ///< the configured output mode
    double                                  desired_uav_state_rate_{100.0};

    std::shared_ptr<CommonHandlers_t>        common_handlers_;
    std::shared_ptr<mrs_lib::Transformer>    transformer_;
    std::unique_ptr<mrs_lib::SafetyZone>     safety_zone_;
    bool                                     use_safety_area_{false};
    std::string                              _safety_area_horizontal_frame_;
    std::string                              _safety_area_vertical_frame_;
    double                                   _safety_area_min_z_{0.0};
    double                                   _safety_area_max_z_{0.0};
    std::vector<std::unique_ptr<Tracker>>    tracker_list_;
    std::vector<std::unique_ptr<Controller>> controller_list_;
    unsigned int _null_tracker_idx_{0}, _landoff_tracker_idx_{0}, _ehover_tracker_idx_{0};
    unsigned int _failsafe_controller_idx_{0}, _eland_controller_idx_{0};
    unsigned int active_tracker_idx_{0}, active_controller_idx_{0};
    mrs_msgs::DynamicsConstraintsSrvRequest current_constraints_;
    std::optional<mrs_msgs::TrackerCommand> last_tracker_cmd_;
    Controller::ControlOutput               last_control_output_;
    mrs_msgs::UavState                      uav_state_;
    bool                                    got_uav_state_{false};
    bool                                    hw_api_checked_{false};
    bool                                    is_initialized_{false};
    bool                                    output_enabled_{false};
    bool                                    callbacks_enabled_{true};
    bool                                    offboard_mode_{false};
    bool                                    armed_{false};
    bool                                    failsafe_triggered_{false};
    bool                                    eland_triggered_{false};
};

void ControlManager::setup(hypha::Node&)
{
    const std::string uav =
        hypha::config().required<std::string>("uav_name", hypha::constraints::nonempty());
    this->nodeName_ = uav + "_control_manager";
    this->host_ =
        std::make_unique<mrs::PluginHost>(*this, this->nodeName_, "/" + uav + "/control_manager");
    this->nh_ = ros::NodeHandle(*this->host_, "~");
    this->loadParameters();
    this->advertiseServices();
    this->setTrigger(this->uav_state);
}

void ControlManager::loadParameters()
{
    this->common_handlers_ = std::make_shared<CommonHandlers_t>();
    mrs_lib::ParamLoader param_loader(this->nh_, "ControlManager");

    param_loader.loadParam("uav_name", this->_uav_name_);
    param_loader.loadParam("body_frame", this->_body_frame_);
    param_loader.loadParam("enable_profiler", this->_profiler_enabled_);
    param_loader.loadParam("uav_mass", this->_uav_mass_);
    param_loader.loadParam("body_disturbance_x", this->_initial_body_disturbance_x_);
    param_loader.loadParam("body_disturbance_y", this->_initial_body_disturbance_y_);
    param_loader.loadParam("g", this->common_handlers_->g);
    this->common_handlers_->throttle_model.initialize(param_loader);

    param_loader.loadParam("safety_area/enabled", this->use_safety_area_);
    param_loader.loadParam(
        "safety_area/horizontal/frame_name", this->_safety_area_horizontal_frame_
    );
    param_loader.loadParam("safety_area/vertical/frame_name", this->_safety_area_vertical_frame_);
    param_loader.loadParam("safety_area/vertical/max_z", this->_safety_area_max_z_);
    param_loader.loadParam("safety_area/vertical/min_z", this->_safety_area_min_z_);
    if (this->use_safety_area_) {
        const Eigen::MatrixXd border_points =
            param_loader.loadMatrixDynamic2("safety_area/horizontal/points", -1, 2);
        try {
            this->safety_zone_ = std::make_unique<mrs_lib::SafetyZone>(border_points);
        } catch (const std::exception& e) {
            // upstream: ROS_ERROR and ros::shutdown()
            throw std::runtime_error(
                std::string(
                    "control_manager: SafetyArea: wrong configuration for the safety "
                    "zone border polygon: "
                )
                + e.what()
            );
        }
        ROS_INFO("[ControlManager]: safety area initialized");
    }

    // hypha port: the output mode is configuration, checked against the first
    // hw_api/capabilities sample (upstream derives it from that sample at run time).
    param_loader.loadParam("hw_api_inputs/attitude_rate", this->_hw_api_inputs_.attitude_rate);
    if (!this->_hw_api_inputs_.attitude_rate) {
        throw std::runtime_error(
            "control_manager: the port supports the attitude_rate output mode only"
        );
    }

    param_loader.setPrefix("mrs_uav_managers/control_manager/");
    param_loader.loadParam("state_input", this->_state_input_);
    if (this->_state_input_ != 0) {
        throw std::runtime_error("control_manager: state_input must be 0 (uav_state)");
    }
    param_loader.loadParam("safety/min_throttle_null_tracker", this->_min_throttle_null_tracker_);
    param_loader.loadParam("safety/ehover_tracker", this->_ehover_tracker_name_);
    param_loader.loadParam("safety/failsafe_controller", this->_failsafe_controller_name_);
    param_loader.loadParam("safety/eland/controller", this->_eland_controller_name_);
    param_loader.loadParam("safety/odometry_max_missing_time", this->_uav_state_max_missing_time_);
    param_loader.loadParam("safety/tracker_error_action", this->_tracker_error_action_);

    auto& c = this->current_constraints_.constraints;
    param_loader.loadParam("default_constraints/horizontal/speed", c.horizontal_speed);
    param_loader.loadParam(
        "default_constraints/horizontal/acceleration", c.horizontal_acceleration
    );
    param_loader.loadParam("default_constraints/horizontal/jerk", c.horizontal_jerk);
    param_loader.loadParam("default_constraints/horizontal/snap", c.horizontal_snap);
    param_loader.loadParam(
        "default_constraints/vertical/ascending/speed", c.vertical_ascending_speed
    );
    param_loader.loadParam(
        "default_constraints/vertical/ascending/acceleration", c.vertical_ascending_acceleration
    );
    param_loader.loadParam(
        "default_constraints/vertical/ascending/jerk", c.vertical_ascending_jerk
    );
    param_loader.loadParam(
        "default_constraints/vertical/ascending/snap", c.vertical_ascending_snap
    );
    param_loader.loadParam(
        "default_constraints/vertical/descending/speed", c.vertical_descending_speed
    );
    param_loader.loadParam(
        "default_constraints/vertical/descending/acceleration", c.vertical_descending_acceleration
    );
    param_loader.loadParam(
        "default_constraints/vertical/descending/jerk", c.vertical_descending_jerk
    );
    param_loader.loadParam(
        "default_constraints/vertical/descending/snap", c.vertical_descending_snap
    );
    param_loader.loadParam("default_constraints/heading/speed", c.heading_speed);
    param_loader.loadParam("default_constraints/heading/acceleration", c.heading_acceleration);
    param_loader.loadParam("default_constraints/heading/jerk", c.heading_jerk);
    param_loader.loadParam("default_constraints/heading/snap", c.heading_snap);
    param_loader.loadParam("default_constraints/angular_speed/roll", c.roll_rate);
    param_loader.loadParam("default_constraints/angular_speed/pitch", c.pitch_rate);
    param_loader.loadParam("default_constraints/angular_speed/yaw", c.yaw_rate);
    param_loader.loadParam("default_constraints/tilt", c.tilt);
    c.tilt = M_PI * (c.tilt / 180.0);

    this->initializeControlOutput();
    this->transformer_ = std::make_shared<mrs_lib::Transformer>(this->nh_, "ControlManager");
    this->transformer_->setDefaultPrefix(this->_uav_name_);
    this->transformer_->retryLookupNewest(true);
    this->common_handlers_->transformer = this->transformer_;
    this->common_handlers_->safety_area.use_safety_area = this->use_safety_area_;
    this->common_handlers_->safety_area.isPointInSafetyArea2d =
        [this](const mrs_msgs::ReferenceStamped& point) {
            return this->isPointInSafetyArea2d(point);
        };
    this->common_handlers_->safety_area.isPointInSafetyArea3d =
        [this](const mrs_msgs::ReferenceStamped& point) {
            return this->isPointInSafetyArea3d(point);
        };
    this->common_handlers_->safety_area.getMinZ = [this](const std::string& frame_id) {
        return this->getMinZ(frame_id);
    };
    this->common_handlers_->safety_area.getMaxZ = [this](const std::string& frame_id) {
        return this->getMaxZ(frame_id);
    };
    this->common_handlers_->getMass = [this] {
        return this->getMass();
    };
    // hypha port: detailed model params only serve the actuators/control_group outputs.
    this->common_handlers_->detailed_model_params = std::nullopt;
    this->common_handlers_->control_output_modalities = this->_hw_api_inputs_;
    this->common_handlers_->uav_name = this->_uav_name_;
    this->common_handlers_->parent_nh = this->nh_;
    this->common_handlers_->scope_timer.enabled = false;

    this->loadPlugins(param_loader);
    if (!param_loader.loadedSuccessfully()) {
        throw std::runtime_error("control_manager: could not load all parameters");
    }
}

void ControlManager::loadPlugins(mrs_lib::ParamLoader& param_loader)
{
    std::vector<std::string> custom;
    param_loader.loadParam("mrs_trackers", this->_tracker_names_);
    param_loader.loadParam("trackers", custom);
    this->_tracker_names_.insert(this->_tracker_names_.end(), custom.begin(), custom.end());
    param_loader.loadParam("null_tracker", this->_null_tracker_name_);
    param_loader.loadParam("landing_takeoff_tracker", this->_landoff_tracker_name_);
    for (const std::string& name : this->_tracker_names_) {
        TrackerParams params;
        param_loader.loadParam(name + "/address", params.address);
        param_loader.loadParam(name + "/namespace", params.name_space);
        param_loader.loadParam(name + "/human_switchable", params.human_switchable, false);
        this->trackers_.emplace(name, params);
        this->tracker_list_.push_back(mrs::PluginRegistry<Tracker>::create(params.address));
    }
    for (size_t i = 0; i < this->tracker_list_.size(); i++) {
        const TrackerParams& params = this->trackers_.at(this->_tracker_names_.at(i));
        auto                 ph = std::make_shared<PrivateHandlers_t>();
        ph->loadConfigFile = [](const std::string&) {
            return true;
        };  // the TOML holds every layer
        ph->name_space = params.name_space;
        ph->runtime_name = this->_tracker_names_.at(i);
        ph->param_loader = std::make_unique<mrs_lib::ParamLoader>(
            ros::NodeHandle(this->nh_, params.name_space), ph->runtime_name
        );
        if (!this->tracker_list_.at(i)->initialize(
                ros::NodeHandle(this->nh_, params.name_space), this->common_handlers_, ph
            )) {
            throw std::runtime_error(
                "control_manager: tracker '" + ph->runtime_name + "' failed to initialize"
            );
        }
    }
    const auto index =
        [](const std::string& name, const std::vector<std::string>& names, const char* what) {
            const std::optional<unsigned int> idx = idxInVector(name, names);
            if (!idx) {
                throw std::runtime_error(
                    std::string("control_manager: the ") + what + " '" + name + "' is not loaded"
                );
            }
            return *idx;
        };
    this->_ehover_tracker_idx_ =
        index(this->_ehover_tracker_name_, this->_tracker_names_, "ehover tracker");
    this->_landoff_tracker_idx_ =
        index(this->_landoff_tracker_name_, this->_tracker_names_, "landoff tracker");
    this->_null_tracker_idx_ =
        index(this->_null_tracker_name_, this->_tracker_names_, "null tracker");

    custom.clear();
    param_loader.loadParam("mrs_controllers", this->_controller_names_);
    param_loader.loadParam("controllers", custom);
    this->_controller_names_.insert(this->_controller_names_.end(), custom.begin(), custom.end());
    for (const std::string& name : this->_controller_names_) {
        ControllerParams params;
        param_loader.loadParam(name + "/address", params.address);
        param_loader.loadParam(name + "/namespace", params.name_space);
        param_loader.loadParam(name + "/eland_threshold", params.eland_threshold);
        param_loader.loadParam(name + "/failsafe_threshold", params.failsafe_threshold);
        param_loader.loadParam(
            name + "/odometry_innovation_threshold", params.odometry_innovation_threshold
        );
        param_loader.loadParam(name + "/human_switchable", params.human_switchable, false);
        param_loader.loadParam(
            name + "/outputs/attitude_rate", params.outputs.attitude_rate, false
        );
        if (!(this->_hw_api_inputs_.attitude_rate && params.outputs.attitude_rate)) {
            throw std::runtime_error(
                "control_manager: controller '" + name + "' cannot command the attitude_rate output"
            );
        }
        params.eland_threshold = params.eland_threshold == 0 ? 1e6 : params.eland_threshold;
        params.failsafe_threshold =
            params.failsafe_threshold == 0 ? 1e6 : params.failsafe_threshold;
        params.odometry_innovation_threshold =
            params.odometry_innovation_threshold == 0 ? 1e6 : params.odometry_innovation_threshold;
        this->controllers_.emplace(name, params);
        this->controller_list_.push_back(mrs::PluginRegistry<Controller>::create(params.address));
    }
    for (size_t i = 0; i < this->controller_list_.size(); i++) {
        const ControllerParams& params = this->controllers_.at(this->_controller_names_.at(i));
        auto                    ph = std::make_shared<PrivateHandlers_t>();
        ph->loadConfigFile = [](const std::string&) {
            return true;
        };
        ph->name_space = params.name_space;
        ph->runtime_name = this->_controller_names_.at(i);
        ph->param_loader = std::make_unique<mrs_lib::ParamLoader>(
            ros::NodeHandle(this->nh_, params.name_space), ph->runtime_name
        );
        if (!this->controller_list_.at(i)->initialize(
                ros::NodeHandle(this->nh_, params.name_space), this->common_handlers_, ph
            )) {
            throw std::runtime_error(
                "control_manager: controller '" + ph->runtime_name + "' failed to initialize"
            );
        }
    }
    this->_failsafe_controller_idx_ =
        index(this->_failsafe_controller_name_, this->_controller_names_, "failsafe controller");
    this->_eland_controller_idx_ =
        index(this->_eland_controller_name_, this->_controller_names_, "eland controller");
}

void ControlManager::advertiseServices()
{
    const std::string ns = "/" + this->_uav_name_ + "/control_manager/";
    this->service_toggle_output_ = std::make_unique<mrs::Serve<std_srvs::SetBool>>(
        *this->host_,
        ns + "toggle_output",
        [this](uint64_t, const std_srvs::SetBool::Request& req) {
            // callbackToggleOutput()
            std_srvs::SetBool::Response res;
            if (!this->is_initialized_) {
                res.message = "not initialized";
                return std::optional{res};
            }
            if (this->failsafe_triggered_ || this->eland_triggered_) {
                res.message = "cannot toggle output, eland or failsafe active";
                return std::optional{res};
            }
            const bool prev = this->output_enabled_;
            this->toggleOutput(req.data);
            res.success = true;
            res.message = std::string("output ") + (this->output_enabled_ ? "enabled" : "disabled")
                        + (prev == this->output_enabled_ ? " (already)" : "");
            return std::optional{res};
        }
    );
    this->service_switch_tracker_ = std::make_unique<mrs::Serve<mrs_msgs::String>>(
        *this->host_,
        ns + "switch_tracker",
        [this](uint64_t, const mrs_msgs::String::Request& req) {
            mrs_msgs::String::Response res;
            if (this->failsafe_triggered_ || this->eland_triggered_) {
                res.message = "cannot switch, eland or failsafe active";
                return std::optional{res};
            }
            std::tie(res.success, res.message) = this->switchTracker(req.value);
            return std::optional{res};
        }
    );
    this->service_switch_controller_ = std::make_unique<mrs::Serve<mrs_msgs::String>>(
        *this->host_,
        ns + "switch_controller",
        [this](uint64_t, const mrs_msgs::String::Request& req) {
            mrs_msgs::String::Response res;
            if (this->failsafe_triggered_ || this->eland_triggered_) {
                res.message = "cannot switch, eland or failsafe active";
                return std::optional{res};
            }
            std::tie(res.success, res.message) = this->switchController(req.value);
            return std::optional{res};
        }
    );
    this->service_goto_ = std::make_unique<mrs::Serve<mrs_msgs::Vec4>>(
        *this->host_, ns + "goto", [this](uint64_t, const mrs_msgs::Vec4::Request& req) {
            // callbackGoto()
            mrs_msgs::Vec4::Response res;
            if (!this->is_initialized_) {
                res.message = "not initialized";
                return std::optional{res};
            }
            mrs_msgs::ReferenceStamped des_reference;
            des_reference.header.frame_id = "";
            des_reference.header.stamp = ros::Time(0);
            des_reference.reference.position.x = req.goal.at(0);
            des_reference.reference.position.y = req.goal.at(1);
            des_reference.reference.position.z = req.goal.at(2);
            des_reference.reference.heading = req.goal.at(3);
            std::tie(res.success, res.message) = this->setReference(des_reference);
            return std::optional{res};
        }
    );
}

void ControlManager::step(const hypha::CycleTime& time)
{
    this->host_->step(time, [this] {
        this->updateHwApiStatus();
        if (!this->hw_api_checked_) {
            this->checkHwApi();
        }
        if (this->uav_state.samples().empty()) {
            this->silent();
            return;
        }
        for (const mrs_msgs::UavState& sample : this->uav_state.samples()) {
            this->control(sample);
        }
    });
}

/// timerHwApiCapabilities(), as a check: the configured output mode must be accepted.
void ControlManager::checkHwApi()
{
    if (!this->hw_api_capabilities.has()) {
        ROS_INFO_THROTTLE(1.0, "[ControlManager]: waiting for hw_api/capabilities");
        return;
    }
    if (!this->hw_api_capabilities->accepts_attitude_rate_cmd) {
        this->failRun("hw_api capabilities do not match the configured output mode");
        return;
    }
    this->hw_api_checked_ = true;
    // The end of upstream initialize(): the null tracker and the eland controller are active.
    this->tracker_list_.at(this->_null_tracker_idx_)->activate(this->last_tracker_cmd_);
    this->active_tracker_idx_ = this->_null_tracker_idx_;
    this->controller_list_.at(this->_eland_controller_idx_)->activate(this->last_control_output_);
    this->active_controller_idx_ = this->_eland_controller_idx_;
    this->output_enabled_ = false;
    for (auto& tracker : this->tracker_list_) {
        tracker->setConstraints(
            std::make_shared<const mrs_msgs::DynamicsConstraintsSrvRequest>(
                this->current_constraints_
            )
        );
    }
    for (auto& controller : this->controller_list_) {
        controller->setConstraints(
            std::make_shared<const mrs_msgs::DynamicsConstraintsSrvRequest>(
                this->current_constraints_
            )
        );
    }
    this->is_initialized_ = true;
    HYPHA_LOG_INFO("{}: initialized, attitude_rate output", this->nodeName_);
}

/// callbackHwApiStatus()
void ControlManager::updateHwApiStatus()
{
    if (!this->hw_api_status.has()) {
        return;
    }
    const mrs_msgs::HwApiStatus& status = *this->hw_api_status;
    if (status.offboard != this->offboard_mode_) {
        HYPHA_LOG_INFO("{}: hw_api offboard {}", this->nodeName_, status.offboard ? "on" : "off");
    }
    if (status.armed != this->armed_) {
        HYPHA_LOG_INFO("{}: hw_api armed {}", this->nodeName_, status.armed ? "on" : "off");
    }
    this->offboard_mode_ = status.offboard;
    this->armed_ = status.armed;
}

/// callbackUavState() and asyncControl()
void ControlManager::control(const mrs_msgs::UavState& state)
{
    if (!this->is_initialized_) {
        return;
    }
    if (!validateUavState(state, "ControlManager", "uav_state")) {
        ROS_ERROR_THROTTLE(1.0, "[ControlManager]: incoming 'uav_state' is not finite!!!");
        return;
    }
    this->uav_state_ = state;
    this->transformer_->setDefaultFrame(state.header.frame_id);
    this->got_uav_state_ = true;
    if (this->failsafe_triggered_) {
        // timerFailsafe(): the failsafe controller flies on the last state.
        this->updateControllers(this->uav_state_);
        this->publish();
        return;
    }
    this->updateTrackers();
    this->updateControllers(this->uav_state_);
    this->publish();
    this->publishDiagnostics();
}

/// timeoutUavState(), then timerFailsafe() for the silence.
void ControlManager::silent()
{
    ROS_WARN_THROTTLE(
        1.0, "[ControlManager]: no uav_state for %.3f s", this->_uav_state_max_missing_time_
    );
    if (!this->is_initialized_) {
        return;
    }
    if (this->output_enabled_ && this->last_control_output_.control_output
        && !this->failsafe_triggered_) {
        ROS_ERROR("[ControlManager]: uav_state is missing: triggering failsafe");
        this->failsafe();
    }
    if (this->failsafe_triggered_) {
        this->updateControllers(this->uav_state_);
        this->publish();
    }
    this->publishDiagnostics();
}

void ControlManager::toggleOutput(const bool input)
{
    if (input == this->output_enabled_) {
        return;
    }
    ROS_INFO("[ControlManager]: switching output %s", input ? "ON" : "OFF");
    this->output_enabled_ = input;
    if (!this->output_enabled_) {
        this->switchTracker(this->_null_tracker_name_);
        this->switchController(this->_eland_controller_name_);
        for (auto& tracker : this->tracker_list_) {
            tracker->deactivate();
        }
        for (auto& controller : this->controller_list_) {
            controller->deactivate();
        }
    }
}

std::tuple<bool, std::string> ControlManager::switchTracker(const std::string& tracker_name)
{
    if (!this->got_uav_state_) {
        return {false, "the tracker can not be switched, missing UAV State!"};
    }
    const std::optional<unsigned int> new_tracker_idx =
        idxInVector(tracker_name, this->_tracker_names_);
    if (!new_tracker_idx) {
        return {false, "the tracker '" + tracker_name + "' does not exist!"};
    }
    if (*new_tracker_idx == this->active_tracker_idx_) {
        return {true, "not switching, the tracker '" + tracker_name + "' is already active!"};
    }
    auto [success, message] =
        this->tracker_list_.at(*new_tracker_idx)->activate(this->last_tracker_cmd_);
    if (!success) {
        return {false, "the tracker '" + tracker_name + "' could not be activated: " + message};
    }
    this->tracker_list_.at(this->active_tracker_idx_)->deactivate();
    if (this->active_tracker_idx_ == this->_null_tracker_idx_) {
        // From the null tracker: the active controller is activated.
        this->initializeControlOutput();
        this->controller_list_.at(this->active_controller_idx_)
            ->activate(this->last_control_output_);
    } else if (*new_tracker_idx == this->_null_tracker_idx_) {
        this->controller_list_.at(this->active_controller_idx_)->deactivate();
        this->last_tracker_cmd_ = {};
        this->initializeControlOutput();
    }
    this->active_tracker_idx_ = *new_tracker_idx;
    ROS_INFO("[ControlManager]: tracker switched to '%s'", tracker_name.c_str());
    this->publishDiagnostics();
    return {true, "the tracker '" + tracker_name + "' was activated"};
}

std::tuple<bool, std::string> ControlManager::switchController(const std::string& controller_name)
{
    if (!this->got_uav_state_) {
        return {false, "the controller can not be switched, missing UAV State!"};
    }
    const std::optional<unsigned int> new_controller_idx =
        idxInVector(controller_name, this->_controller_names_);
    if (!new_controller_idx) {
        return {false, "the controller '" + controller_name + "' does not exist!"};
    }
    if (*new_controller_idx == this->active_controller_idx_) {
        return {true, "not switching, the controller '" + controller_name + "' is already active!"};
    }
    if (!this->controller_list_.at(*new_controller_idx)->activate(this->last_control_output_)) {
        return {false, "the controller '" + controller_name + "' was not activated"};
    }
    // The tracker is reactivated so it starts from the new controller's state.
    this->tracker_list_.at(this->active_tracker_idx_)->deactivate();
    this->tracker_list_.at(this->active_tracker_idx_)->activate({});
    this->controller_list_.at(this->active_controller_idx_)->deactivate();
    this->active_controller_idx_ = *new_controller_idx;
    for (auto& controller : this->controller_list_) {
        controller->setConstraints(
            std::make_shared<const mrs_msgs::DynamicsConstraintsSrvRequest>(
                this->current_constraints_
            )
        );
    }
    ROS_INFO("[ControlManager]: controller switched to '%s'", controller_name.c_str());
    this->publishDiagnostics();
    return {true, "the controller '" + controller_name + "' was activated"};
}

std::tuple<bool, std::string>
ControlManager::setReference(const mrs_msgs::ReferenceStamped& reference_in)
{
    if (!this->callbacks_enabled_) {
        return {false, "not setting the reference, the callbacks are disabled"};
    }
    if (!validateReference(reference_in.reference, "ControlManager", "reference")) {
        return {false, "the reference is not valid"};
    }
    const std::optional<mrs_msgs::ReferenceStamped> ret =
        this->transformer_->transformSingle(reference_in, this->uav_state_.header.frame_id);
    if (!ret) {
        return {false, "the reference could not be transformed"};
    }
    const mrs_msgs::ReferenceStamped& transformed_reference = *ret;
    if (!this->isPointInSafetyArea3d(transformed_reference)) {
        const std::string message =
            "failed to set the reference, the point is outside of the safety area!";
        ROS_ERROR_THROTTLE(1.0, "[ControlManager]: %s", message.c_str());
        return {false, message};
    }
    if (this->last_tracker_cmd_) {
        mrs_msgs::ReferenceStamped from_point;
        from_point.header.frame_id = this->uav_state_.header.frame_id;
        from_point.reference.position.x = this->last_tracker_cmd_->position.x;
        from_point.reference.position.y = this->last_tracker_cmd_->position.y;
        from_point.reference.position.z = this->last_tracker_cmd_->position.z;
        if (!this->isPathToPointInSafetyArea3d(from_point, transformed_reference)) {
            const std::string message =
                "failed to set the reference, the path is going outside the safety area!";
            ROS_ERROR_THROTTLE(1.0, "[ControlManager]: %s", message.c_str());
            return {false, message};
        }
    }
    auto request = std::make_shared<mrs_msgs::ReferenceSrvRequest>();
    request->reference = ret->reference;
    const mrs_msgs::ReferenceSrvResponse::ConstPtr response =
        this->tracker_list_.at(this->active_tracker_idx_)
            ->setReference(mrs_msgs::ReferenceSrvRequest::ConstPtr(request));
    if (!response) {
        return {
            false,
            "the tracker '" + this->_tracker_names_.at(this->active_tracker_idx_)
                + "' does not implement the 'setReference()' function!"
        };
    }
    return {response->success, response->message};
}

void ControlManager::updateTrackers()
{
    std::optional<mrs_msgs::TrackerCommand> tracker_command;
    for (size_t i = 0; i < this->tracker_list_.size(); i++) {
        if (i == this->active_tracker_idx_) {
            tracker_command =
                this->tracker_list_.at(i)->update(this->uav_state_, this->last_control_output_);
        } else {
            this->tracker_list_.at(i)->update(this->uav_state_, this->last_control_output_);
        }
    }
    if (this->active_tracker_idx_ == this->_null_tracker_idx_) {
        return;
    }
    if (validateTrackerCommand(tracker_command, "ControlManager", "tracker_command")) {
        this->last_tracker_cmd_ = tracker_command;
        if (this->last_tracker_cmd_->header.frame_id.empty()) {
            this->last_tracker_cmd_->header.frame_id = this->uav_state_.header.frame_id;
        }
        if (ros::Time(this->last_tracker_cmd_->header.stamp).isZero()) {
            this->last_tracker_cmd_->header.stamp = ros::Time::now();
        }
        return;
    }
    ROS_ERROR_THROTTLE(1.0, "[ControlManager]: the active tracker gave an invalid command");
    if (this->active_tracker_idx_ == this->_ehover_tracker_idx_
        || this->_tracker_error_action_ != "eland") {
        this->failsafe();
    } else {
        this->eland();
    }
}

void ControlManager::updateControllers(const mrs_msgs::UavState& state)
{
    if (!this->last_tracker_cmd_) {
        for (auto& controller : this->controller_list_) {
            controller->updateInactive(state, this->last_tracker_cmd_);
        }
        return;
    }
    Controller::ControlOutput control_output;
    for (size_t i = 0; i < this->controller_list_.size(); i++) {
        if (i == this->active_controller_idx_) {
            control_output =
                this->controller_list_.at(i)->updateActive(state, *this->last_tracker_cmd_);
        } else {
            this->controller_list_.at(i)->updateInactive(state, this->last_tracker_cmd_);
        }
    }
    if (validateControlOutput(
            control_output, this->_hw_api_inputs_, "ControlManager", "control_output"
        )) {
        this->last_control_output_ = control_output;
        return;
    }
    if (this->controller_list_.at(this->active_controller_idx_)->getStatus().active) {
        ROS_ERROR_THROTTLE(
            1.0, "[ControlManager]: the active controller returned an invalid output"
        );
        if (this->failsafe_triggered_) {
            this->toggleOutput(false);
        } else if (this->eland_triggered_) {
            this->failsafe();
        } else {
            this->eland();
        }
    }
}

void ControlManager::publish()
{
    std::optional<Controller::HwApiOutputVariant> output;
    if (!this->output_enabled_) {
        output = std::nullopt;
    } else if (
        this->active_tracker_idx_ == this->_null_tracker_idx_
        || !this->last_control_output_.control_output
    ) {
        output = initializeDefaultOutput(
            this->_hw_api_inputs_,
            this->uav_state_,
            this->_min_throttle_null_tracker_,
            this->common_handlers_->throttle_model.n_motors
        );
        if (this->active_tracker_idx_ == this->_null_tracker_idx_) {
            this->last_control_output_.control_output = output;
        }
    } else {
        output = this->last_control_output_.control_output;
    }
    if (output) {
        const auto* cmd = std::get_if<mrs_msgs::HwApiAttitudeRateCmd>(&*output);
        if (cmd == nullptr) {
            this->failRun("control_manager: the controller output is not an attitude_rate command");
            return;
        }
        if (validateHwApiAttitudeRateCmd(*cmd, "ControlManager", "control_output")) {
            mrs_msgs::HwApiAttitudeRateCmd stamped = *cmd;
            stamped.stamp = ros::Time::now();
            this->attitude_rate_cmd.send(stamped);
        }
    }
    this->controller_diagnostics.send(this->last_control_output_.diagnostics);
    if (this->last_tracker_cmd_) {
        this->tracker_cmd.send(*this->last_tracker_cmd_);
    }
}

void ControlManager::publishDiagnostics()
{
    if (!this->is_initialized_ || this->diagnostics.sent()) {
        return;
    }
    mrs_msgs::ControlManagerDiagnostics msg;
    msg.stamp = ros::Time::now();
    msg.uav_name = this->_uav_name_;
    msg.desired_uav_state_rate = this->desired_uav_state_rate_;
    msg.output_enabled = this->output_enabled_;
    msg.flying_normally = this->isFlyingNormally();
    msg.active_tracker = this->_tracker_names_.at(this->active_tracker_idx_);
    msg.tracker_status = this->tracker_list_.at(this->active_tracker_idx_)->getStatus();
    msg.active_controller = this->_controller_names_.at(this->active_controller_idx_);
    msg.controller_status = this->controller_list_.at(this->active_controller_idx_)->getStatus();
    for (const std::string& name : this->_controller_names_) {
        if (name != this->_failsafe_controller_name_ && name != this->_eland_controller_name_) {
            msg.available_controllers.push_back(name);
            msg.human_switchable_controllers.push_back(
                this->controllers_.at(name).human_switchable
            );
        }
    }
    for (const std::string& name : this->_tracker_names_) {
        if (name != this->_null_tracker_name_) {
            msg.available_trackers.push_back(name);
            msg.human_switchable_trackers.push_back(this->trackers_.at(name).human_switchable);
        }
    }
    this->diagnostics.send(msg);
}

/// eland(): the LandoffTracker's eland, called directly (upstream calls its own service).
std::tuple<bool, std::string> ControlManager::eland()
{
    if (this->eland_triggered_ || this->failsafe_triggered_) {
        return {false, "cannot, eland or failsafe already active"};
    }
    if (this->active_tracker_idx_ == this->_null_tracker_idx_) {
        return {false, "cannot, the null tracker is active"};
    }
    this->switchController(this->_eland_controller_name_);
    this->switchTracker(this->_landoff_tracker_name_);
    std_srvs::Trigger::Request  req;
    std_srvs::Trigger::Response res;
    const std::string name = "/" + this->_uav_name_ + "/control_manager/"
                           + this->trackers_.at(this->_landoff_tracker_name_).name_space + "/eland";
    this->host_->callLocal(name, req, res);
    if (!res.success) {
        ROS_ERROR("[ControlManager]: eland failed: %s, triggering failsafe", res.message.c_str());
        return this->failsafe();
    }
    this->eland_triggered_ = true;
    this->callbacks_enabled_ = false;
    ROS_WARN("[ControlManager]: eland activated");
    return {true, res.message};
}

std::tuple<bool, std::string> ControlManager::failsafe()
{
    if (!this->is_initialized_) {
        return {false, "the ControlManager is not initialized"};
    }
    if (this->failsafe_triggered_) {
        return {false, "cannot, failsafe already triggered"};
    }
    if (this->active_tracker_idx_ == this->_null_tracker_idx_) {
        return {false, "cannot, the null tracker is active"};
    }
    if (this->_failsafe_controller_idx_ != this->active_controller_idx_) {
        this->controller_list_.at(this->_failsafe_controller_idx_)
            ->activate(this->last_control_output_);
        this->failsafe_triggered_ = true;
        this->eland_triggered_ = false;
        this->callbacks_enabled_ = false;
        this->controller_list_.at(this->active_controller_idx_)->deactivate();
        this->active_controller_idx_ = this->_failsafe_controller_idx_;
    }
    ROS_ERROR("[ControlManager]: failsafe activated");
    this->publishDiagnostics();
    return {true, "failsafe activated"};
}

void ControlManager::initializeControlOutput()
{
    Controller::ControlOutput controller_output;
    controller_output.diagnostics.total_mass = this->_uav_mass_;
    controller_output.diagnostics.mass_difference = 0.0;
    controller_output.diagnostics.disturbance_bx_b = this->_initial_body_disturbance_x_;
    controller_output.diagnostics.disturbance_by_b = this->_initial_body_disturbance_y_;
    controller_output.diagnostics.disturbance_estimator =
        std::abs(this->_initial_body_disturbance_x_) > 0.001
        || std::abs(this->_initial_body_disturbance_y_) > 0.001;
    controller_output.diagnostics.controller = "none";
    this->last_control_output_ = controller_output;
}

bool ControlManager::isFlyingNormally() const
{
    return this->callbacks_enabled_ && this->output_enabled_ && this->offboard_mode_ && this->armed_
        && ((this->active_tracker_idx_ != this->_ehover_tracker_idx_
             && this->active_controller_idx_ != this->_eland_controller_idx_
             && this->active_controller_idx_ != this->_failsafe_controller_idx_)
            || this->_controller_names_.size() == 1)
        && ((this->active_tracker_idx_ != this->_null_tracker_idx_
             && this->active_tracker_idx_ != this->_landoff_tracker_idx_)
            || this->_tracker_names_.size() == 1);
}

bool ControlManager::isPointInSafetyArea3d(const mrs_msgs::ReferenceStamped& point)
{
    if (!this->use_safety_area_) {
        return true;
    }
    const auto tfed_horizontal =
        this->transformer_->transformSingle(point, this->_safety_area_horizontal_frame_);
    if (!tfed_horizontal) {
        ROS_ERROR_THROTTLE(
            1.0,
            "[ControlManager]: SafetyArea: Could not transform the point to the safety area "
            "horizontal frame"
        );
        return false;
    }
    if (!this->safety_zone_->isPointValid(
            tfed_horizontal->reference.position.x, tfed_horizontal->reference.position.y
        )) {
        return false;
    }
    return point.reference.position.z >= this->getMinZ(point.header.frame_id)
        && point.reference.position.z <= this->getMaxZ(point.header.frame_id);
}

bool ControlManager::isPointInSafetyArea2d(const mrs_msgs::ReferenceStamped& point)
{
    if (!this->use_safety_area_) {
        return true;
    }
    const auto tfed_horizontal =
        this->transformer_->transformSingle(point, this->_safety_area_horizontal_frame_);
    if (!tfed_horizontal) {
        ROS_ERROR_THROTTLE(
            1.0,
            "[ControlManager]: SafetyArea: Could not transform the point to the safety area "
            "horizontal frame"
        );
        return false;
    }
    return this->safety_zone_->isPointValid(
        tfed_horizontal->reference.position.x, tfed_horizontal->reference.position.y
    );
}

bool ControlManager::isPathToPointInSafetyArea3d(
    const mrs_msgs::ReferenceStamped& start, const mrs_msgs::ReferenceStamped& end
)
{
    if (!this->use_safety_area_) {
        return true;
    }
    if (!this->isPointInSafetyArea3d(start) || !this->isPointInSafetyArea3d(end)) {
        return false;
    }
    const auto start_transformed =
        this->transformer_->transformSingle(start, this->_safety_area_horizontal_frame_);
    const auto end_transformed =
        this->transformer_->transformSingle(end, this->_safety_area_horizontal_frame_);
    if (!start_transformed || !end_transformed) {
        ROS_ERROR("[ControlManager]: SafetyArea: Could not transform a point of the path");
        return false;
    }
    return this->safety_zone_->isPathValid(
        start_transformed->reference.position.x,
        start_transformed->reference.position.y,
        end_transformed->reference.position.x,
        end_transformed->reference.position.y
    );
}

double ControlManager::getMinZ(const std::string& frame_id)
{
    if (!this->use_safety_area_) {
        return std::numeric_limits<double>::lowest();
    }
    geometry_msgs::PointStamped point;
    point.header.frame_id = this->_safety_area_vertical_frame_;
    point.point.z = this->_safety_area_min_z_;
    const auto ret = this->transformer_->transformSingle(point, frame_id);
    if (!ret) {
        ROS_ERROR_THROTTLE(
            1.0,
            "[ControlManager]: SafetyArea: Could not transform safety area's min_z to '%s'",
            frame_id.c_str()
        );
        return std::numeric_limits<double>::lowest();
    }
    return ret->point.z;
}

/// hypha port: estimation_manager's max_z is not subscribed, so only the safety area
/// bounds it; a failed transform returns no bound.
double ControlManager::getMaxZ(const std::string& frame_id)
{
    if (!this->use_safety_area_) {
        return std::numeric_limits<double>::max();
    }
    geometry_msgs::PointStamped point;
    point.header.frame_id = this->_safety_area_vertical_frame_;
    point.point.z = this->_safety_area_max_z_;
    const auto ret = this->transformer_->transformSingle(point, frame_id);
    if (!ret) {
        ROS_ERROR_THROTTLE(
            1.0,
            "[ControlManager]: SafetyArea: Could not transform safety area's max_z to '%s'",
            frame_id.c_str()
        );
        return std::numeric_limits<double>::max();
    }
    return ret->point.z;
}

double ControlManager::getMass() const
{
    if (this->last_control_output_.diagnostics.mass_estimator) {
        return this->_uav_mass_ + this->last_control_output_.diagnostics.mass_difference;
    }
    return this->_uav_mass_;
}

}  // namespace

int main(int argc, char** argv)
{
    return hypha::runNode<ControlManager>(argc, argv, "uav1_control_manager");
}
