/// transform_manager: the subset of upstream MRS TransformManager that this flight
/// needs, as a native hypha CycleNode triggered on estimation_manager/odom_main.
/// hypha port: the tf sources read odom_main, world_origin is the estimator's origin, and
/// stable_origin, amsl, utm, the mapping origin and estimator switching are not ported.
#include <hypha/hypha.h>
#include <hypha_log/hypha_log.h>

#include <Eigen/Geometry>
#include <geometry_msgs/QuaternionStamped.h>
#include <geometry_msgs/TransformStamped.h>
#include <mrs_host/plugin_host.h>
#include <mrs_lib/attitude_converter.h>
#include <mrs_lib/param_loader.h>
#include <mrs_lib/transformer.h>
#include <nav_msgs/Odometry.h>
#include <ros/node_handle.h>
#include <tf2_msgs/TFMessage.h>

#include <cmath>
#include <memory>
#include <optional>
#include <stdexcept>
#include <string>

HYPHA_LOG_MODULE_INIT("transform_manager")

namespace {

/// A frame of upstream's *_tf tables: whether it is published, its parent and child.
struct FrameTf
{
    bool        enabled{false};
    std::string parent;
    std::string child;
};

bool finite(const geometry_msgs::TransformStamped& tf)
{
    const auto& t = tf.transform.translation;
    const auto& q = tf.transform.rotation;
    return std::isfinite(t.x) && std::isfinite(t.y) && std::isfinite(t.z) && std::isfinite(q.x)
        && std::isfinite(q.y) && std::isfinite(q.z) && std::isfinite(q.w);
}

/// The inverted transform of a pose: where the pose's origin frame sits in the pose's frame.
void setInverse(geometry_msgs::TransformStamped& tf, const geometry_msgs::Pose& pose)
{
    Eigen::Quaterniond q(
        pose.orientation.w, pose.orientation.x, pose.orientation.y, pose.orientation.z
    );
    if (q.coeffs().isZero()) {
        q = Eigen::Quaterniond::Identity();  // upstream: an uninitialized quaternion is w = 1
    }
    const Eigen::Quaterniond inverse = q.conjugate();
    const Eigen::Vector3d    t =
        -(inverse * Eigen::Vector3d(pose.position.x, pose.position.y, pose.position.z));
    tf.transform.translation.x = t.x();
    tf.transform.translation.y = t.y();
    tf.transform.translation.z = t.z();
    tf.transform.rotation.x = inverse.x();
    tf.transform.rotation.y = inverse.y();
    tf.transform.rotation.z = inverse.z();
    tf.transform.rotation.w = inverse.w();
}

class TransformManager final : public hypha::CycleNode
{
  public:
    /// The trigger: one step per estimation_manager/odom_main sample. silence_ms in its table.
    hypha::Arrivals<nav_msgs::Odometry> odom_main{*this, "odom_main"};
    /// callbackHwApiOrientation: the source of fcu -> fcu_untilted.
    hypha::Arrivals<geometry_msgs::QuaternionStamped> orientation{
        *this, "orientation", unsupervised
    };
    hypha::Output<tf2_msgs::TFMessage>                tf{*this, "tf"};
    hypha::Output<tf2_msgs::TFMessage>                tf_static{*this, "tf_static", latched};
    /// transform_manager.launch's static_transform_publisher fcu_to_rtk_antenna.
    hypha::Output<tf2_msgs::TFMessage> rtk_antenna_tf{*this, "rtk_antenna_tf", latched};

