// The standard services of ROS 1 that the SDK ships beside the messages of their package
// (nav_msgs/GetMap, sensor_msgs/SetCameraInfo, ...): each under its ROS name and with the
// MD5 of ROS (the values of ROS 1's genmsg, also pinned in hypha_msgdef's golden_srv_md5.txt),
// a GetMap and a SetCameraInfo through a real server and client, and what an application
// adds to the package sensor_msgs without redefining the SDK's messages.

#include "test_types.h"

#include <diagnostic_msgs/AddDiagnostics.h>
#include <diagnostic_msgs/SelfTest.h>
#include <gtest/gtest.h>
#include <nav_msgs/GetMap.h>
#include <nav_msgs/GetPlan.h>
#include <nav_msgs/LoadMap.h>
#include <nav_msgs/SetMap.h>
#include <sensor_msgs/FocusState.h>
#include <sensor_msgs/SetCameraInfo.h>
#include <sensor_msgs/SetRegion.h>
#include <tf2_msgs/FrameGraph.h>

#include <chrono>
#include <cstdint>
#include <functional>
#include <optional>
#include <string>
#include <string_view>

namespace {

using hypha_test::testTopic;

struct Expected
{
    std::string_view typeName;
    std::string_view actualTypeName;
    std::string_view md5;
    std::string_view actualMd5;
};

TEST(StandardServices, EachShippedServiceHasTheTypeNameAndTheMd5OfRos)
{
    const Expected services[] = {
        {"diagnostic_msgs/AddDiagnostics",
         diagnostic_msgs::AddDiagnostics::TYPE_NAME,
         "e6ac9bbde83d0d3186523c3687aecaee",
         diagnostic_msgs::AddDiagnostics::MD5},
        {"diagnostic_msgs/SelfTest",
         diagnostic_msgs::SelfTest::TYPE_NAME,
         "ac21b1bab7ab17546986536c22eb34e9",
         diagnostic_msgs::SelfTest::MD5},
        {"nav_msgs/GetMap",
         nav_msgs::GetMap::TYPE_NAME,
         "6cdd0a18e0aff5b0a3ca2326a89b54ff",
         nav_msgs::GetMap::MD5},
        {"nav_msgs/GetPlan",
         nav_msgs::GetPlan::TYPE_NAME,
         "421c8ea4d21c6c9db7054b4bbdf1e024",
         nav_msgs::GetPlan::MD5},
        {"nav_msgs/LoadMap",
         nav_msgs::LoadMap::TYPE_NAME,
         "22e647fdfbe3b23c8c9f419908afaebd",
         nav_msgs::LoadMap::MD5},
        {"nav_msgs/SetMap",
         nav_msgs::SetMap::TYPE_NAME,
         "c36922319011e63ed7784112ad4fdd32",
         nav_msgs::SetMap::MD5},
        {"sensor_msgs/SetCameraInfo",
         sensor_msgs::SetCameraInfo::TYPE_NAME,
         "bef1df590ed75ed1f393692395e15482",
         sensor_msgs::SetCameraInfo::MD5},
        {"tf2_msgs/FrameGraph",
         tf2_msgs::FrameGraph::TYPE_NAME,
         "437ea58e9463815a0d511c7326b686b0",
         tf2_msgs::FrameGraph::MD5},
    };
    for (const Expected& service : services) {
        EXPECT_EQ(service.actualTypeName, service.typeName);
        EXPECT_EQ(service.actualMd5, service.md5) << service.typeName;
    }
}

TEST(StandardServices, TheConstantsOfLoadMapAreThoseOfRos)
{
    EXPECT_EQ(nav_msgs::LoadMapResponse::RESULT_SUCCESS, 0);
    EXPECT_EQ(nav_msgs::LoadMapResponse::RESULT_MAP_DOES_NOT_EXIST, 1);
    EXPECT_EQ(nav_msgs::LoadMapResponse::RESULT_INVALID_MAP_DATA, 2);
    EXPECT_EQ(nav_msgs::LoadMapResponse::RESULT_INVALID_MAP_METADATA, 3);
    EXPECT_EQ(nav_msgs::LoadMapResponse::RESULT_UNDEFINED_FAILURE, 255);
}

class StandardServiceFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        static_cast<void>(hypha::disableSimTime());
        this->node.emplace("standard_service_test");
    }

    template <typename Srv>
    hypha::CallResult<Srv>
    callAndRun(hypha::ServiceClient<Srv>& client, const typename Srv::Request& request)
    {
        std::optional<hypha::CallResult<Srv>> result;
        EXPECT_EQ(
            client.call(
                request,
                [&result](const hypha::CallResult<Srv>& r) {
                    result = r;
                }
            ),
            hypha::CallStatus::Sent
        );
        const auto until = std::chrono::steady_clock::now() + std::chrono::seconds(5);
        while (!result && std::chrono::steady_clock::now() < until) {
            static_cast<void>(this->node->runFor(hypha::Duration::fromMillis(5)));
        }
        EXPECT_TRUE(result.has_value()) << "no outcome within 5 s";
        return result.value_or(hypha::CallResult<Srv>::notSent(hypha::CallStatus::Busy));
    }

    std::optional<hypha::Node> node;
};

