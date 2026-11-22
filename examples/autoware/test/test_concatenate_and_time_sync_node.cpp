/// @file
/// The upstream PointCloudConcatenateDataSynchronizerComponent on the bus, through the
/// rclcpp shim: its parameters from TOML, two lidars mounted 1 m left and right of
/// base_link on /tf_static, the vehicle's twist, and the lidars' clouds in; one cloud in
/// base_link, its concatenation info, the synchronized clouds and a diagnostic out.

#include <hypha/transform_buffer.h>

#include "autoware/pointcloud_preprocessor/concatenate_data/concatenate_and_time_sync_node.hpp"

#include <autoware/point_types/types.hpp>
#include <autoware_sensing_msgs/msg/concatenated_point_cloud_info.hpp>
#include <diagnostic_msgs/msg/diagnostic_array.hpp>
#include <geometry_msgs/msg/twist_with_covariance_stamped.hpp>
#include <gtest/gtest.h>
#include <pcl/point_cloud.h>
#include <pcl/point_types.h>
#include <pcl_conversions/pcl_conversions.h>
#include <rclcpp/rclcpp.hpp>
#include <sensor_msgs/msg/point_cloud2.hpp>

#include <array>
#include <cmath>
#include <cstdint>
#include <memory>
#include <optional>
#include <string>
#include <vector>

namespace {

constexpr const char* kParameterFile = "test/config/concatenate_and_time_sync_node_test.toml";
const std::string     kLeft = "/test/concatenate/left/pointcloud_before_sync";
const std::string     kRight = "/test/concatenate/right/pointcloud_before_sync";
/// The vehicle drives straight ahead at this speed.
constexpr double kSpeed = 10.0;
/// The right lidar's cloud is this much newer than the left's.
constexpr uint32_t kRightDelayNanos = 50'000'000;

using Info = autoware_sensing_msgs::msg::ConcatenatedPointCloudInfo;
using Source = autoware_sensing_msgs::msg::SourcePointCloudInfo;
using Status = diagnostic_msgs::msg::DiagnosticStatus;

struct Xyz
{
    float x;
    float y;
    float z;
};

/// Points in each lidar's own frame, along its x axis.
constexpr std::array<Xyz, 3> kLeftPoints{
    {{1.0F, 0.0F, 0.0F}, {2.0F, 0.0F, 0.0F}, {3.0F, 0.0F, 0.0F}}
};
constexpr std::array<Xyz, 2> kRightPoints{{{1.0F, 0.0F, 0.0F}, {2.0F, 0.0F, 0.0F}}};

builtin_interfaces::msg::Time stamp(uint32_t secs, uint32_t nsecs)
{
    builtin_interfaces::msg::Time time;
    time.secs = secs;
    time.nsecs = nsecs;
    return time;
}

template <std::size_t N>
sensor_msgs::msg::PointCloud2
cloud(const std::array<Xyz, N>& points, const std::string& frame, builtin_interfaces::msg::Time at)
{
    pcl::PointCloud<autoware::point_types::PointXYZIRC> pcl;
    for (const Xyz& xyz : points) {
        autoware::point_types::PointXYZIRC point;
        point.x = xyz.x;
        point.y = xyz.y;
        point.z = xyz.z;
        point.intensity = 50;
        point.return_type = 1;
        point.channel = 0;
        pcl.push_back(point);
    }
    sensor_msgs::msg::PointCloud2 message;
    pcl::toROSMsg(pcl, message);
    message.header.frame_id = frame;
    message.header.stamp = at;
    return message;
}

geometry_msgs::msg::TransformStamped mount(const std::string& child, double y)
{
    geometry_msgs::msg::TransformStamped transform;
    transform.header.frame_id = "base_link";
    transform.child_frame_id = child;
    transform.transform.translation.y = y;
    transform.transform.rotation.w = 1.0;
    return transform;
}

/// What the node sent for one scene.
struct Received
{
    std::optional<sensor_msgs::msg::PointCloud2> output;
    std::optional<Info>                          info;
    std::optional<sensor_msgs::msg::PointCloud2> leftSynchronized;
    std::optional<sensor_msgs::msg::PointCloud2> rightSynchronized;
    std::optional<Status>                        status;
};

/// Each expected point is in @p cloud once, and nothing else is.
void expectPoints(const sensor_msgs::msg::PointCloud2& message, const std::vector<Xyz>& expected)
{
    pcl::PointCloud<pcl::PointXYZ> points;
    pcl::fromROSMsg(message, points);
    ASSERT_EQ(points.size(), expected.size());
    for (const Xyz& want : expected) {
        int found = 0;
        for (const pcl::PointXYZ& point : points) {
            const bool near = std::fabs(point.x - want.x) < 1e-4F
                           && std::fabs(point.y - want.y) < 1e-4F
                           && std::fabs(point.z - want.z) < 1e-4F;
            found += near ? 1 : 0;
        }
        EXPECT_EQ(found, 1) << "one point at (" << want.x << ", " << want.y << ", " << want.z
                            << ")";
    }
}

const Source* sourceOf(const Info& info, const std::string& topic)
{
    for (const Source& source : info.source_info) {
        if (source.topic == topic) {
            return &source;
        }
    }
    return nullptr;
}

std::string value(const Status& status, const std::string& key)
{
    for (const auto& entry : status.values) {
        if (entry.key == key) {
            return entry.value;
        }
    }
    return "<missing>";
}

}  // namespace

