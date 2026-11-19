/// mrs_simulator: the native multirotor model as the clock driver of the flight, and
/// the UAV's hardware API (hw_api/*). One cycle: every output, the capabilities latch,
/// the status divider and the arming and offboard servers belong to the driver cycle.
#include <hypha/hypha.h>
#include <hypha_log/hypha_log.h>

#include "simulator_core.h"

#include <geometry_msgs/QuaternionStamped.h>
#include <mrs_host/plugin_host.h>
#include <mrs_host/service.h>
#include <mrs_msgs/HwApiAttitudeRateCmd.h>
#include <mrs_msgs/HwApiCapabilities.h>
#include <mrs_msgs/HwApiStatus.h>
#include <nav_msgs/Odometry.h>
#include <sensor_msgs/Imu.h>
#include <std_msgs/String.h>
#include <std_srvs/SetBool.h>
#include <std_srvs/Trigger.h>

#include <filesystem>
#include <fstream>
#include <iterator>
#include <memory>
#include <optional>
#include <string>
#include <utility>
#include <vector>

HYPHA_LOG_MODULE_INIT("mrs_simulator")

namespace {

Eigen::Vector3d vector3(const hypha::ConfigView& view, const std::string& key)
{
    const hypha::ConfigList list = view.list(key);
    if (list.size() != 3) {
        throw std::runtime_error("mrs_simulator: '" + view.name(key) + "' needs three values");
    }
    return {
        list.at(0).required<double>(""),
        list.at(1).required<double>(""),
        list.at(2).required<double>("")
    };
}

/// The URDF at @p path for viewers (Foxglove's 3D panel reads /robot_description), with
/// every `{uav}` replaced by @p uav and every `{dir}` by the file's absolute directory,
/// so its `file://` meshes resolve.
std::string robotDescription(const std::string& path, const std::string& uav)
{
    std::ifstream file(path);
    if (!file) {
        throw std::runtime_error("mrs_simulator: cannot read robot_description '" + path + "'");
    }
    std::string       text{std::istreambuf_iterator<char>(file), std::istreambuf_iterator<char>()};
    const std::string dir = std::filesystem::absolute(path).parent_path().string();
    for (const auto& [token, value] : {std::pair{"{uav}", uav}, std::pair{"{dir}", dir}}) {
        const std::string key = token;
        for (size_t at = text.find(key); at != std::string::npos; at = text.find(key, at)) {
            text.replace(at, key.size(), value);
            at += value.size();
        }
    }
    return text;
}

mrs_sim::SimulatorParams readParams()
{
    using hypha::constraints::positive;
    const hypha::ConfigView  config = hypha::config();
    mrs_sim::SimulatorParams params;
    const hypha::ConfigView  model = hypha::config("model");
    params.model.mass = model.required<double>("mass", positive());
    params.model.inertia = vector3(model, "inertia");
    params.model.armLength = model.required<double>("arm_length", positive());
    params.model.kf = model.required<double>("kf", positive());
    params.model.km = model.required<double>("km", positive());
    params.model.rpmMin = model.required<double>("rpm_min");
    params.model.rpmMax = model.required<double>("rpm_max", positive());
    params.model.motorTimeConstant = model.required<double>("motor_time_constant", positive());
    params.model.linearDrag = model.required<double>("linear_drag");
    params.model.computeAllocation();
    const hypha::ConfigView rate = hypha::config("rate_controller");
    params.rate.kp = vector3(rate, "kp");
    params.rate.ki = vector3(rate, "ki");
    params.rate.kd = vector3(rate, "kd");
    params.rate.integralLimit = rate.required<double>("integral_limit", positive());
    const hypha::ConfigView sim = hypha::config("simulator");
    params.inputTimeout = sim.required<double>("input_timeout_s", positive());
    params.substeps = static_cast<int>(sim.required<int64_t>("substeps"));
    params.armThrottleLimit = sim.required<double>("arm_throttle_limit");
    static_cast<void>(config);
    return params;
}

class Simulator final : public hypha::CycleNode
{
  public:
    hypha::Latest<mrs_msgs::HwApiAttitudeRateCmd> attitude_rate_cmd{
        *this, "attitude_rate_cmd", unsupervised
    };
    hypha::Output<nav_msgs::Odometry>               ground_truth{*this, "ground_truth"};
    hypha::Output<nav_msgs::Odometry>               odometry{*this, "odometry"};
    hypha::Output<sensor_msgs::Imu>                 imu{*this, "imu"};
    hypha::Output<geometry_msgs::QuaternionStamped> orientation{*this, "orientation"};
    hypha::Output<mrs_msgs::HwApiStatus>            status{*this, "status"};
    hypha::Output<mrs_msgs::HwApiCapabilities>      capabilities{*this, "capabilities", latched};
    hypha::Output<std_msgs::String> robot_description{*this, "robot_description", latched};

