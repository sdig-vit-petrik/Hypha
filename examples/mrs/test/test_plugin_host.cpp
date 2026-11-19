/// mrs::PluginHost and the mrs_lib wrappers: two hosts on two composed cycles of one node,
/// run in host time with node.runFor slices.
#include <hypha/config.h>
#include <hypha/cycle_node.h>
#include <hypha/node.h>
#include <hypha_log/hypha_log.h>

#include "check.h"

#include <mrs_host/plugin_host.h>
#include <mrs_host/service.h>
#include <mrs_lib/param_loader.h>
#include <mrs_lib/publisher_handler.h>
#include <mrs_lib/subscribe_handler.h>
#include <mrs_msgs/Float64Stamped.h>
#include <mrs_msgs/Vec1.h>
#include <ros/node_handle.h>
#include <std_srvs/SetBool.h>
#include <unistd.h>

#include <filesystem>
#include <fstream>
#include <memory>
#include <stdexcept>
#include <string>

HYPHA_LOG_MODULE_INIT("mrs_plugin_host_test")

namespace {

const char* PARAMETERS = R"(
[node]
name = "mrs_plugin_host_test"

[control_manager.default_constraints]
horizontal_speed = 2.5

[[remap]]
from = "~cmd_in"
to = "/uav1/x"

[ports.outputs.uav1.x]
topic = "/uav1/x"
[ports.outputs.uav1.b.answer.request]
topic = "/uav1/b/answer/request"
[ports.outputs.uav1.b.answer.result]
topic = "/uav1/b/answer/result"
[ports.outputs.uav1.b.silent.request]
topic = "/uav1/b/silent/request"
[ports.outputs.uav1.b.later.request]
topic = "/uav1/b/later/request"
[ports.outputs.uav1.b.later.result]
topic = "/uav1/b/later/result"
)";

/// One composed cycle at 100 Hz whose step runs the host, then a test hook.
struct HostCycle
{
    hypha::Cycle          cycle;
    mrs::PluginHost       host;
    std::function<void()> onStep;

    HostCycle(const std::string& name, const std::string& ns)
        : cycle(name), host(cycle, "mrs_plugin_host_test", ns)
    {
        this->cycle.setRate(hypha::Hz(100.0));
    }

    struct Step
    {
        HostCycle* self;

        void operator()(const hypha::CycleTime& time) const
        {
            self->host.step(time, self->onStep);
        }
    };

    void start(hypha::Node& node)
    {
        const auto started = this->cycle.start(node, Step{this});
        if (!started) {
            throw std::runtime_error(started.error());
        }
    }
};

void run(hypha::Node& node, double seconds)
{
    static_cast<void>(node.runFor(hypha::Duration::fromSeconds(seconds)));
}

void a_timer_fires_every_tenth_step(hypha::Node& node)
{
    HostCycle a("timer_a", "/uav1/a");
    int       fired = 0;
    a.host.addTimer(
        hypha::Duration::fromMillis(100),
        [&](const ros::TimerEvent&) {
            ++fired;
        },
        true
    );
    a.start(node);
    run(node, 1.0);
    MRS_CHECK(fired >= 9 && fired <= 11);
}

void a_stopped_timer_does_not_fire(hypha::Node& node)
{
    HostCycle    a("stop_a", "/uav1/a");
    int          fired = 0;
    mrs::TimerId id = 0;
    id = a.host.addTimer(
        hypha::Duration::fromMillis(50),
        [&](const ros::TimerEvent&) {
            if (++fired == 3) {
                a.host.stopTimer(id);
            }
        },
        true
    );
    a.start(node);
    run(node, 0.6);
    MRS_CHECK(fired == 3);
}

void a_publisher_handler_reaches_a_subscribe_handler(hypha::Node& node)
{
    HostCycle                                           a("pub_a", "/uav1/a");
    HostCycle                                           b("pub_b", "/uav1/b");
    ros::NodeHandle                                     nhA(a.host, "/uav1/a");
    ros::NodeHandle                                     nhB(b.host, "/uav1/b");
    mrs_lib::PublisherHandler<mrs_msgs::Float64Stamped> pub(nhA, "/uav1/x", 1);
    int                                                 callbacks = 0;
    double                                              lastSeen = -1.0;
    mrs_lib::SubscribeHandlerOptions                    opts(nhB);
    mrs_lib::SubscribeHandler<mrs_msgs::Float64Stamped> sub(
        opts, "/uav1/x", [&](const mrs_msgs::Float64Stamped::ConstPtr& msg) {
            ++callbacks;
            lastSeen = msg->value;
        }
    );
    int sent = 0;
    a.onStep = [&] {
        if (sent < 5) {
            mrs_msgs::Float64Stamped msg;
            msg.value = ++sent;
            pub.publish(msg);
        }
    };
    a.start(node);
    b.start(node);
    run(node, 0.3);
    MRS_CHECK(sub.hasMsg());
    MRS_CHECK(sub.getMsg()->value == 5.0);
    MRS_CHECK(callbacks == 5);
    MRS_CHECK(lastSeen == 5.0);
    MRS_CHECK(sub.topicName() == "/uav1/x");
}

