/// @file
/// The upstream GNSSPoser on the bus, through the rclcpp shim, with the deployment
/// configuration: the map projection on the latched /map/map_projector_info, a NavSatFix
/// and the GNSS/INS orientation in; the pose, its covariance, the fixed flag and
/// map -> gnss_base_link out. The scenarios follow upstream's test_gnss_poser_node.cpp,
/// which needs rclcpp executors, with exact expectations: x and y are GeographicLib's local
/// Cartesian projection of the fix about the map origin, z its altitude above the origin's.

#include "gnss_poser_node.hpp"

#include <GeographicLib/LocalCartesian.hpp>
#include <autoware_internal_debug_msgs/msg/bool_stamped.hpp>
#include <autoware_map_msgs/msg/map_projector_info.hpp>
#include <autoware_sensing_msgs/msg/gnss_ins_orientation_stamped.hpp>
#include <geometry_msgs/msg/pose_stamped.hpp>
#include <geometry_msgs/msg/pose_with_covariance_stamped.hpp>
#include <gtest/gtest.h>
#include <hypha/transform_buffer.h>
#include <rclcpp/rclcpp.hpp>
#include <sensor_msgs/msg/nav_sat_fix.hpp>
#include <tf2_msgs/msg/tf_message.hpp>

#include <functional>
#include <memory>
#include <optional>
#include <string>

