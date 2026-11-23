/// @file
/// The upstream MapProjectionLoader on the bus, through the rclcpp shim: it reads the map
/// projection from upstream's local Cartesian UTM test file and publishes it once on
/// /map/map_projector_info, transient local as autoware_component_interface_specs declares
/// it. A subscriber created after the node still receives it. Upstream's node tests are
/// launch tests in Python.

#include "map_projection_loader_node.hpp"

#include <autoware_map_msgs/msg/map_projector_info.hpp>
#include <gtest/gtest.h>
#include <rclcpp/rclcpp.hpp>

#include <memory>
#include <optional>

namespace {

constexpr const char* kParameterFile = "test/config/map_projection_loader_node_test.toml";
using ProjectorInfo = autoware_map_msgs::msg::MapProjectorInfo;

}  // namespace

TEST(MapProjectionLoaderNode, ALateSubscriberReceivesTheProjection)
{
    auto loader = std::make_shared<autoware::map_projection_loader::MapProjectionLoader>(
        rclcpp::NodeOptions{}
    );
    auto                         harness = rclcpp::Node::make_shared("projection_test_harness");
    std::optional<ProjectorInfo> received;
    auto                         subscription = harness->hypha_node().subscribe<ProjectorInfo>(
        hypha::Topic{"/map/map_projector_info"}, [&](const ProjectorInfo::ConstPtr& info) {
            received = *info;
        }
    );
    for (int i = 0; i < 20 && !received; ++i) {
        rclcpp::spin_some(harness);
        rclcpp::spin_some(loader);
    }

    ASSERT_TRUE(received.has_value());
    EXPECT_EQ(received->projector_type, ProjectorInfo::LOCAL_CARTESIAN_UTM);
    EXPECT_EQ(received->vertical_datum, ProjectorInfo::WGS84);
    EXPECT_DOUBLE_EQ(received->map_origin.latitude, 35.6762);
    EXPECT_DOUBLE_EQ(received->map_origin.longitude, 139.6503);
    EXPECT_DOUBLE_EQ(received->map_origin.altitude, 0.0);
    // LocalCartesianUTM takes UTM's scale factor, whatever the file says.
    EXPECT_FLOAT_EQ(received->scale_factor, 0.9996F);
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
