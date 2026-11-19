#pragma once

/// @file
/// mrs_lib::Transformer over the hypha TF buffer (hypha port of mrs_lib/transformer.h).
/// A Transformer made from a NodeHandle reads the hosting cycle's hypha::Transforms port,
/// so it looks up only in a step. No lookup waits: setLookupTimeout() is accepted and
/// ignored. latlon_origin is not ported and fails with an error log.

#include <hypha/cycle_transforms.h>

#include <Eigen/Dense>
#include <geometry_msgs/PointStamped.h>
#include <geometry_msgs/PoseStamped.h>
#include <geometry_msgs/TransformStamped.h>
#include <geometry_msgs/Vector3Stamped.h>
#include <mrs_lib/geometry/misc.h>
#include <mrs_msgs/ReferenceStamped.h>
#include <ros/node_handle.h>
#include <ros/ros.h>

#include <memory>
#include <optional>
#include <string>
#include <string_view>

namespace mrs_lib {

class Transformer
{
  public:
    /// No TF input: only equal frames transform.
    Transformer() = default;

    explicit Transformer(const std::string& node_name, const ros::Duration& = ros::Duration(10.0))
        : node_name_(node_name)
    {
    }

    /// Reads the TF input of @p nh's host, created with @p cache on first use.
    Transformer(
        const ros::NodeHandle& nh,
        const std::string&     node_name = std::string(),
        const ros::Duration&   cache = ros::Duration(10.0)
    )
        : node_name_(node_name), tf_(&nh.host().transforms(cache.toHypha()))
    {
    }

    void setDefaultFrame(const std::string& frame_id)
    {
        this->default_frame_id_ = frame_id;
    }

    void setDefaultPrefix(const std::string& prefix)
    {
        this->prefix_ = prefix.empty() ? std::string{} : prefix + "/";
    }

    void setLookupTimeout(const ros::Duration = ros::Duration(0))
    {
    }

    void retryLookupNewest(const bool retry = true)
    {
        this->retry_lookup_newest_ = retry;
    }

    void beQuiet(const bool quiet = true)
    {
        this->quiet_ = quiet;
    }

    std::string resolveFrame(const std::string& frame_id) const
    {
        if (frame_id.empty()) {
            return this->default_frame_id_;
        }
        if (this->prefix_.empty() || frame_id.starts_with(this->prefix_)) {
            return frame_id;
        }
        return this->prefix_ + frame_id;
    }

    /// The transform that maps @p from_frame coordinates into @p to_frame at @p stamp
    /// (zero: the newest common time).
    std::optional<geometry_msgs::TransformStamped> getTransform(
        const std::string& from_frame,
        const std::string& to_frame,
        const ros::Time&   stamp = ros::Time(0)
    )
    {
        return this->lookup(this->resolveFrame(from_frame), this->resolveFrame(to_frame), stamp);
    }

    /// @p what in @p to_frame, looked up at its header stamp. PoseStamped, PointStamped,
    /// Vector3Stamped and mrs_msgs::ReferenceStamped.
    template <class T>
    std::optional<T> transformSingle(const T& what, const std::string& to_frame)
    {
        const std::optional<geometry_msgs::TransformStamped> tf = this->lookup(
            this->resolveFrame(what.header.frame_id),
            this->resolveFrame(to_frame),
            ros::Time(what.header.stamp)
        );
        if (!tf) {
            return std::nullopt;
        }
        return this->transform(what, *tf);
    }

    template <class T>
    std::optional<T>
    transformSingle(const std::shared_ptr<const T>& what, const std::string& to_frame)
    {
        return this->transformSingle(*what, to_frame);
    }

    /// @p what, expressed in tf's child frame, expressed in tf's parent frame.
    template <class T>
    std::optional<T> transform(const T& what, const geometry_msgs::TransformStamped& tf)
    {
        const std::string from = this->resolveFrame(what.header.frame_id);
        if (from != this->resolveFrame(tf.child_frame_id)) {
            this->warn(
                from, tf.header.frame_id, "the message is not in the transform's source frame"
            );
            return std::nullopt;
        }
        T out = what;
        out.header.frame_id = this->resolveFrame(tf.header.frame_id);
        apply(tf, out);
        return out;
    }

  private:
    static Eigen::Quaterniond rotationOf(const geometry_msgs::TransformStamped& tf)
    {
        const auto& q = tf.transform.rotation;
        return Eigen::Quaterniond(q.w, q.x, q.y, q.z);
    }

    static Eigen::Vector3d translationOf(const geometry_msgs::TransformStamped& tf)
    {
        const auto& t = tf.transform.translation;
        return Eigen::Vector3d(t.x, t.y, t.z);
    }

    static Eigen::Vector3d
    point(const geometry_msgs::TransformStamped& tf, const Eigen::Vector3d& p)
    {
        return rotationOf(tf) * p + translationOf(tf);
    }