namespace {

constexpr const char* kParameterFile = "test/config/gnss_poser_node_test.toml";
/// The map origin and a fix 0.001 degrees north-east of it, as upstream's Tokyo fixes.
constexpr double kOriginLatitude = 35.681236;
constexpr double kOriginLongitude = 139.767125;
constexpr double kLatitude = 35.682236;
constexpr double kLongitude = 139.768125;
constexpr double kAltitude = 41.0;
constexpr double kNear = 1e-6;

using Fix = sensor_msgs::msg::NavSatFix;
using Orientation = autoware_sensing_msgs::msg::GnssInsOrientationStamped;
using ProjectorInfo = autoware_map_msgs::msg::MapProjectorInfo;
using Pose = geometry_msgs::msg::PoseStamped;
using PoseCov = geometry_msgs::msg::PoseWithCovarianceStamped;
using Fixed = autoware_internal_debug_msgs::msg::BoolStamped;

class GnssPoserNodeTest : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->poser_ = std::make_shared<autoware::gnss_poser::GNSSPoser>(rclcpp::NodeOptions{});
        this->harness_ = rclcpp::Node::make_shared("gnss_test_harness");
        hypha::Node& bus = this->harness_->hypha_node();
        this->projector_ =
            bus.advertise<ProjectorInfo>(hypha::Topic{"/map/map_projector_info"}, hypha::latched);
        this->fix_ = bus.advertise<Fix>(hypha::Topic{"/sensing/gnss/ublox/nav_sat_fix"});
        this->orientation_ = bus.advertise<Orientation>(hypha::Topic{"/autoware_orientation"});
        this->tfStatic_.emplace(bus);
        this->poseIn_ = bus.subscribe<Pose>(
            hypha::Topic{"/sensing/gnss/pose"}, [this](const Pose::ConstPtr& m) { this->pose_ = *m; }
        );
        this->poseCovIn_ = bus.subscribe<PoseCov>(
            hypha::Topic{"/sensing/gnss/pose_with_covariance"},
            [this](const PoseCov::ConstPtr& m) { this->poseCov_ = *m; }
        );
        this->fixedIn_ = bus.subscribe<Fixed>(
            hypha::Topic{"/sensing/gnss/fixed"}, [this](const Fixed::ConstPtr& m) { this->fixed_ = *m; }
        );
        this->tfIn_ = bus.subscribe<tf2_msgs::msg::TFMessage>(
            hypha::Topic{"/tf"},
            [this](const tf2_msgs::msg::TFMessage::ConstPtr& message) {
                for (const auto& transform : message->transforms) {
                    if (transform.child_frame_id == "gnss_base_link") {
                        this->gnssBaseLink_ = transform;
                    }
                }
            }
        );
    }

    /// Both nodes take dispatch turns until @p done, at most 20 times each.
    void turnUntil(const std::function<bool()>& done)
    {
        for (int i = 0; i < 20 && !done(); ++i) {
            rclcpp::spin_some(this->harness_);
            rclcpp::spin_some(this->poser_);
        }
    }

    void sendProjector()
    {
        ProjectorInfo info;
        info.projector_type = ProjectorInfo::LOCAL_CARTESIAN;
        info.vertical_datum = ProjectorInfo::WGS84;
        info.map_origin.latitude = kOriginLatitude;
        info.map_origin.longitude = kOriginLongitude;
        info.map_origin.altitude = 0.0;
        info.scale_factor = 1.0F;
        ASSERT_TRUE(this->projector_.publish(info));
        this->turnUntil([] { return false; });
    }

    /// A u-blox INS heading: @p yaw about z, with the rotation errors 0.1, 0.2, 0.3 rad.
    void sendOrientation(double yaw)
    {
        Orientation orientation;
        orientation.header.frame_id = "base_link";
        orientation.orientation.orientation.z = std::sin(yaw / 2.0);
        orientation.orientation.orientation.w = std::cos(yaw / 2.0);
        orientation.orientation.rmse_rotation_x = 0.1F;
        orientation.orientation.rmse_rotation_y = 0.2F;
        orientation.orientation.rmse_rotation_z = 0.3F;
        ASSERT_TRUE(this->orientation_.publish(orientation));
        this->turnUntil([] { return false; });
    }

    /// A fix at @p frame with the position variances 2, 3, 4 m^2 on the diagonal.
    void sendFix(int8_t status, const std::string& frame = "base_link")
    {
        Fix fix;
        fix.header.frame_id = frame;
        fix.header.stamp = rclcpp::Time(int64_t{100'000'000'000});
        fix.status.status = status;
        fix.latitude = kLatitude;
        fix.longitude = kLongitude;
        fix.altitude = kAltitude;
        fix.position_covariance = {2.0, 0.0, 0.0, 0.0, 3.0, 0.0, 0.0, 0.0, 4.0};
        fix.position_covariance_type = Fix::COVARIANCE_TYPE_DIAGONAL_KNOWN;
        ASSERT_TRUE(this->fix_.publish(fix));
        this->turnUntil([this] { return this->fixed_.has_value(); });
    }

    /// GeographicLib's local Cartesian coordinates of the fix about the map origin.
    static std::array<double, 3> expectedPosition()
    {
        const GeographicLib::LocalCartesian projection(kOriginLatitude, kOriginLongitude, 0.0);
        std::array<double, 3> xyz{};
        projection.Forward(kLatitude, kLongitude, kAltitude, xyz[0], xyz[1], xyz[2]);
        return xyz;
    }

    std::shared_ptr<autoware::gnss_poser::GNSSPoser>     poser_;
    rclcpp::Node::SharedPtr                              harness_;
    hypha::Publisher<ProjectorInfo>                      projector_;
    hypha::Publisher<Fix>                                fix_;
    hypha::Publisher<Orientation>                        orientation_;
    std::optional<hypha::StaticTransformBroadcaster>     tfStatic_;
    hypha::Subscriber<Pose>                              poseIn_;
    hypha::Subscriber<PoseCov>                           poseCovIn_;
    hypha::Subscriber<Fixed>                             fixedIn_;
    hypha::Subscriber<tf2_msgs::msg::TFMessage>          tfIn_;
    std::optional<Pose>                                  pose_;
    std::optional<PoseCov>                               poseCov_;
    std::optional<Fixed>                                 fixed_;
    std::optional<geometry_msgs::msg::TransformStamped>  gnssBaseLink_;
};

}  // namespace