void endpoint_after_start_throws(hypha::Node& node)
{
    HostCycle a("late_a", "/uav1/a");
    a.start(node);
    run(node, 0.05);
    bool threw = false;
    try {
        a.host.latest<mrs_msgs::Float64Stamped>("/uav1/late");
    } catch (const std::logic_error&) {
        threw = true;
    }
    MRS_CHECK(threw);
}

void a_request_reaches_its_server_and_matches_its_id(hypha::Node& node)
{
    HostCycle                  a("req_a", "/uav1/a");
    HostCycle                  b("req_b", "/uav1/b");
    mrs::Serve<mrs_msgs::Vec1> server(
        b.host, "/uav1/b/answer", [](uint64_t, const mrs_msgs::Vec1::Request& req) {
            mrs_msgs::Vec1::Response res;
            res.success = req.goal > 1.0;
            res.message = "goal " + std::to_string(static_cast<int>(req.goal));
            return std::optional{res};
        }
    );
    mrs::Request<mrs_msgs::Vec1> client(
        a.host, "/uav1/b/answer", hypha::Duration::fromSeconds(1.0)
    );
    bool sent = false;
    a.onStep = [&] {
        if (!sent) {
            mrs_msgs::Vec1::Request req;
            req.goal = 2.0;
            sent = client.send(req);
            MRS_CHECK(!client.send(req));  // a second send while pending is refused
        }
    };
    a.start(node);
    b.start(node);
    run(node, 0.2);
    MRS_CHECK(client.state() == mrs::CallState::Done);
    MRS_CHECK(client.state() == mrs::CallState::Done && client.response().success);
    MRS_CHECK(client.state() == mrs::CallState::Done && client.response().message == "goal 2");
}

void a_deferred_response_is_delivered(hypha::Node& node)
{
    HostCycle                                   a("defer_a", "/uav1/a");
    HostCycle                                   b("defer_b", "/uav1/b");
    std::optional<uint64_t>                     pending;
    int                                         stepsSince = 0;
    std::unique_ptr<mrs::Serve<mrs_msgs::Vec1>> server;
    server = std::make_unique<mrs::Serve<mrs_msgs::Vec1>>(
        b.host, "/uav1/b/later", [&](uint64_t id, const mrs_msgs::Vec1::Request&) {
            pending = id;
            stepsSince = 0;
            return std::optional<mrs_msgs::Vec1::Response>{};
        }
    );
    b.onStep = [&] {
        if (pending && ++stepsSince == 2) {
            mrs_msgs::Vec1::Response res;
            res.success = true;
            server->respond(*pending, res);
            pending.reset();
        }
    };
    mrs::Request<mrs_msgs::Vec1> client(a.host, "/uav1/b/later", hypha::Duration::fromSeconds(1.0));
    a.onStep = [&] {
        if (client.state() == mrs::CallState::Idle) {
            client.send(mrs_msgs::Vec1::Request{});
        }
    };
    a.start(node);
    b.start(node);
    run(node, 0.3);
    MRS_CHECK(client.state() == mrs::CallState::Done);
}

void a_request_without_a_server_times_out(hypha::Node& node)
{
    HostCycle                    a("silent_a", "/uav1/a");
    mrs::Request<mrs_msgs::Vec1> client(a.host, "/uav1/b/silent", hypha::Duration::fromMillis(50));
    std::optional<hypha::Time>   sentAt;
    std::optional<hypha::Time>   timedOutAt;
    a.onStep = [&] {
        const hypha::Time now = *mrs::currentStepTime();
        if (!sentAt) {
            client.send(mrs_msgs::Vec1::Request{});
            sentAt = now;
        } else if (!timedOutAt && client.state() == mrs::CallState::TimedOut) {
            timedOutAt = now;
        }
    };
    a.start(node);
    run(node, 0.3);
    MRS_CHECK(timedOutAt.has_value());
    if (timedOutAt && sentAt) {
        const double waited = (*timedOutAt - *sentAt).seconds();
        MRS_CHECK(waited >= 0.050 && waited <= 0.070);
    }
}