    void setup(hypha::Node&) override
    {
        const hypha::ConfigView config = hypha::config();
        this->uav_ = config.required<std::string>("uav_name", hypha::constraints::nonempty());
        this->core_.emplace(readParams());
        if (const auto urdf = config.optional<std::string>("robot_description")) {
            this->robotDescription_ = robotDescription(*urdf, this->uav_);
        }

        const hypha::ConfigView clock = hypha::config("clock");
        const int64_t           periodMs = clock.required<int64_t>("period_ms");
        if (periodMs <= 0) {
            throw std::runtime_error("mrs_simulator: clock.period_ms must be positive");
        }
        this->period_ = static_cast<double>(periodMs) / 1000.0;
        this->setRate(hypha::Hz(1000.0 / static_cast<double>(periodMs)));
        hypha::ClockDriverOptions driver;
        driver.start = hypha::Time::fromSeconds(clock.required<double>("start_s"));
        driver.timeout = hypha::Duration::fromSeconds(
            clock.required<double>("timeout_s", hypha::constraints::positive())
        );
        // Unset: as fast as the participants allow; 1.0 paces a flight to the wall clock.
        driver.realTimeFactor =
            clock.optional<double>("real_time_factor", hypha::constraints::positive());
        for (const hypha::ConfigView entry : clock.list("participants")) {
            driver.participants.push_back(entry.required<std::string>(""));
        }
        this->setClockDriver(driver);
        this->statusEvery_ =
            static_cast<int>(hypha::config("simulator").required<int64_t>("status_every_steps"));

        this->host_ =
            std::make_unique<mrs::PluginHost>(*this, "mrs_simulator", "/" + this->uav_ + "/hw_api");
        this->arming_ = std::make_unique<mrs::Serve<std_srvs::SetBool>>(
            *this->host_,
            "/" + this->uav_ + "/hw_api/arming",
            [this](uint64_t, const std_srvs::SetBool::Request& req) {
                const mrs_sim::Answer answer = this->core_->arm(req.data);
                HYPHA_LOG_INFO("arming {}: {}", req.data ? "on" : "off", answer.message);
                std_srvs::SetBool::Response res;
                res.success = answer.success;
                res.message = answer.message;
                return std::optional{res};
            }
        );
        this->offboard_ = std::make_unique<mrs::Serve<std_srvs::Trigger>>(
            *this->host_,
            "/" + this->uav_ + "/hw_api/offboard",
            [this](uint64_t, const std_srvs::Trigger::Request&) {
                const mrs_sim::Answer answer = this->core_->offboard();
                HYPHA_LOG_INFO("offboard: {}", answer.message);
                std_srvs::Trigger::Response res;
                res.success = answer.success;
                res.message = answer.message;
                return std::optional{res};
            }
        );
    }

    void step(const hypha::CycleTime& time) override
    {
        this->host_->step(time, [this, &time] {
            this->simulate(time);
        });
    }

