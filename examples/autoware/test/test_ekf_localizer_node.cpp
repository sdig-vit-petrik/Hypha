/// @file
/// The upstream EKFLocalizerNode on the bus, through the rclcpp shim, on simulated time that
/// the test drives: its parameters and topics from config/ekf_localizer.toml, the trigger
/// service, an initial pose, pose and twist measurements in; odometry, /tf and diagnostics
/// out. The scenarios follow upstream's test_ekf_localizer_node_integration.cpp, which needs
/// rclcpp executors: the node does nothing before the trigger and an initial pose, and its
/// diagnostics go from WARN to ERROR as pose updates stay away. Each step moves /clock by
/// one predict period and turns each node once, so the EKF's timer runs once per step.

#include "ekf_localizer_node.hpp"

#include <diagnostic_msgs/msg/diagnostic_array.hpp>
#include <geometry_msgs/msg/pose_with_covariance_stamped.hpp>
#include <geometry_msgs/msg/twist_with_covariance_stamped.hpp>
#include <gtest/gtest.h>
#include <nav_msgs/msg/odometry.hpp>
#include <rclcpp/rclcpp.hpp>
#include <rosgraph_msgs/Clock.h>
#include <std_srvs/srv/set_bool.hpp>
#include <tf2_msgs/msg/tf_message.hpp>

#include <future>
#include <memory>
#include <optional>
#include <string>

