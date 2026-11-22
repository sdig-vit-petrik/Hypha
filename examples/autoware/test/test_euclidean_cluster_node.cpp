/// @file
/// The upstream EuclideanClusterNode on the bus, through the rclcpp shim: its parameters
/// from TOML, a scene of two clusters, one too big and three lone points in; the two
/// clusters as objects, their debug cloud and an OK diagnostic out.

#include "euclidean_cluster_node.hpp"
#include "euclidean_cluster_scene.hpp"

#include <gtest/gtest.h>
#include <rclcpp/rclcpp.hpp>

#include <memory>

namespace {
constexpr const char* kParameterFile = "test/config/euclidean_cluster_node_test.toml";
}  // namespace

TEST(EuclideanClusterNode, FindsTheClustersWithinTheSizeLimits)
{
    const char* argv[] = {"test", kParameterFile};
    rclcpp::init(2, argv);

    auto node =
        std::make_shared<autoware::euclidean_cluster::EuclideanClusterNode>(rclcpp::NodeOptions());
    EXPECT_TRUE(hypha::config().unreadKeys().empty());

    const euclidean_cluster_test::Received received =
        euclidean_cluster_test::runScene("euclidean_cluster", "euclidean_cluster_node");

    ASSERT_TRUE(received.objects.has_value()) << "no objects on /test/euclidean_cluster/output";
    euclidean_cluster_test::expectTheTwoClusters(*received.objects);
    ASSERT_TRUE(received.clusters.has_value()) << "no debug cloud";
    euclidean_cluster_test::expectTheDebugCloud(*received.clusters);

    // PCL's extraction already drops a cluster above max_cluster_size, so upstream's node
    // never counts one as skipped: the big cluster is gone and the diagnostic is OK.
    ASSERT_TRUE(received.status.has_value()) << "no diagnostic on /diagnostics";
    EXPECT_EQ(received.status->level, diagnostic_msgs::msg::DiagnosticStatus::OK);
    EXPECT_EQ(received.status->message, "OK");
    EXPECT_EQ(received.status->hardware_id, "euclidean_cluster_node");
    EXPECT_EQ(
        euclidean_cluster_test::value(*received.status, "is_cluster_data_size_within_range"), "True"
    );
}

int main(int argc, char** argv)
{
    testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
