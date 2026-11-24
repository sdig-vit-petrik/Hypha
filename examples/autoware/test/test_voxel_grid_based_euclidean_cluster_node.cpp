/// @file
/// The upstream VoxelGridBasedEuclideanClusterNode on the bus, through the rclcpp shim:
/// its parameters from TOML, a scene of two clusters, one too big and three lone points
/// in; the two clusters as objects, their debug cloud and a WARN diagnostic for the big
/// cluster out.

#include "euclidean_cluster_scene.hpp"
#include "voxel_grid_based_euclidean_cluster_node.hpp"

#include <gtest/gtest.h>
#include <rclcpp/rclcpp.hpp>

#include <memory>

namespace {
constexpr const char* kParameterFile =
    "test/config/voxel_grid_based_euclidean_cluster_node_test.toml";
}  // namespace

TEST(VoxelGridBasedEuclideanClusterNode, FindsTheClustersAndReportsTheBigOne)
{
    const char* argv[] = {"test", kParameterFile};
    rclcpp::init(2, argv);

    auto node = std::make_shared<autoware::euclidean_cluster::VoxelGridBasedEuclideanClusterNode>(
        rclcpp::NodeOptions()
    );
    EXPECT_TRUE(hypha::config().unreadKeys().empty());

    const euclidean_cluster_test::Received received = euclidean_cluster_test::runScene(
        "voxel_grid_based_euclidean_cluster", "voxel_grid_based_euclidean_cluster_node"
    );

    ASSERT_TRUE(received.objects.has_value())
        << "no objects on /test/voxel_grid_based_euclidean_cluster/output";
    euclidean_cluster_test::expectTheTwoClusters(*received.objects);
    ASSERT_TRUE(received.clusters.has_value()) << "no debug cloud";
    euclidean_cluster_test::expectTheDebugCloud(*received.clusters);

    // The node buckets the raw points of each centroid cluster itself and counts the big
    // one as skipped.
    ASSERT_TRUE(received.status.has_value()) << "no diagnostic on /diagnostics";
    EXPECT_EQ(received.status->level, diagnostic_msgs::msg::DiagnosticStatus::WARN);
    EXPECT_EQ(
        received.status->message,
        "1 clusters skipped because cluster point size exceeds the maximum allowed 50"
    );
    EXPECT_EQ(
        euclidean_cluster_test::value(*received.status, "is_cluster_data_size_within_range"),
        "False"
    );
}

int main(int argc, char** argv)
{
    testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