namespace {

constexpr const char* kParameterFile = "test/config/ekf_localizer_node_test.toml";
/// One period of predict_frequency (50 Hz).
constexpr int64_t kTickNanos = 20'000'000;
constexpr double  kNear = 1e-6;

class EkfLocalizerNodeTest : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->ekf_ =
            std::make_shared<autoware::ekf_localizer::EKFLocalizerNode>(rclcpp::NodeOptions{});
        this->harness_ = rclcpp::Node::make_shared("ekf_test_harness");
        hypha::Node& bus = this->harness_->hypha_node();
        this->clock_ = bus.advertise<rosgraph_msgs::Clock>(hypha::Topic{"/clock"});
        this->initialPose_ = bus.advertise<geometry_msgs::msg::PoseWithCovarianceStamped>(
            hypha::Topic{"/initialpose3d"}
        );
        this->pose_ = bus.advertise<geometry_msgs::msg::PoseWithCovarianceStamped>(
            hypha::Topic{"/localization/pose_estimator/pose_with_covariance"}
        );
        this->twist_ = bus.advertise<geometry_msgs::msg::TwistWithCovarianceStamped>(
            hypha::Topic{"/localization/twist_estimator/twist_with_covariance"}
        );
        this->trigger_ = this->harness_->create_client<std_srvs::srv::SetBool>(
            "/localization/pose_twist_fusion_filter/trigger_node"
        );
        this->odomIn_ = bus.subscribe<nav_msgs::msg::Odometry>(
            hypha::Topic{"/localization/pose_twist_fusion_filter/kinematic_state"},
            [this](const nav_msgs::msg::Odometry::ConstPtr& odom) {
                this->odom_ = *odom;
                ++this->odomCount_;
            }
        );
        this->diagnosticsIn_ = bus.subscribe<diagnostic_msgs::msg::DiagnosticArray>(
            hypha::Topic{"/diagnostics"},
            [this](const diagnostic_msgs::msg::DiagnosticArray::ConstPtr& array) {
                this->diagnostics_ = *array;
            }
        );
        this->tfIn_ = bus.subscribe<tf2_msgs::msg::TFMessage>(
            hypha::Topic{"/tf"}, [this](const tf2_msgs::msg::TFMessage::ConstPtr& message) {
                for (const auto& transform : message->transforms) {
                    if (transform.child_frame_id == "base_link") {
                        this->baseLink_ = transform;
                    }
                }
            }
        );
        // The first /clock of this scenario: one period after the last one.
        this->tick();
    }

    /// Each node takes one dispatch turn, three times, so a sample sent in one turn is
    /// served in the next.
    void turn()
    {
        for (int i = 0; i < 3; ++i) {
            rclcpp::spin_some(this->harness_);
            rclcpp::spin_some(this->ekf_);
        }
    }

    void publishClock()
    {
        rosgraph_msgs::Clock clock;
        clock.clock = rclcpp::Time(this->nowNanos_);
        ASSERT_TRUE(this->clock_.publish(clock));
        this->turn();
    }

    /// One predict period later.
    void tick()
    {
        this->nowNanos_ += kTickNanos;
        this->publishClock();
    }

    void triggerNode()
    {
        ASSERT_TRUE(this->trigger_->service_is_ready());
        auto request = std::make_shared<std_srvs::srv::SetBool::Request>();
        request->data = true;
        auto call = this->trigger_->async_send_request(request);
        for (int i = 0; i < 20; ++i) {
            if (call.future.wait_for(std::chrono::seconds(0)) == std::future_status::ready) {
                break;
            }
            this->turn();
        }
        ASSERT_EQ(call.future.wait_for(std::chrono::seconds(0)), std::future_status::ready);
        EXPECT_TRUE(call.get()->success);
    }

    geometry_msgs::msg::PoseWithCovarianceStamped poseAt(double x, double y) const
    {
        geometry_msgs::msg::PoseWithCovarianceStamped pose;
        pose.header.stamp = rclcpp::Time(this->nowNanos_);
        pose.header.frame_id = "map";
        pose.pose.pose.position.x = x;
        pose.pose.pose.position.y = y;
        pose.pose.pose.orientation.w = 1.0;
        for (const size_t diagonal : {0U, 7U, 14U, 21U, 28U, 35U}) {
            pose.pose.covariance[diagonal] = 0.01;
        }
        return pose;
    }

    geometry_msgs::msg::TwistWithCovarianceStamped twistOf(double vx) const
    {
        geometry_msgs::msg::TwistWithCovarianceStamped twist;
        twist.header.stamp = rclcpp::Time(this->nowNanos_);
        twist.header.frame_id = "base_link";
        twist.twist.twist.linear.x = vx;
        twist.twist.covariance[0] = 0.1;
        twist.twist.covariance[35] = 0.1;
        return twist;
    }

    /// A status of the latest diagnostics whose message holds @p text.
    bool diagnosed(const std::string& text) const
    {
        if (!this->diagnostics_) {
            return false;
        }
        for (const auto& status : this->diagnostics_->status) {
            if (status.message.find(text) != std::string::npos) {
                return true;
            }
        }
        return false;
    }

    /// Ticks until the diagnostics hold @p text, at most @p ticks times.
    bool tickUntilDiagnosed(const std::string& text, int ticks)
    {
        for (int i = 0; i < ticks; ++i) {
            this->tick();
            if (this->diagnosed(text)) {
                return true;
            }
        }
        return false;
    }

    /// Simulated time belongs to the process and never goes back, so every scenario goes on
    /// from where the one before it stopped: it starts at 100 s, as upstream's does, away
    /// from the edge cases of time 0.
    static inline int64_t                                            nowNanos_{100'000'000'000};
    std::shared_ptr<autoware::ekf_localizer::EKFLocalizerNode>       ekf_;
    rclcpp::Node::SharedPtr                                          harness_;
    hypha::Publisher<rosgraph_msgs::Clock>                           clock_;
    hypha::Publisher<geometry_msgs::msg::PoseWithCovarianceStamped>  initialPose_;
    hypha::Publisher<geometry_msgs::msg::PoseWithCovarianceStamped>  pose_;
    hypha::Publisher<geometry_msgs::msg::TwistWithCovarianceStamped> twist_;
    rclcpp::Client<std_srvs::srv::SetBool>::SharedPtr                trigger_;
    hypha::Subscriber<nav_msgs::msg::Odometry>                       odomIn_;
    hypha::Subscriber<diagnostic_msgs::msg::DiagnosticArray>         diagnosticsIn_;
    hypha::Subscriber<tf2_msgs::msg::TFMessage>                      tfIn_;
    std::optional<nav_msgs::msg::Odometry>                           odom_;
    std::optional<diagnostic_msgs::msg::DiagnosticArray>             diagnostics_;
    std::optional<geometry_msgs::msg::TransformStamped>              baseLink_;
    size_t                                                           odomCount_{0};
};

}  // namespace