class ConcatenateAndTimeSyncNode : public testing::Test
{
  protected:
    static void SetUpTestSuite()
    {
        const char* argv[] = {"test", kParameterFile};
        rclcpp::init(2, argv);
        node_ = std::make_shared<
            autoware::pointcloud_preprocessor::PointCloudConcatenateDataSynchronizerComponent>(
            rclcpp::NodeOptions()
        );
        EXPECT_TRUE(hypha::config().unreadKeys().empty());

        probe_ = rclcpp::Node::make_shared("concatenate_probe");
        left_ = probe_->create_publisher<sensor_msgs::msg::PointCloud2>(
            kLeft, rclcpp::SensorDataQoS().keep_last(5)
        );
        right_ = probe_->create_publisher<sensor_msgs::msg::PointCloud2>(
            kRight, rclcpp::SensorDataQoS().keep_last(5)
        );
        twist_ = probe_->create_publisher<geometry_msgs::msg::TwistWithCovarianceStamped>(
            "/test/concatenate/twist", rclcpp::QoS{100}
        );
        received_ = std::make_shared<Received>();
        const auto received = received_;
        subscriptions_.push_back(probe_->create_subscription<sensor_msgs::msg::PointCloud2>(
            "/test/concatenate/output",
            rclcpp::SensorDataQoS().keep_last(5),
            [received](const sensor_msgs::msg::PointCloud2& m) {
                received->output = m;
            }
        ));
        subscriptions_.push_back(probe_->create_subscription<Info>(
            "/test/concatenate/output_info",
            rclcpp::SensorDataQoS().keep_last(5),
            [received](const Info& m) {
                received->info = m;
            }
        ));
        subscriptions_.push_back(probe_->create_subscription<sensor_msgs::msg::PointCloud2>(
            "/test/concatenate/left/pointcloud",
            rclcpp::SensorDataQoS().keep_last(5),
            [received](const sensor_msgs::msg::PointCloud2& m) {
                received->leftSynchronized = m;
            }
        ));
        subscriptions_.push_back(probe_->create_subscription<sensor_msgs::msg::PointCloud2>(
            "/test/concatenate/right/pointcloud",
            rclcpp::SensorDataQoS().keep_last(5),
            [received](const sensor_msgs::msg::PointCloud2& m) {
                received->rightSynchronized = m;
            }
        ));
        const std::string statusName =
            std::string(node_->get_name()) + ": " + node_->get_fully_qualified_name();
        subscriptions_.push_back(probe_->create_subscription<diagnostic_msgs::msg::DiagnosticArray>(
            "/diagnostics",
            rclcpp::QoS(10),
            [received, statusName](const diagnostic_msgs::msg::DiagnosticArray& m) {
                for (const auto& status : m.status) {
                    if (status.name == statusName) {
                        received->status = status;
                    }
                }
            }
        ));

        // The lidars' mounts, latched on /tf_static; one slice delivers them to the node.
        tf_ = std::make_unique<hypha::TransformBuffer>(probe_->hypha_node());
        statics_ = std::make_unique<hypha::StaticTransformBroadcaster>(probe_->hypha_node());
        ASSERT_TRUE(
            statics_->send(std::vector{mount("left_lidar", 1.0), mount("right_lidar", -1.0)})
        );
        ASSERT_TRUE(probe_->hypha_node().runFor(hypha::Duration::fromMillis(20)).ok());
        ASSERT_TRUE(tf_->lookupLatest("base_link", "left_lidar").has_value());
        ASSERT_TRUE(tf_->lookupLatest("base_link", "right_lidar").has_value());
    }