    void setup(hypha::Node&) override
    {
        const std::string uav =
            hypha::config().required<std::string>("uav_name", hypha::constraints::nonempty());
        this->host_ = std::make_unique<mrs::PluginHost>(
            *this, uav + "_transform_manager", "/" + uav + "/transform_manager"
        );
        this->host_->adoptArrivals("/" + uav + "/estimation_manager/odom_main", this->odom_main);
        this->nh_ = ros::NodeHandle(*this->host_, "~");
        this->transformer_ = std::make_unique<mrs_lib::Transformer>(this->nh_, "TransformManager");
        this->transformer_->retryLookupNewest(true);

        mrs_lib::ParamLoader param_loader(this->nh_, "TransformManager");
        param_loader.loadParam("uav_name", this->uav_name_);
        const std::string prefix = "mrs_uav_managers/transform_manager/";
        const auto        frame = [&](const std::string& name) {
            FrameTf loaded;
            param_loader.loadParam(prefix + name + "/enabled", loaded.enabled);
            param_loader.loadParam(prefix + name + "/parent", loaded.parent);
            param_loader.loadParam(prefix + name + "/child", loaded.child);
            loaded.parent = this->uav_name_ + "/" + loaded.parent;
            loaded.child = this->uav_name_ + "/" + loaded.child;
            return loaded;
        };
        this->local_origin_ = frame("local_origin_tf");
        this->fixed_origin_ = frame("fixed_origin_tf");
        this->world_origin_ = frame("world_origin_tf");
        this->fcu_untilted_ = frame("fcu_untilted_tf");
        this->ns_fcu_ = this->fcu_untilted_.parent;
        std::string rtk_antenna;
        param_loader.loadParam(prefix + "rtk_antenna/frame_id", rtk_antenna);
        this->ns_rtk_antenna_ = this->uav_name_ + "/" + rtk_antenna;
        param_loader.loadParam("rtk_antenna_tf/z", this->rtk_antenna_z_);
        if (!param_loader.loadedSuccessfully()) {
            throw std::runtime_error("transform_manager: could not load all parameters");
        }
        this->setTrigger(this->odom_main);
    }

    void step(const hypha::CycleTime& time) override
    {
        this->host_->step(time, [this, &time] {
            this->publishRtkAntenna();
            if (!this->initialized_ && !this->checkRtkAntenna(time.now)) {
                return;
            }
            this->transforms_.transforms.clear();
            for (const nav_msgs::Odometry& sample : this->odom_main.samples()) {
                this->callbackOdom(sample);
            }
            if (this->fcu_untilted_.enabled) {
                for (const auto& sample : this->orientation.samples()) {
                    this->publishFcuUntiltedTf(sample);
                }
            }
            if (!this->transforms_.transforms.empty()) {
                this->tf.send(this->transforms_);
            }
        });
    }

  private:
    /// The launch file's static fcu -> rtk_antenna transform, sent once.
    void publishRtkAntenna()
    {
        if (this->rtk_antenna_sent_) {
            return;
        }
        geometry_msgs::TransformStamped antenna;
        antenna.header.stamp = ros::Time::now();
        antenna.header.frame_id = this->ns_fcu_;
        antenna.child_frame_id = this->ns_rtk_antenna_;
        antenna.transform.translation.z = this->rtk_antenna_z_;
        antenna.transform.rotation.w = 1.0;
        tf2_msgs::TFMessage message;
        message.transforms = {antenna};
        this->rtk_antenna_tf.send(message);
        this->rtk_antenna_sent_ = true;
    }

    /// onInit's wait for the fcu -> rtk_antenna transform, over steps.
    bool checkRtkAntenna(hypha::Time now)
    {
        if (!this->waiting_since_) {
            this->waiting_since_ = now;
        }
        if (this->transformer_->getTransform(
                this->ns_rtk_antenna_, this->ns_fcu_, ros::Time::now()
            )) {
            ROS_INFO("[TransformManager]: got tf from FCU to RTK antenna");
            ROS_INFO("[TransformManager]: initialized");
            this->initialized_ = true;
            return true;
        }
        if ((now - *this->waiting_since_).seconds() >= 5.0) {
            throw std::runtime_error(
                "transform_manager: the transform from FCU to RTK antenna is not defined; "
                "provide a static tf from "
                + this->ns_fcu_ + " to " + this->ns_rtk_antenna_
            );
        }
        ROS_INFO_THROTTLE(1.0, "[TransformManager]: waiting for tf from FCU to RTK antenna");
        return false;
    }

