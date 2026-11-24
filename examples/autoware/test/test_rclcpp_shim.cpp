/// @file
/// The rclcpp shim: names, time, the QoS check, timers and services.

#include <hypha_msgs/sensor_msgs/PointCloud2.h>
#include <hypha_rclcpp_msgs/ServiceCall.h>

#include <gtest/gtest.h>
#include <rclcpp/rclcpp.hpp>
#include <std_srvs/srv/set_bool.hpp>
#include <tf2_ros/transform_broadcaster.hpp>

#include <limits>
#include <stdexcept>

TEST(RclcppShim, RelativeNamesArePorts)
{
    EXPECT_EQ(rclcpp::detail::portName("output"), "output");
    EXPECT_EQ(rclcpp::detail::portName("~/crop_box_polygon"), "crop_box_polygon");
    EXPECT_EQ(
        rclcpp::detail::portName("crop_box_filter/debug/processing_time_ms"),
        "crop_box_filter.debug.processing_time_ms"
    );
    EXPECT_THROW((void)rclcpp::detail::portName("/absolute"), std::invalid_argument);
    EXPECT_THROW((void)rclcpp::detail::portName("~/"), std::invalid_argument);
}

TEST(RclcppShim, NodeNamesBecomeHyphaNames)
{
    EXPECT_EQ(rclcpp::detail::hyphaNodeName("crop_box_filter"), "crop_box_filter");
    EXPECT_EQ(
        rclcpp::detail::hyphaNodeName("VoxelGridDownsampleFilter"), "voxel_grid_downsample_filter"
    );
    EXPECT_EQ(rclcpp::detail::hyphaNodeName("ndt_scan_matcher2"), "ndt_scan_matcher2");
    EXPECT_THROW((void)rclcpp::detail::hyphaNodeName("my-node"), std::invalid_argument);
    EXPECT_THROW((void)rclcpp::detail::hyphaNodeName("_hidden"), std::invalid_argument);
    EXPECT_THROW((void)rclcpp::detail::hyphaNodeName(""), std::invalid_argument);
}

TEST(RclcppShim, TimeRoundTripsThroughAStamp)
{
    builtin_interfaces::msg::Time stamp;
    stamp.secs = 12;
    stamp.nsecs = 345;
    const rclcpp::Time time(stamp);
    EXPECT_EQ(time.nanoseconds(), 12000000345);
    const builtin_interfaces::msg::Time back = time;
    EXPECT_EQ(back.secs, 12U);
    EXPECT_EQ(back.nsecs, 345U);
    EXPECT_EQ((rclcpp::Time(13, 0) - time).nanoseconds(), 999999655);
    EXPECT_EQ(rclcpp::Duration::from_seconds(1.5).nanoseconds(), 1500000000);
}

TEST(RclcppShim, ADurationFromNonFiniteSecondsIsRefused)
{
    const double nan = std::numeric_limits<double>::quiet_NaN();
    const double inf = std::numeric_limits<double>::infinity();
    EXPECT_THROW(static_cast<void>(rclcpp::Duration::from_seconds(nan)), std::invalid_argument);
    EXPECT_THROW(static_cast<void>(rclcpp::Duration::from_seconds(-inf)), std::invalid_argument);
}

TEST(RclcppShim, AStampWithTooManyNanosecondsIsRefused)
{
    builtin_interfaces::msg::Time stamp;
    stamp.nsecs = 1000000000;
    EXPECT_THROW(rclcpp::Time{stamp}, std::invalid_argument);
}

TEST(RclcppShim, ADepthAboveThePolicyIsRefused)
{
    EXPECT_EQ(rclcpp::detail::checkedDepth<sensor_msgs::PointCloud2>("x", rclcpp::QoS(5)), 5U);
    EXPECT_THROW(
        (void)rclcpp::detail::checkedDepth<sensor_msgs::PointCloud2>("x", rclcpp::QoS(1000)),
        std::invalid_argument
    );
    EXPECT_THROW(
        (void)rclcpp::detail::checkedDepth<sensor_msgs::PointCloud2>("x", rclcpp::QoS(0)),
        std::invalid_argument
    );
}

TEST(RclcppShim, PrintfFormatting)
{
    EXPECT_EQ(rclcpp::detail::formatPrintf("%d points in %s", 3, "lidar"), "3 points in lidar");
}