    static void TearDownTestSuite()
    {
        subscriptions_.clear();
        statics_.reset();
        tf_.reset();
        left_.reset();
        right_.reset();
        twist_.reset();
        probe_.reset();
        node_.reset();
    }

    /// The time of a new scene, 10 s after the previous one: the node drops a cloud older
    /// than the last it published, so the tests may run in any order.
    static uint32_t nextScene()
    {
        sceneSecs_ += 10;
        return sceneSecs_;
    }

    /// The vehicle at kSpeed from 50 ms before @p secs to 50 ms after, every 50 ms; one
    /// slice delivers it to the node before any cloud.
    static void sendTwists(uint32_t secs)
    {
        const std::array<builtin_interfaces::msg::Time, 3> stamps{
            stamp(secs - 1, 950'000'000), stamp(secs, 0), stamp(secs, kRightDelayNanos)
        };
        for (const auto& at : stamps) {
            geometry_msgs::msg::TwistWithCovarianceStamped twist;
            twist.header.frame_id = "base_link";
            twist.header.stamp = at;
            twist.twist.twist.linear.x = kSpeed;
            twist_->publish(twist);
        }
        ASSERT_TRUE(probe_->hypha_node().runFor(hypha::Duration::fromMillis(20)).ok());
    }

    /// Runs until the output, its info and a diagnostic arrived, and @p alsoRight when set.
    static Received collect(bool alsoRight)
    {
        for (int slice = 0; slice < 100; ++slice) {
            const bool done = received_->output && received_->info && received_->status
                           && received_->leftSynchronized
                           && (!alsoRight || received_->rightSynchronized);
            if (done) {
                break;
            }
            EXPECT_TRUE(probe_->hypha_node().runFor(hypha::Duration::fromMillis(20)).ok());
        }
        return *received_;
    }

    void SetUp() override
    {
        *received_ = Received{};
    }

    static inline std::shared_ptr<rclcpp::Node>                               node_;
    static inline std::shared_ptr<rclcpp::Node>                               probe_;
    static inline rclcpp::Publisher<sensor_msgs::msg::PointCloud2>::SharedPtr left_;
    static inline rclcpp::Publisher<sensor_msgs::msg::PointCloud2>::SharedPtr right_;
    static inline rclcpp::Publisher<geometry_msgs::msg::TwistWithCovarianceStamped>::SharedPtr
                                                                     twist_;
    static inline std::vector<std::shared_ptr<void>>                 subscriptions_;
    static inline std::unique_ptr<hypha::TransformBuffer>            tf_;
    static inline std::unique_ptr<hypha::StaticTransformBroadcaster> statics_;
    static inline std::shared_ptr<Received>                          received_;
    static inline uint32_t                                           sceneSecs_ = 90;
};