    static void apply(const geometry_msgs::TransformStamped& tf, geometry_msgs::PointStamped& msg)
    {
        const Eigen::Vector3d p = point(tf, Eigen::Vector3d(msg.point.x, msg.point.y, msg.point.z));
        msg.point.x = p.x();
        msg.point.y = p.y();
        msg.point.z = p.z();
    }

    static void apply(const geometry_msgs::TransformStamped& tf, geometry_msgs::Vector3Stamped& msg)
    {
        const Eigen::Vector3d v =
            rotationOf(tf) * Eigen::Vector3d(msg.vector.x, msg.vector.y, msg.vector.z);
        msg.vector.x = v.x();
        msg.vector.y = v.y();
        msg.vector.z = v.z();
    }

    static void apply(const geometry_msgs::TransformStamped& tf, geometry_msgs::PoseStamped& msg)
    {
        auto&                 pose = msg.pose;
        const Eigen::Vector3d p =
            point(tf, Eigen::Vector3d(pose.position.x, pose.position.y, pose.position.z));
        const auto&              o = pose.orientation;
        const Eigen::Quaterniond q = rotationOf(tf) * Eigen::Quaterniond(o.w, o.x, o.y, o.z);
        pose.position.x = p.x();
        pose.position.y = p.y();
        pose.position.z = p.z();
        pose.orientation.x = q.x();
        pose.orientation.y = q.y();
        pose.orientation.z = q.z();
        pose.orientation.w = q.w();
    }

    /// Upstream: the reference as a pose with its heading, transformed, heading read back.
    static void apply(const geometry_msgs::TransformStamped& tf, mrs_msgs::ReferenceStamped& msg)
    {
        auto&                 ref = msg.reference;
        const Eigen::Vector3d p =
            point(tf, Eigen::Vector3d(ref.position.x, ref.position.y, ref.position.z));
        const Eigen::Quaterniond q =
            rotationOf(tf) * mrs_lib::geometry::quaternionFromHeading(ref.heading);
        ref.position.x = p.x();
        ref.position.y = p.y();
        ref.position.z = p.z();
        ref.heading = mrs_lib::geometry::headingFromRot(q);
    }

    static geometry_msgs::TransformStamped
    identity(const std::string& from, const std::string& to, const ros::Time& stamp)
    {
        geometry_msgs::TransformStamped tf;
        tf.header.frame_id = to;
        tf.header.stamp = stamp;
        tf.child_frame_id = from;
        tf.transform.rotation.w = 1.0;
        return tf;
    }

    std::optional<geometry_msgs::TransformStamped>
    lookup(const std::string& from, const std::string& to, const ros::Time& stamp)
    {
        if (from.empty() || to.empty()) {
            this->warn(from, to, "cannot transform to/from an empty frame");
            return std::nullopt;
        }
        if (from == to) {
            return identity(from, to, stamp);
        }
        const std::string latlon = this->resolveFrame("latlon_origin");
        if (from == latlon || to == latlon) {
            ROS_ERROR_THROTTLE(
                1.0, "[%s]: Transformer: latlon_origin is not ported", this->node_name_.c_str()
            );
            return std::nullopt;
        }
        if (this->tf_ == nullptr) {
            this->warn(from, to, "no TF input");
            return std::nullopt;
        }
        // ROS: time 0 asks for the latest common time.
        auto found = stamp.isZero() ? this->tf_->lookupLatest(to, from)
                                    : this->tf_->lookup(to, from, stamp.toHypha());
        if (!found && this->retry_lookup_newest_) {
            found = this->tf_->lookupLatest(to, from);
        }
        if (!found) {
            this->warn(from, to, errorText(found.error()));
            return std::nullopt;
        }
        return *found;
    }

    static const char* errorText(hypha::TransformError error)
    {
        switch (error) {
            case hypha::TransformError::InvalidFrame:
                return "invalid frame name";
            case hypha::TransformError::UnknownFrame:
                return "unknown frame";
            case hypha::TransformError::NotConnected:
                return "the frames are not connected";
            case hypha::TransformError::BeforeBuffer:
                return "the time is before the buffer";
            case hypha::TransformError::AfterBuffer:
                return "the time is after the buffer";
        }
        return "unknown error";
    }

    void warn(const std::string& from, const std::string& to, std::string_view why) const
    {
        if (this->quiet_) {
            return;
        }
        ROS_WARN_THROTTLE(
            1.0,
            "[%s]: Transformer: cannot transform from \"%s\" to \"%s\": %.*s",
            this->node_name_.c_str(),
            from.c_str(),
            to.c_str(),
            static_cast<int>(why.size()),
            why.data()
        );
    }

    std::string        node_name_;
    std::string        default_frame_id_;
    std::string        prefix_;
    bool               quiet_{false};
    bool               retry_lookup_newest_{false};
    hypha::Transforms* tf_{nullptr};
};

}  // namespace mrs_lib
