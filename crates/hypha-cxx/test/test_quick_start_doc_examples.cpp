// Compile-only check: the quick-start examples of README.md, docs/cpp-api.md
// and hypha.h build as written. Nothing here runs. A missing overload or a
// renamed call fails the build.

#include "hypha/hypha.h"
#include <hypha_log/hypha_log.h>

#include <sensor_msgs/Imu.h>
#include <std_msgs/Float64.h>

HYPHA_LOG_MODULE_INIT("compute_node")

namespace readme {

// README.md: one subscription doubles its input on another topic.
struct Doubler
{
    hypha::Publisher<std_msgs::Float64>* out;

    void operator()(const hypha::Subscriber<std_msgs::Float64>::ConstPtr& input) const
    {
        std_msgs::Float64 message;
        message.data = input->data * 2.0;
        if (!this->out->publish(message)) {
            HYPHA_LOG_WARN_THROTTLE(
                1.0, "output not sent: {}", this->out->lastPublishStatus().name()
            );
        }
    }
};

[[maybe_unused]] int main()
{
    hypha::Node   node("compute_node");
    auto          out = node.advertise<std_msgs::Float64>(hypha::Topic{"/output/data"});
    const Doubler doubler{&out};
    hypha::Subscriber<std_msgs::Float64> in =
        node.subscribe<std_msgs::Float64>(hypha::Topic{"/sensors/data"}, doubler);

    const hypha::RunResult result = node.run();
    return result.ok() ? 0 : 1;
}

}  // namespace readme

namespace cpp_api {

// docs/cpp-api.md section 1 and hypha.h: the shortest node.
[[maybe_unused]] void onScan(const hypha::Subscriber<std_msgs::Float64>::ConstPtr& scan)
{
    static_cast<void>(scan);
}

[[maybe_unused]] int main()
{
    hypha::Node            node("my_node");
    auto                   pub = node.advertise<std_msgs::Float64>(hypha::Topic{"/pose"});
    auto                   sub = node.subscribe<std_msgs::Float64>(hypha::Topic{"/scan"}, onScan);
    const hypha::RunResult result = node.run();
    return result.ok() ? 0 : 1;
}

// docs/cpp-api.md section 1: the object form registers its member callback.
class AttitudeNode
{
  public:
    AttitudeNode()
        : node_("attitude_node")
        , out_(node_.advertise<sensor_msgs::Imu>(hypha::Topic{"/attitude"}))
        , in_(node_.subscribe<sensor_msgs::Imu>(
              hypha::Topic{"/imu"}, hypha::bind<&AttitudeNode::onImu>(*this)
          ))
    {
    }

    hypha::RunResult run()
    {
        // run() calls onImu on this thread for every received message.
        return this->node_.run();
    }

  private:
    void onImu(const hypha::Subscriber<sensor_msgs::Imu>::ConstPtr& imu)
    {
        if (!this->out_.publish(*imu)) {
            HYPHA_LOG_WARN_THROTTLE(
                1.0, "attitude not sent: {}", this->out_.lastPublishStatus().name()
            );
        }
    }

    hypha::Node                         node_;
    hypha::Publisher<sensor_msgs::Imu>  out_;
    hypha::Subscriber<sensor_msgs::Imu> in_;
};

}  // namespace cpp_api

namespace publisher_only {

[[maybe_unused]] int run()
{
    hypha::Node node("sample_source");
    auto        publisher = node.advertise<std_msgs::Float64>(hypha::Topic{"/samples"});
    while (hypha::ok()) {
        const std_msgs::Float64 sample{1.0};
        if (!publisher.publish(sample)) {
            return 1;
        }
        const auto result = node.runFor(hypha::Duration::fromMillis(10));
        if (!result.ok()) {
            return 1;
        }
    }
    return 0;
}

}  // namespace publisher_only