TEST(RclcppShim, ABroadcasterRefusesANanTransformAndCountsIt)
{
    auto                                 node = rclcpp::Node::make_shared("tf_broadcaster_probe");
    tf2_ros::TransformBroadcaster        broadcaster(*node);
    geometry_msgs::msg::TransformStamped pose;
    pose.header.frame_id = "map";
    pose.child_frame_id = "base_link";
    pose.header.stamp.secs = 1;
    pose.transform.rotation.w = 1.0;
    broadcaster.sendTransform(pose);
    EXPECT_EQ(broadcaster.failed_publish_count(), 0U);
    pose.transform.translation.x = std::numeric_limits<double>::quiet_NaN();
    broadcaster.sendTransform(pose);
    EXPECT_EQ(broadcaster.failed_publish_count(), 1U);
}

TEST(RclcppShim, ATimerCreatedInACallbackIsCanceledAndReset)
{
    auto                         node = rclcpp::Node::make_shared("timer_probe");
    rclcpp::TimerBase::SharedPtr late;
    int                          lateCalls = 0;
    // As a cloud collector does: the timer is created in a callback, canceled at once,
    // and reset when work arrives.
    auto starter = rclcpp::create_timer(node, node->get_clock(), std::chrono::milliseconds(1), [&] {
        if (late) {
            return;
        }
        late = rclcpp::create_timer(node, node->get_clock(), std::chrono::milliseconds(2), [&] {
            ++lateCalls;
            late->cancel();
        });
        late->cancel();
        EXPECT_TRUE(late->is_canceled());
        EXPECT_EQ(late->time_until_trigger(), std::chrono::nanoseconds::max());
        late->reset();
        EXPECT_FALSE(late->is_canceled());
        EXPECT_LE(late->time_until_trigger(), std::chrono::milliseconds(2));
    });
    // Only outcomes: the late timer fired once and canceled itself.
    for (int slice = 0; slice < 200 && lateCalls == 0; ++slice) {
        ASSERT_TRUE(node->hypha_node().runFor(hypha::Duration::fromMillis(10)).ok());
    }
    ASSERT_TRUE(late);
    EXPECT_EQ(lateCalls, 1);
    EXPECT_TRUE(late->is_canceled());
}

namespace {

/// Runs both nodes in turns until @p done or 200 turns; only outcomes are checked.
template <class Done>
void runUntil(rclcpp::Node& first, rclcpp::Node& second, Done done)
{
    for (int turn = 0; turn < 200 && !done(); ++turn) {
        ASSERT_TRUE(first.hypha_node().runFor(hypha::Duration::fromMillis(5)).ok());
        ASSERT_TRUE(second.hypha_node().runFor(hypha::Duration::fromMillis(5)).ok());
    }
}

/// A SetBool server that answers success = data and counts its calls.
struct EchoServer
{
    explicit EchoServer(rclcpp::Node& node)
        : service(node.create_service<std_srvs::srv::SetBool>(
              "/shim_test/echo",
              [this](
                  const std::shared_ptr<const std_srvs::srv::SetBool::Request>& request,
                  const std::shared_ptr<std_srvs::srv::SetBool::Response>&      response
              ) {
                  ++this->calls;
                  response->success = request->data;
                  response->message = request->data ? "on" : "off";
              }
          ))
    {
    }

    int                                                calls{0};
    rclcpp::Service<std_srvs::srv::SetBool>::SharedPtr service;
};

}  // namespace

TEST(RclcppShim, ServiceNamesResolveAsInTheRootNamespace)
{
    EXPECT_EQ(rclcpp::detail::serviceName("ekf_localizer", "/a/b"), "/a/b");
    EXPECT_EQ(
        rclcpp::detail::serviceName("ekf_localizer", "~/config_logger"),
        "/ekf_localizer/config_logger"
    );
    // No parameter file sets services.trigger_node_srv here.
    EXPECT_EQ(
        rclcpp::detail::serviceName("ekf_localizer", "trigger_node_srv"), "/trigger_node_srv"
    );
    EXPECT_THROW((void)rclcpp::detail::serviceName("ekf_localizer", ""), std::invalid_argument);
}