TEST_F(ConcatenateAndTimeSyncNode, ConcatenatesBothLidarsInBaseLinkWithMotionCompensation)
{
    const uint32_t secs = nextScene();
    sendTwists(secs);
    left_->publish(cloud(kLeftPoints, "left_lidar", stamp(secs, 0)));
    right_->publish(cloud(kRightPoints, "right_lidar", stamp(secs, kRightDelayNanos)));
    const Received received = collect(true);

    // The cloud carries the older stamp. The right cloud is 50 ms newer: at 10 m/s the
    // vehicle drove 0.5 m ahead, so its points are 0.5 m further ahead at the older stamp.
    ASSERT_TRUE(received.output.has_value()) << "no cloud on /test/concatenate/output";
    EXPECT_EQ(received.output->header.frame_id, "base_link");
    EXPECT_EQ(received.output->header.stamp.secs, secs);
    EXPECT_EQ(received.output->header.stamp.nsecs, 0U);
    const float ahead = static_cast<float>(kSpeed * 0.05);
    expectPoints(
        *received.output,
        {{1.0F, 1.0F, 0.0F},
         {2.0F, 1.0F, 0.0F},
         {3.0F, 1.0F, 0.0F},
         {1.0F + ahead, -1.0F, 0.0F},
         {2.0F + ahead, -1.0F, 0.0F}}
    );

    ASSERT_TRUE(received.info.has_value()) << "no info on /test/concatenate/output_info";
    EXPECT_TRUE(received.info->concatenation_success);
    EXPECT_EQ(received.info->matching_strategy, Info::STRATEGY_NAIVE);
    ASSERT_EQ(received.info->source_info.size(), 2U);
    const Source* left = sourceOf(*received.info, kLeft);
    const Source* right = sourceOf(*received.info, kRight);
    ASSERT_NE(left, nullptr);
    ASSERT_NE(right, nullptr);
    EXPECT_EQ(left->status, Source::STATUS_OK);
    EXPECT_EQ(right->status, Source::STATUS_OK);
    EXPECT_EQ(left->length, kLeftPoints.size());
    EXPECT_EQ(right->length, kRightPoints.size());
    EXPECT_EQ(right->header.stamp.nsecs, kRightDelayNanos);

    // Each lidar's synchronized cloud stays in its own frame.
    ASSERT_TRUE(received.leftSynchronized.has_value());
    ASSERT_TRUE(received.rightSynchronized.has_value());
    EXPECT_EQ(received.leftSynchronized->header.frame_id, "left_lidar");
    EXPECT_EQ(received.rightSynchronized->header.frame_id, "right_lidar");
    EXPECT_EQ(received.leftSynchronized->width * received.leftSynchronized->height, 3U);
    EXPECT_EQ(received.rightSynchronized->width * received.rightSynchronized->height, 2U);

    ASSERT_TRUE(received.status.has_value()) << "no diagnostic on /diagnostics";
    // DiagnosticsInterface keeps a message only above OK and sends "OK" when none is left.
    EXPECT_EQ(received.status->level, Status::OK);
    EXPECT_EQ(received.status->message, "OK");
    EXPECT_EQ(value(*received.status, "Pointcloud concatenation succeeded"), "True");
}

TEST_F(ConcatenateAndTimeSyncNode, PublishesTheCloudsThatArrivedWhenTheTimeoutEnds)
{
    // Only the left lidar sends: the collector's timer, created and reset inside a
    // callback (#90), ends the wait after timeout_sec.
    const uint32_t secs = nextScene();
    sendTwists(secs);
    left_->publish(cloud(kLeftPoints, "left_lidar", stamp(secs, 0)));
    const Received received = collect(false);

    ASSERT_TRUE(received.output.has_value()) << "no cloud on /test/concatenate/output";
    EXPECT_EQ(received.output->header.stamp.secs, secs);
    expectPoints(*received.output, {{1.0F, 1.0F, 0.0F}, {2.0F, 1.0F, 0.0F}, {3.0F, 1.0F, 0.0F}});

    ASSERT_TRUE(received.info.has_value());
    EXPECT_FALSE(received.info->concatenation_success);
    const Source* left = sourceOf(*received.info, kLeft);
    const Source* right = sourceOf(*received.info, kRight);
    ASSERT_NE(left, nullptr);
    ASSERT_NE(right, nullptr);
    EXPECT_EQ(left->status, Source::STATUS_OK);
    EXPECT_EQ(right->status, Source::STATUS_TIMEOUT);
    EXPECT_EQ(right->length, 0U);

    ASSERT_TRUE(received.status.has_value()) << "no diagnostic on /diagnostics";
    EXPECT_EQ(received.status->level, Status::ERROR);
    EXPECT_EQ(
        received.status->message, "Concatenated pointcloud is published but misses some topics"
    );
    EXPECT_EQ(value(*received.status, "Concatenated: " + kRight), "False");
}

int main(int argc, char** argv)
{
    testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