  private:
    void simulate(const hypha::CycleTime& time)
    {
        const ros::Time now = ros::Time::fromHypha(time.now);
        if (this->attitude_rate_cmd.has() && this->attitude_rate_cmd.fresh()) {
            const mrs_msgs::HwApiAttitudeRateCmd& cmd = *this->attitude_rate_cmd;
            const mrs_sim::AttitudeRateCommand    command{
                Eigen::Vector3d(cmd.body_rate.x, cmd.body_rate.y, cmd.body_rate.z), cmd.throttle
            };
            this->core_->command(command, ros::Time(cmd.stamp).toSec());
        }
        this->core_->step(now.toSec(), this->period_);
        this->publish(now);
    }

    void publish(const ros::Time& now)
    {
        const mrs_sim::State& s = this->core_->model().state();
        const std::string     world = this->uav_ + "/world_origin";
        const std::string     body = this->uav_ + "/fcu";

        geometry_msgs::Quaternion q;
        q.w = s.q.w();
        q.x = s.q.x();
        q.y = s.q.y();
        q.z = s.q.z();
        const Eigen::Vector3d vBody = s.q.conjugate() * s.v;

        nav_msgs::Odometry odom;
        odom.header.stamp = now;
        odom.header.frame_id = world;
        odom.child_frame_id = body;
        odom.pose.pose.position.x = s.p.x();
        odom.pose.pose.position.y = s.p.y();
        odom.pose.pose.position.z = s.p.z();
        odom.pose.pose.orientation = q;
        odom.twist.twist.linear.x = vBody.x();
        odom.twist.twist.linear.y = vBody.y();
        odom.twist.twist.linear.z = vBody.z();
        odom.twist.twist.angular.x = s.omega.x();
        odom.twist.twist.angular.y = s.omega.y();
        odom.twist.twist.angular.z = s.omega.z();
        this->ground_truth.send(odom);
        this->odometry.send(odom);

        sensor_msgs::Imu imu;
        imu.header.stamp = now;
        imu.header.frame_id = body;
        imu.orientation = q;
        imu.angular_velocity = odom.twist.twist.angular;
        const Eigen::Vector3d f = this->core_->model().acceleration();
        imu.linear_acceleration.x = f.x();
        imu.linear_acceleration.y = f.y();
        imu.linear_acceleration.z = f.z();
        this->imu.send(imu);

        geometry_msgs::QuaternionStamped orientation;
        orientation.header.stamp = now;
        orientation.header.frame_id = world;
        orientation.quaternion = q;
        this->orientation.send(orientation);

        if (this->steps_ % this->statusEvery_ == 0) {
            mrs_msgs::HwApiStatus status;
            status.stamp = now;
            status.connected = true;
            status.armed = this->core_->armed();
            status.offboard = this->core_->isOffboard();
            status.mode = status.offboard ? "OFFBOARD" : "MANUAL";
            this->status.send(status);
        }
        if (this->steps_ == 0) {
            mrs_msgs::HwApiCapabilities caps;
            caps.stamp = now;
            caps.api_name = "mrs_simulator (hypha)";
            caps.accepts_attitude_rate_cmd = true;
            caps.produces_ground_truth = true;
            caps.produces_imu = true;
            caps.produces_orientation = true;
            caps.produces_odometry = true;
            caps.produces_angular_velocity = true;
            this->capabilities.send(caps);
            if (!this->robotDescription_.empty()) {
                this->robot_description.send({.data = this->robotDescription_});
            }
        }
        ++this->steps_;
    }

    std::string                                    uav_;
    std::optional<mrs_sim::SimulatorCore>          core_;
    std::string                                    robotDescription_;
    double                                         period_{0.01};
    int                                            statusEvery_{10};
    uint64_t                                       steps_{0};
    std::unique_ptr<mrs::PluginHost>               host_;
    std::unique_ptr<mrs::Serve<std_srvs::SetBool>> arming_;
    std::unique_ptr<mrs::Serve<std_srvs::Trigger>> offboard_;
};

}  // namespace

int main(int argc, char** argv)
{
    return hypha::runNode<Simulator>(argc, argv, "mrs_simulator");
}