TEST(RclcppShim, AServiceCallIsAnsweredThroughTheCallback)
{
    auto serverNode = rclcpp::Node::make_shared("echo_server");
    auto clientNode = rclcpp::Node::make_shared("echo_client");
    auto client = clientNode->create_client<std_srvs::srv::SetBool>("/shim_test/echo");
    EXPECT_FALSE(client->service_is_ready());
    EchoServer server(*serverNode);
    EXPECT_TRUE(client->service_is_ready());

    auto request = std::make_shared<std_srvs::srv::SetBool::Request>();
    request->data = true;
    int                                               answered = 0;
    std::shared_ptr<std_srvs::srv::SetBool::Response> seen;
    auto                                              future = client->async_send_request(
        request, [&](rclcpp::Client<std_srvs::srv::SetBool>::SharedFuture done) {
            ++answered;
            seen = done.get();
        }
    );
    runUntil(*serverNode, *clientNode, [&] {
        return answered > 0;
    });

    ASSERT_EQ(answered, 1);
    EXPECT_EQ(server.calls, 1);
    ASSERT_TRUE(seen);
    EXPECT_TRUE(seen->success);
    EXPECT_EQ(seen->message, "on");
    ASSERT_EQ(future.wait_for(std::chrono::seconds(0)), std::future_status::ready);
    EXPECT_EQ(future.get(), seen);
    EXPECT_EQ(client->prune_pending_requests(), 0U);
}

TEST(RclcppShim, EachClientTakesOnlyItsOwnResponse)
{
    auto       serverNode = rclcpp::Node::make_shared("echo_server_two");
    auto       clientNode = rclcpp::Node::make_shared("echo_clients");
    EchoServer server(*serverNode);
    auto       on = clientNode->create_client<std_srvs::srv::SetBool>("/shim_test/echo");
    auto       off = clientNode->create_client<std_srvs::srv::SetBool>("/shim_test/echo");

    auto onRequest = std::make_shared<std_srvs::srv::SetBool::Request>();
    onRequest->data = true;
    auto offRequest = std::make_shared<std_srvs::srv::SetBool::Request>();
    offRequest->data = false;
    auto onCall = on->async_send_request(onRequest);
    auto offCall = off->async_send_request(offRequest);
    // Both clients send id 1: only the identity tells their responses apart.
    EXPECT_EQ(onCall.request_id, offCall.request_id);
    const auto ready = [](auto& call) {
        return call.future.wait_for(std::chrono::seconds(0)) == std::future_status::ready;
    };
    runUntil(*serverNode, *clientNode, [&] {
        return ready(onCall) && ready(offCall);
    });

    ASSERT_TRUE(ready(onCall));
    ASSERT_TRUE(ready(offCall));
    EXPECT_EQ(server.calls, 2);
    EXPECT_TRUE(onCall.get()->success);
    EXPECT_FALSE(offCall.get()->success);
    EXPECT_EQ(on->prune_pending_requests(), 0U);
    EXPECT_EQ(off->prune_pending_requests(), 0U);
}

TEST(RclcppShim, ARequestThatDoesNotDecodeIsAnsweredUnhandled)
{
    auto       serverNode = rclcpp::Node::make_shared("echo_server_three");
    auto       probeNode = rclcpp::Node::make_shared("raw_caller");
    EchoServer server(*serverNode);
    std::vector<hypha_rclcpp_msgs::ServiceCall> answers;
    auto responses = probeNode->hypha_node().subscribe<hypha_rclcpp_msgs::ServiceCall>(
        hypha::Topic{"/shim_test/echo/response"},
        [&](const hypha_rclcpp_msgs::ServiceCall::ConstPtr& call) {
            answers.push_back(*call);
        }
    );
    auto requests = probeNode->hypha_node().advertise<hypha_rclcpp_msgs::ServiceCall>(
        hypha::Topic{"/shim_test/echo/request"}
    );
    hypha_rclcpp_msgs::ServiceCall garbage;
    garbage.caller = "raw_caller/probe";
    garbage.id = 7;
    garbage.payload = {1, 2, 3};  // a SetBool request is one byte
    ASSERT_TRUE(requests.publish(garbage));
    runUntil(*serverNode, *probeNode, [&] {
        return !answers.empty();
    });

    ASSERT_EQ(answers.size(), 1U);
    EXPECT_EQ(answers.front().caller, "raw_caller/probe");
    EXPECT_EQ(answers.front().id, 7U);
    EXPECT_FALSE(answers.front().handled);
    EXPECT_TRUE(answers.front().payload.empty());
    EXPECT_EQ(server.calls, 0);
}
