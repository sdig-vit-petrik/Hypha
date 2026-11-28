// Compile-only check: the application examples of docs/ros1-migration-guide.md section 10
// build as the guide prints them (10.2 and 10.6). Nothing here runs. Keep the two classes
// below equal to the guide: copy them from it, without its `#include` lines and `main`.

#include "hypha/hypha.h"
#include "hypha/transform_buffer.h"
#include "hypha_log/hypha_log.h"  // test_hypha defines HYPHA_NO_LOGGING; hypha.h includes it

#include <geometry_msgs/PoseStamped.h>
#include <gtest/gtest.h>
#include <sensor_msgs/PointCloud2.h>

#include <exception>
#include <optional>
#include <stop_token>
#include <thread>
#include <type_traits>

namespace {

class Locator final : public hypha::ApplicationNode
{
  public:
    // The bus is not joined yet. Read and check the parameters here: a refused value
    // ends the run with exit 78.
    Locator() : gain_(hypha::config().required<double>("gain", hypha::constraints::positive()))
    {
    }

    // Create every subscription, publisher, timer and transform buffer here, and
    // nowhere else. The members hold them.
    void setup(hypha::Node& node) override
    {
        this->scan_ = node.subscribe<sensor_msgs::PointCloud2>(
            "scan", hypha::bind<&Locator::onScan>(*this)
        );  // [ports.inputs.scan]
        this->pose_ = node.advertise<geometry_msgs::PoseStamped>("pose");  // [ports.outputs.pose]
        this->timer_ =
            node.timer(hypha::Duration::fromMillis(100), hypha::bind<&Locator::onTick>(*this));
        this->tf_ = hypha::TransformBuffer(node);  // a member like the others, assigned here
    }

  private:
    void onScan(const hypha::MessagePtr<sensor_msgs::PointCloud2>& scan)
    {
        geometry_msgs::PoseStamped pose;
        pose.header = scan->header;
        pose.pose.position.x = this->gain_ * static_cast<double>(scan->width);  // the node's logic
        this->pose_.publishOrWarn(pose);  // a refusal is logged: topic, status, once a second
    }

    void onTick()
    {
        // periodic work
    }

    double                                       gain_;
    hypha::Subscriber<sensor_msgs::PointCloud2>  scan_;
    hypha::Publisher<geometry_msgs::PoseStamped> pose_;
    hypha::Timer                                 timer_;
    hypha::TransformBuffer                       tf_;
};

sensor_msgs::PointCloud2 readFromDevice();  // the driver's blocking read, with a timeout

class LidarNode final : public hypha::ApplicationNode
{
  public:
    ~LidarNode() override
    {
        if (this->worker_.joinable()) {
            this->worker_.join();  // the stop is already requested
        }
    }

    void setup(hypha::Node& node) override
    {
        this->points_ = node.advertise<sensor_msgs::PointCloud2>("points");
    }

    void started(hypha::Node& /*node*/) override
    {
        this->worker_ =
            std::thread(&LidarNode::readLoop, this, this->stopToken());  // after the checks
    }

  private:
    // The only user of points_ once the thread runs.
    void readLoop(std::stop_token stop)
    {
        hypha::Rate rate(10.0, stop);  // ros::Rate rate(10)
        do {
            try {
                const bool sent = this->points_.publish(readFromDevice());
                if (!sent) {
                    HYPHA_LOG_WARN_THROTTLE(
                        1.0, "cloud not sent: {}", this->points_.lastPublishStatus().name()
                    );
                }
            } catch (const std::exception& error) {
                this->failRun(error.what());
                return;
            }
        } while (rate.sleep());  // false: the node stopped
    }

    hypha::Publisher<sensor_msgs::PointCloud2> points_;
    std::thread                                worker_;
};

sensor_msgs::PointCloud2 readFromDevice()
{
    return {};
}

}  // namespace

TEST(ApplicationDocExamples, TheGuideClassesAreApplicationNodes)
{
    EXPECT_TRUE((std::is_base_of_v<hypha::ApplicationNode, Locator>));
    EXPECT_TRUE((std::is_base_of_v<hypha::ApplicationNode, LidarNode>));
}