// Upstream's TestFixedGNSS and TestCovarianceProcessing: a fixed fix becomes the pose of
// gnss_base_link in map, with the INS heading, the fix's variances and the INS errors
// squared, and map -> gnss_base_link on /tf.
TEST_F(GnssPoserNodeTest, AFixedFixBecomesTheMapPose)
{
    this->sendProjector();
    this->sendOrientation(M_PI / 2.0);
    this->sendFix(sensor_msgs::msg::NavSatStatus::STATUS_FIX);

    ASSERT_TRUE(this->fixed_.has_value());
    EXPECT_TRUE(this->fixed_->data);
    this->turnUntil([this] { return this->pose_ && this->poseCov_ && this->gnssBaseLink_; });
    ASSERT_TRUE(this->pose_ && this->poseCov_ && this->gnssBaseLink_);
    const std::array<double, 3> expected = expectedPosition();
    EXPECT_EQ(this->pose_->header.frame_id, "map");
    EXPECT_NEAR(this->pose_->pose.position.x, expected[0], kNear);
    EXPECT_NEAR(this->pose_->pose.position.y, expected[1], kNear);
    // project_forward replaces the projector's z with the altitude above the map origin's.
    EXPECT_NEAR(this->pose_->pose.position.z, kAltitude - 0.0, kNear);
    EXPECT_GT(this->pose_->pose.position.x, 80.0);   // about 90 m east
    EXPECT_GT(this->pose_->pose.position.y, 100.0);  // about 111 m north
    EXPECT_NEAR(this->pose_->pose.orientation.z, std::sin(M_PI / 4.0), kNear);
    EXPECT_NEAR(this->pose_->pose.orientation.w, std::cos(M_PI / 4.0), kNear);
    const auto& covariance = this->poseCov_->pose.covariance;
    EXPECT_NEAR(covariance[0], 2.0, kNear);
    EXPECT_NEAR(covariance[7], 3.0, kNear);
    EXPECT_NEAR(covariance[14], 4.0, kNear);
    EXPECT_NEAR(covariance[21], 0.01, 1e-6);
    EXPECT_NEAR(covariance[28], 0.04, 1e-6);
    EXPECT_NEAR(covariance[35], 0.09, 1e-6);
    EXPECT_EQ(this->gnssBaseLink_->header.frame_id, "map");
    EXPECT_NEAR(this->gnssBaseLink_->transform.translation.x, expected[0], kNear);
}

// Upstream's TestNonFixedGNSS: a fix without a fix is flagged and gives no pose.
TEST_F(GnssPoserNodeTest, AFixWithoutAFixGivesNoPose)
{
    this->sendProjector();
    this->sendOrientation(0.0);
    this->sendFix(sensor_msgs::msg::NavSatStatus::STATUS_NO_FIX);

    ASSERT_TRUE(this->fixed_.has_value());
    EXPECT_FALSE(this->fixed_->data);
    EXPECT_FALSE(this->pose_.has_value());
}

// Upstream's TestStaticTransform: an antenna 1 m ahead of base_link, heading east, puts
// base_link 1 m west of the projected antenna.
TEST_F(GnssPoserNodeTest, TheAntennaMountMovesThePoseToBaseLink)
{
    geometry_msgs::msg::TransformStamped mount;
    mount.header.frame_id = "base_link";
    mount.child_frame_id = "gnss_link";
    mount.transform.translation.x = 1.0;
    mount.transform.rotation.w = 1.0;
    ASSERT_TRUE(this->tfStatic_->send(mount));
    this->sendProjector();
    this->sendOrientation(0.0);
    this->sendFix(sensor_msgs::msg::NavSatStatus::STATUS_FIX, "gnss_link");

    this->turnUntil([this] { return this->pose_.has_value(); });
    ASSERT_TRUE(this->pose_.has_value());
    const std::array<double, 3> expected = expectedPosition();
    EXPECT_NEAR(this->pose_->pose.position.x, expected[0] - 1.0, kNear);
    EXPECT_NEAR(this->pose_->pose.position.y, expected[1], kNear);
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