    /// The estimator tf source, callbackUavState's fixed_origin and the world origin.
    void callbackOdom(const nav_msgs::Odometry& odom)
    {
        if (!this->first_pose_) {
            this->first_pose_ = odom.pose.pose;
            if (this->local_origin_.enabled) {
                this->publishLocalTf(odom);
            }
        }
        tf2_msgs::TFMessage& out = this->transforms_;
        const auto           add = [&](const std::string& child) {
            geometry_msgs::TransformStamped item;
            item.header.stamp = odom.header.stamp;
            item.header.frame_id = this->ns_fcu_;
            item.child_frame_id = child;
            setInverse(item, odom.pose.pose);
            if (finite(item)) {
                out.transforms.push_back(item);
            } else {
                ROS_WARN_THROTTLE(
                    1.0,
                    "[TransformManager]: NaN detected in transform from %s to %s. Not publishing "
                    "tf.",
                    item.header.frame_id.c_str(),
                    item.child_frame_id.c_str()
                );
            }
        };
        add(odom.header.frame_id);
        if (this->fixed_origin_.enabled) {
            add(this->fixed_origin_.child);
        }
        if (this->world_origin_.enabled) {
            add(this->world_origin_.child);
        }
    }

    /// publishFcuUntiltedTf(): fcu_untilted shares fcu's origin and heading, with the
    /// tilt removed, so its rotation in fcu is the inverse attitude times the heading.
    void publishFcuUntiltedTf(const geometry_msgs::QuaternionStamped& msg)
    {
        double heading = 0.0;
        try {
            heading = mrs_lib::AttitudeConverter(msg.quaternion).getHeading();
        } catch (...) {
            ROS_ERROR_THROTTLE(
                1.0, "[TransformManager]: Exception caught during getting heading (fcu_untilted_tf)"
            );
            return;
        }
        const Eigen::Quaterniond attitude = mrs_lib::AttitudeConverter(msg.quaternion);
        const Eigen::Quaterniond untilted =
            attitude.inverse()
            * Eigen::Quaterniond(Eigen::AngleAxisd(heading, Eigen::Vector3d::UnitZ()));
        geometry_msgs::TransformStamped item;
        item.header.stamp = msg.header.stamp;
        item.header.frame_id = this->fcu_untilted_.parent;
        item.child_frame_id = this->fcu_untilted_.child;
        item.transform.rotation = mrs_lib::AttitudeConverter(untilted);
        if (!finite(item)) {
            ROS_WARN_THROTTLE(
                1.0,
                "[TransformManager]: NaN detected in the fcu_untilted transform. Not publishing tf."
            );
            return;
        }
        this->transforms_.transforms.push_back(item);
    }

    /// publishLocalTf(): local_origin in fixed_origin at the first pose, static.
    void publishLocalTf(const nav_msgs::Odometry& odom)
    {
        geometry_msgs::TransformStamped local;
        local.header.stamp = odom.header.stamp;
        local.header.frame_id = this->fixed_origin_.child;
        local.child_frame_id = this->local_origin_.child;
        local.transform.translation.x = odom.pose.pose.position.x;
        local.transform.translation.y = odom.pose.pose.position.y;
        local.transform.translation.z = odom.pose.pose.position.z;
        local.transform.rotation = odom.pose.pose.orientation;
        if (!finite(local)) {
            ROS_WARN("[TransformManager]: NaN in the first pose. Not publishing local_origin.");
            return;
        }
        tf2_msgs::TFMessage message;
        message.transforms = {local};
        this->tf_static.send(message);
        ROS_INFO(
            "[TransformManager]: Broadcasting transform from parent frame: %s to child frame: %s",
            local.header.frame_id.c_str(),
            local.child_frame_id.c_str()
        );
    }

    /// The transforms of this step, published on tf.
    tf2_msgs::TFMessage                   transforms_;
    std::unique_ptr<mrs::PluginHost>      host_;
    ros::NodeHandle                       nh_;
    std::unique_ptr<mrs_lib::Transformer> transformer_;
    std::string                           uav_name_;
    std::string                           ns_fcu_;
    std::string                           ns_rtk_antenna_;
    double                                rtk_antenna_z_{0.0};
    FrameTf                               local_origin_;
    FrameTf                               fixed_origin_;
    FrameTf                               world_origin_;
    FrameTf                               fcu_untilted_;
    bool                                  rtk_antenna_sent_{false};
    bool                                  initialized_{false};
    std::optional<hypha::Time>            waiting_since_;
    std::optional<geometry_msgs::Pose>    first_pose_;
};

}  // namespace

int main(int argc, char** argv)
{
    return hypha::runNode<TransformManager>(argc, argv, "uav1_transform_manager");
}
