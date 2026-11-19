/// mrs_lib::Transformer over the host cycle's hypha::Transforms port: one node, host time.
/// A publisher puts uav1/gt_origin in uav1/fcu (1 m along x, 90 degrees of yaw) on /tf,
/// stamped at 100 s; the checks run in the host's step.
#include <hypha/cycle_node.h>
#include <hypha/node.h>
#include <hypha/publisher.h>
#include <hypha_log/hypha_log.h>

#include "check.h"

#include <geometry_msgs/PointStamped.h>
#include <geometry_msgs/PoseStamped.h>
#include <geometry_msgs/Vector3Stamped.h>
#include <mrs_host/plugin_host.h>
#include <mrs_lib/transformer.h>
#include <mrs_msgs/ReferenceStamped.h>
#include <ros/node_handle.h>
#include <tf2_msgs/TFMessage.h>

#include <cmath>
#include <functional>
#include <stdexcept>
#include <string>

HYPHA_LOG_MODULE_INIT("mrs_transformer_test")

namespace {

constexpr double HALF_PI = 1.5707963267948966;

bool near(double a, double b)
{
    return std::abs(a - b) < 1e-9;
}

/// One composed cycle at 100 Hz whose step runs the host, then the checks once a transform arrived.
struct HostCycle
{
    hypha::Cycle          cycle{"transformer"};
    mrs::PluginHost       host{cycle, "mrs_transformer_test", "/uav1/test"};
    std::function<bool()> onStep;  ///< true once it checked
    bool                  checked{false};

    HostCycle()
    {
        this->cycle.setRate(hypha::Hz(100.0));
    }

    struct Step
    {
        HostCycle* self;

        void operator()(const hypha::CycleTime& time) const
        {
            self->host.step(time, [this] {
                if (!self->checked) {
                    self->checked = self->onStep();
                }
            });
        }
    };
};

tf2_msgs::TFMessage gtOriginInFcu()
{
    geometry_msgs::TransformStamped tf;
    tf.header.frame_id = "uav1/fcu";
    tf.header.stamp = ros::Time(100.0);
    tf.child_frame_id = "uav1/gt_origin";
    tf.transform.translation.x = 1.0;
    tf.transform.rotation.z = std::sin(HALF_PI / 2.0);
    tf.transform.rotation.w = std::cos(HALF_PI / 2.0);
    tf2_msgs::TFMessage message;
    message.transforms.push_back(tf);
    return message;
}

}  // namespace

int main()
{
    hypha::Node          node("mrs_transformer_test");
    HostCycle            a;
    ros::NodeHandle      nh(a.host, "~");
    mrs_lib::Transformer transformer(nh, "TransformerTest");
    transformer.setDefaultPrefix("uav1");

    a.onStep = [&] {
        if (!transformer.getTransform("gt_origin", "fcu", ros::Time(0))) {
            return false;
        }

        // transformer_prefixes_frames: "gt_origin" and "fcu" resolve under uav1/.
        const auto tf = transformer.getTransform("gt_origin", "fcu", ros::Time(0));
        MRS_CHECK(tf.has_value());
        MRS_CHECK(tf->header.frame_id == "uav1/fcu");
        MRS_CHECK(tf->child_frame_id == "uav1/gt_origin");
        MRS_CHECK(near(tf->transform.translation.x, 1.0));
        MRS_CHECK(near(ros::Time(tf->header.stamp).toSec(), 100.0));
        MRS_CHECK(transformer.getTransform("uav1/gt_origin", "uav1/fcu", ros::Time(0)).has_value());

        // transformer_uses_the_default_frame: an empty frame_id is uav1/gt_origin.
        transformer.setDefaultFrame("uav1/gt_origin");
        geometry_msgs::PointStamped point;
        point.header.stamp = ros::Time(100.0);
        point.point.x = 1.0;
        const auto moved = transformer.transformSingle(point, "fcu");
        MRS_CHECK(moved.has_value());
        MRS_CHECK(moved->header.frame_id == "uav1/fcu");
        MRS_CHECK(near(moved->point.x, 1.0) && near(moved->point.y, 1.0));

        geometry_msgs::Vector3Stamped vector;
        vector.vector.x = 1.0;
        const auto turned = transformer.transformSingle(vector, "fcu");
        MRS_CHECK(turned.has_value());
        MRS_CHECK(near(turned->vector.x, 0.0) && near(turned->vector.y, 1.0));

        mrs_msgs::ReferenceStamped reference;
        reference.reference.position.x = 1.0;
        reference.reference.heading = 0.0;
        const auto ref = transformer.transformSingle(reference, "fcu");
        MRS_CHECK(ref.has_value());
        MRS_CHECK(near(ref->reference.position.y, 1.0) && near(ref->reference.heading, HALF_PI));

        geometry_msgs::PoseStamped pose;
        pose.header.frame_id = "gt_origin";
        pose.pose.orientation.w = 1.0;
        const auto posed = transformer.transformSingle(pose, "fcu");
        MRS_CHECK(posed.has_value());
        MRS_CHECK(
            near(posed->pose.position.x, 1.0)
            && near(posed->pose.orientation.z, std::sin(HALF_PI / 2.0))
        );

        // transformer_returns_identity_for_equal_frames, known or not.
        const auto same = transformer.getTransform("nowhere", "nowhere", ros::Time(5.0));
        MRS_CHECK(same.has_value() && near(same->transform.rotation.w, 1.0));
        geometry_msgs::PointStamped inFcu;
        inFcu.header.frame_id = "uav1/fcu";
        inFcu.point.z = 3.0;
        const auto kept = transformer.transformSingle(inFcu, "fcu");
        MRS_CHECK(kept.has_value() && near(kept->point.z, 3.0));

        // Unconnected or unknown frames fail.
        MRS_CHECK(!transformer.getTransform("gt_origin", "elsewhere", ros::Time(0)).has_value());

        // transformer_retries_newest: 200 s is after the buffer.
        point.header.stamp = ros::Time(200.0);
        transformer.retryLookupNewest(false);
        MRS_CHECK(!transformer.transformSingle(point, "fcu").has_value());
        transformer.retryLookupNewest(true);
        MRS_CHECK(transformer.transformSingle(point, "fcu").has_value());

        // latlon_origin is out of scope.
        MRS_CHECK(!transformer.getTransform("latlon_origin", "fcu", ros::Time(0)).has_value());
        return true;
    };

    const auto started = a.cycle.start(node, HostCycle::Step{&a});
    if (!started) {
        throw std::runtime_error(started.error());
    }
    auto publisher = hypha::Node().advertise<tf2_msgs::TFMessage>(hypha::Topic{"/tf"});
    for (int i = 0; i < 300 && !a.checked; ++i) {
        static_cast<void>(publisher.publish(gtOriginInFcu()));
        static_cast<void>(node.runFor(hypha::Duration::fromMillis(10)));
    }
    MRS_CHECK(a.checked);

    // Outside a step the transformer has no image to read.
    bool threw = false;
    try {
        static_cast<void>(transformer.getTransform("gt_origin", "fcu", ros::Time(0)));
    } catch (const std::logic_error&) {
        threw = true;
    }
    MRS_CHECK(threw);
    return mrsCheckResult();
}