TEST_F(StandardServiceFixture, AGetMapRequestIsAnsweredWithTheMapOfTheServer)
{
    nav_msgs::OccupancyGrid map;
    map.header.frame_id = "map";
    map.info.resolution = 0.05F;
    map.info.width = 16;
    map.info.height = 8;
    map.info.origin.position.x = -4.0;
    map.info.origin.orientation.w = 1.0;
    map.data.resize(16U * 8U);
    for (std::size_t cell = 0; cell < map.data.size(); ++cell) {
        map.data[cell] = static_cast<int8_t>(cell % 3U == 0U ? 100 : (cell % 5U == 0U ? -1 : 0));
    }

    auto server = this->node->advertiseService<nav_msgs::GetMap>(
        hypha::Service{testTopic("/static_map")},
        [&map](hypha::Request<nav_msgs::GetMap>& request) {
            static_cast<void>(request.reply({.map = map}));
        }
    );
    auto client =
        this->node->serviceClient<nav_msgs::GetMap>(hypha::Service{testTopic("/static_map")});

    const auto result = this->callAndRun(client, {});
    ASSERT_TRUE(result.ok()) << result.description();
    EXPECT_EQ(result.response().map, map);
    EXPECT_EQ(result.response().map.header.frame_id, "map");
    EXPECT_EQ(result.response().map.data.size(), 16U * 8U);
}

TEST_F(StandardServiceFixture, ASetCameraInfoRequestReachesTheServerWithItsCalibration)
{
    sensor_msgs::CameraInfo seen;
    auto                    server = this->node->advertiseService<sensor_msgs::SetCameraInfo>(
        hypha::Service{testTopic("/set_camera_info")},
        [&seen](hypha::Request<sensor_msgs::SetCameraInfo>& request) {
            seen = request->camera_info;
            static_cast<void>(request.reply({.success = true, .status_message = "stored"}));
        }
    );
    auto client = this->node->serviceClient<sensor_msgs::SetCameraInfo>(
        hypha::Service{testTopic("/set_camera_info")}
    );

    sensor_msgs::SetCameraInfo::Request request;
    request.camera_info.width = 640;
    request.camera_info.height = 480;
    request.camera_info.distortion_model = "plumb_bob";
    request.camera_info.D = {0.1, -0.2, 0.0, 0.0, 0.01};
    request.camera_info.K = {500.0, 0.0, 320.0, 0.0, 500.0, 240.0, 0.0, 0.0, 1.0};

    const auto result = this->callAndRun(client, request);
    ASSERT_TRUE(result.ok()) << result.description();
    EXPECT_TRUE(result.response().success);
    EXPECT_EQ(result.response().status_message, "stored");
    EXPECT_EQ(seen, request.camera_info);
}

TEST_F(StandardServiceFixture, AServiceAddedToSensorMsgsWorksBesideTheMessagesOfTheSdk)
{
    // sensor_msgs/SetRegion and sensor_msgs/FocusState are the application's own
    // (test/srv/sensor_msgs, test/msg/sensor_msgs); RegionOfInterest is the SDK's.
    static_assert(std::string_view(sensor_msgs::SetRegion::TYPE_NAME) == "sensor_msgs/SetRegion");
    static_assert(std::string_view(sensor_msgs::FocusState::TYPE_NAME) == "sensor_msgs/FocusState");
    EXPECT_NE(std::string_view(sensor_msgs::SetRegion::MD5), sensor_msgs::SetCameraInfo::MD5);

    auto server = this->node->advertiseService<sensor_msgs::SetRegion>(
        hypha::Service{testTopic("/set_region")},
        [](hypha::Request<sensor_msgs::SetRegion>& request) {
            static_cast<void>(request.reply({.success = request->roi.width > 0}));
        }
    );
    auto client =
        this->node->serviceClient<sensor_msgs::SetRegion>(hypha::Service{testTopic("/set_region")});
    sensor_msgs::RegionOfInterest roi;
    roi.width = 64;
    const auto result = this->callAndRun(client, {.roi = roi});
    ASSERT_TRUE(result.ok()) << result.description();
    EXPECT_TRUE(result.response().success);

    sensor_msgs::FocusState focus;
    focus.focus = roi;
    focus.sharpness = 0.5F;
    EXPECT_EQ(focus.focus.width, 64U);
}

}  // namespace