void a_result_for_another_caller_is_ignored(hypha::Node& node)
{
    HostCycle                  a("other_a", "/uav1/a");
    HostCycle                  c("other_c", "/uav1/c");
    HostCycle                  b("other_b", "/uav1/b");
    mrs::Serve<mrs_msgs::Vec1> server(
        b.host, "/uav1/b/answer", [](uint64_t, const mrs_msgs::Vec1::Request& req) {
            mrs_msgs::Vec1::Response res;
            res.message = std::to_string(static_cast<int>(req.goal));
            return std::optional{res};
        }
    );
    mrs::Request<mrs_msgs::Vec1> fromA(a.host, "/uav1/b/answer", hypha::Duration::fromSeconds(1.0));
    mrs::Request<mrs_msgs::Vec1> fromC(c.host, "/uav1/b/answer", hypha::Duration::fromSeconds(1.0));
    a.onStep = [&] {
        if (fromA.state() == mrs::CallState::Idle) {
            mrs_msgs::Vec1::Request req;
            req.goal = 1.0;
            fromA.send(req);
        }
    };
    c.onStep = [&] {
        if (fromC.state() == mrs::CallState::Idle) {
            mrs_msgs::Vec1::Request req;
            req.goal = 2.0;
            fromC.send(req);
        }
    };
    a.start(node);
    c.start(node);
    b.start(node);
    run(node, 0.2);
    MRS_CHECK(fromA.state() == mrs::CallState::Done && fromA.response().message == "1");
    MRS_CHECK(fromC.state() == mrs::CallState::Done && fromC.response().message == "2");
}

void param_loader_reports_the_missing_key()
{
    HostCycle            a("param_a", "/uav1/control_manager");
    ros::NodeHandle      nh(a.host, "/uav1/control_manager");
    mrs_lib::ParamLoader loader(nh, "ControlManager");
    double               value = 0.0;
    const uint64_t       before = mrs::paramErrorCount();
    MRS_CHECK(!loader.loadParam("missing/key", value));
    MRS_CHECK(!loader.loadedSuccessfully());
    MRS_CHECK(mrs::paramErrorCount() == before + 1);
    MRS_CHECK(mrs::lastParamError().find("missing/key") != std::string::npos);
}

void param_loader_reads_nested_keys()
{
    HostCycle            a("nested_a", "/uav1/control_manager");
    ros::NodeHandle      nh(a.host, "/uav1/control_manager");
    mrs_lib::ParamLoader loader(nh, "ControlManager");
    double               speed = 0.0;
    MRS_CHECK(loader.loadParam("control_manager/default_constraints/horizontal_speed", speed));
    MRS_CHECK(speed == 2.5);
    double fallback = 0.0;
    // As upstream: a load that falls back to the default returns false and is no error.
    MRS_CHECK(!loader.loadParam("not/there", fallback, 7.0));
    MRS_CHECK(fallback == 7.0);
    MRS_CHECK(loader.loadedSuccessfully());
}

void a_remap_from_toml_renames_a_topic(hypha::Node& node)
{
    HostCycle                                           a("remap_a", "/uav1/a");
    HostCycle                                           b("remap_b", "/uav1/remapped");
    ros::NodeHandle                                     nhA(a.host, "/uav1/a");
    ros::NodeHandle                                     nhB(b.host, "~");
    mrs_lib::PublisherHandler<mrs_msgs::Float64Stamped> pub(nhA, "/uav1/x", 1);
    mrs_lib::SubscribeHandlerOptions                    opts(nhB);
    mrs_lib::SubscribeHandler<mrs_msgs::Float64Stamped> sub(opts, "~cmd_in");
    a.onStep = [&] {
        mrs_msgs::Float64Stamped msg;
        msg.value = 3.0;
        pub.publish(msg);
    };
    a.start(node);
    b.start(node);
    run(node, 0.1);
    MRS_CHECK(sub.topicName() == "/uav1/x");
    MRS_CHECK(sub.hasMsg() && sub.getMsg()->value == 3.0);
}

}  // namespace

int main()
{
    const std::string path = (std::filesystem::temp_directory_path()
                              / ("mrs_plugin_host_test_" + std::to_string(::getpid()) + ".toml"))
                                 .string();
    {
        std::ofstream file(path);
        file << PARAMETERS;
    }
    {
        hypha::ConfigContext context(path);
        hypha::Node          node("mrs_plugin_host_test", path);
        param_loader_reports_the_missing_key();
        param_loader_reads_nested_keys();
        a_timer_fires_every_tenth_step(node);
        a_stopped_timer_does_not_fire(node);
        a_publisher_handler_reaches_a_subscribe_handler(node);
        endpoint_after_start_throws(node);
        a_request_reaches_its_server_and_matches_its_id(node);
        a_deferred_response_is_delivered(node);
        a_request_without_a_server_times_out(node);
        a_result_for_another_caller_is_ignored(node);
        a_remap_from_toml_renames_a_topic(node);
    }
    std::filesystem::remove(path);
    return mrsCheckResult();
}