// Upstream's GatekeeperInitialization: nothing before the trigger and an initial pose,
// then odometry and map -> base_link at that pose.
TEST_F(EkfLocalizerNodeTest, NothingIsEstimatedBeforeTheTriggerAndAnInitialPose)
{
    for (int i = 0; i < 8; ++i) {
        this->tick();
    }
    EXPECT_EQ(this->odomCount_, 0U);

    this->triggerNode();
    this->diagnostics_.reset();
    EXPECT_TRUE(this->tickUntilDiagnosed("[ERROR]initial pose is not set", 10));
    EXPECT_EQ(this->odomCount_, 0U);

    ASSERT_TRUE(this->initialPose_.publish(this->poseAt(10.0, 20.0)));
    this->turn();
    this->tick();

    ASSERT_TRUE(this->odom_.has_value());
    EXPECT_NEAR(this->odom_->pose.pose.position.x, 10.0, kNear);
    EXPECT_NEAR(this->odom_->pose.pose.position.y, 20.0, kNear);
    EXPECT_EQ(this->odom_->header.frame_id, "map");
    ASSERT_TRUE(this->baseLink_.has_value());
    EXPECT_EQ(this->baseLink_->header.frame_id, "map");
    EXPECT_NEAR(this->baseLink_->transform.translation.x, 10.0, kNear);
    EXPECT_NEAR(this->baseLink_->transform.translation.y, 20.0, kNear);
}

// Upstream's TimeoutCascade: WARN after 50 ticks without a pose update, ERROR after 100.
TEST_F(EkfLocalizerNodeTest, MissingPoseUpdatesWarnThenFail)
{
    this->triggerNode();
    ASSERT_TRUE(this->initialPose_.publish(this->poseAt(0.0, 0.0)));
    this->turn();
    this->tick();

    for (int i = 0; i < 40; ++i) {
        this->tick();
    }
    EXPECT_FALSE(this->diagnosed("[WARN]pose is not updated"));
    EXPECT_TRUE(this->tickUntilDiagnosed("[WARN]pose is not updated", 30));
    EXPECT_TRUE(this->tickUntilDiagnosed("[ERROR]pose is not updated", 70));
}

// Poses along x at 1 m/s and the matching twist: the estimate follows them.
TEST_F(EkfLocalizerNodeTest, PosesAndTwistMoveTheEstimate)
{
    this->triggerNode();
    ASSERT_TRUE(this->initialPose_.publish(this->poseAt(0.0, 0.0)));
    this->turn();
    this->tick();

    constexpr double kSpeed = 1.0;
    constexpr int    kTicks = 100;
    for (int i = 1; i <= kTicks; ++i) {
        this->nowNanos_ += kTickNanos;
        const double x = kSpeed * static_cast<double>(i) * 0.02;
        ASSERT_TRUE(this->pose_.publish(this->poseAt(x, 0.0)));
        ASSERT_TRUE(this->twist_.publish(this->twistOf(kSpeed)));
        this->publishClock();
    }

    ASSERT_TRUE(this->odom_.has_value());
    EXPECT_NEAR(this->odom_->pose.pose.position.x, kSpeed * kTicks * 0.02, 0.1);
    EXPECT_NEAR(this->odom_->pose.pose.position.y, 0.0, 0.05);
    EXPECT_NEAR(this->odom_->twist.twist.linear.x, kSpeed, 0.1);
    EXPECT_FALSE(this->diagnosed("[WARN]pose is not updated"));
    EXPECT_FALSE(this->diagnosed("[WARN]twist is not updated"));
}

int main(int argc, char** argv)
{
    ::testing::InitGoogleTest(&argc, argv);
    const char* hyphaArgv[] = {argv[0], kParameterFile};
    rclcpp::init(2, hyphaArgv);
    const int result = RUN_ALL_TESTS();
    rclcpp::shutdown();
    return result;
}
