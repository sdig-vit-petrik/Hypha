// The checks of a DiagnosticUpdater in `~/state` (#121): the bridge from the companion to
// hypha::reportChecks. The rules of a check (degrade, replace per owner, bounds) are decided
// in hypha_node (node_state.rs, node_checks/tests.rs).

#include "hypha/application_node.h"
#include "hypha/cycle_node.h"
#include "hypha/diagnostic_functions.h"
#include "hypha/diagnostic_updater.h"
#include "hypha/hypha.h"
#include "hypha/publisher.h"
#include "hypha/testing.h"
#include <hypha_msgs/NodeState.h>

#include "test_types.h"

#include <diagnostic_msgs/DiagnosticArray.h>
#include <gtest/gtest.h>

#include <atomic>
#include <expected>
#include <stdexcept>
#include <string>
#include <vector>

namespace {

using diagnostic_msgs::DiagnosticArray;
using diagnostic_msgs::DiagnosticStatus;
using hypha::DiagnosticStatusWrapper;
using hypha::DiagnosticUpdater;
using hypha::Time;

Time at(double seconds)
{
    return Time::fromSeconds(seconds);
}

}  // namespace

// The same named checks reach `~/state` of the node: the application declares a check once.
namespace {

hypha_msgs::NodeState stateNow()
{
    const std::expected<hypha_msgs::NodeState, hypha::Status> state = hypha::nodeState();
    EXPECT_TRUE(state.has_value());
    return state.value_or(hypha_msgs::NodeState{});
}

const hypha_msgs::NodeCheck* checkNamed(const hypha_msgs::NodeState& state, const std::string& name)
{
    for (const hypha_msgs::NodeCheck& check : state.checks) {
        if (check.name == name) {
            return &check;
        }
    }
    return nullptr;
}

/// The state of a driver whose device is, or is not, there.
struct Device
{
    bool present{false};

    void diagnose(DiagnosticStatusWrapper& status) const
    {
        if (this->present) {
            status.summary(DiagnosticStatus::OK, "connected");
        } else {
            status.summary(DiagnosticStatus::ERROR, "serial port absent");
        }
    }
};

}  // namespace

TEST(DiagnosticUpdaterState, ACheckThatThrowsSetsNothingOrReportsStaleIsNotReady)
{
    hypha::Node       node("checks_fault_state");
    DiagnosticUpdater updater;
    updater.add("Throws", [](DiagnosticStatusWrapper&) {
        throw std::runtime_error("boom");
    });
    updater.add("Silent", [](DiagnosticStatusWrapper&) {
    });
    updater.add("Stale", [](DiagnosticStatusWrapper& status) {
        status.summary(DiagnosticStatus::STALE, "no fix for 5 s");
    });
    updater.forceUpdate(at(1.0));

    const hypha_msgs::NodeState state = stateNow();
    EXPECT_EQ(state.state, hypha_msgs::NodeState::DEGRADED);
    ASSERT_EQ(state.checks.size(), 3U);
    EXPECT_EQ(state.checks[0].level, hypha_msgs::NodeCheck::ERROR);
    EXPECT_EQ(state.checks[0].message, "Exception in diagnostic task: boom");
    EXPECT_EQ(state.checks[1].level, hypha_msgs::NodeCheck::ERROR);
    EXPECT_EQ(state.checks[1].message, "No message was set");
    EXPECT_EQ(state.checks[2].level, hypha_msgs::NodeCheck::STALE);
}

TEST(DiagnosticUpdaterState, TheSameChecksLeaveOnDiagnosticsWithTheNodeNameAndOnState)
{
    hypha::Node       node("checks_both");
    Collect           sink;
    Device            device;
    DiagnosticUpdater updater{sink};
    updater.add("Device", [&device](DiagnosticStatusWrapper& status) {
        device.diagnose(status);
    });
    updater.forceUpdate(at(1.0));

    ASSERT_EQ(sink.sent.size(), 1U);
    ASSERT_EQ(sink.sent[0].status.size(), 1U);
    EXPECT_EQ(sink.sent[0].status[0].name, "checks_both: Device");
    const hypha_msgs::NodeState state = stateNow();
    EXPECT_EQ(state.state, hypha_msgs::NodeState::DEGRADED);
    ASSERT_EQ(state.checks.size(), 1U);
    EXPECT_EQ(state.checks[0].message, sink.sent[0].status[0].message);

    updater.broadcast(DiagnosticStatus::WARN, "starting", at(2.0));
    EXPECT_EQ(stateNow().state, hypha_msgs::NodeState::INPUTS_USABLE) << "a broadcast is a report";
    EXPECT_EQ(stateNow().checks.at(0).level, hypha_msgs::NodeCheck::WARN);
}

TEST(DiagnosticUpdaterState, ATaskThatIsRemovedLeavesWithTheNextUpdateAndAGoneUpdaterLeavesAtOnce)
{
    hypha::Node node("checks_removal");
    {
        DiagnosticUpdater first;
        DiagnosticUpdater second;
        first.add("A", [](DiagnosticStatusWrapper& status) {
            status.summary(DiagnosticStatus::OK, "a");
        });
        first.add("B", [](DiagnosticStatusWrapper& status) {
            status.summary(DiagnosticStatus::OK, "b");
        });
        second.add("B", [](DiagnosticStatusWrapper& status) {
            status.summary(DiagnosticStatus::WARN, "other b");
        });
        first.forceUpdate(at(1.0));
        second.forceUpdate(at(1.0));
        EXPECT_EQ(stateNow().checks.size(), 3U) << "two updaters report apart";

        EXPECT_TRUE(first.removeByName("A"));
        first.forceUpdate(at(2.0));
        EXPECT_EQ(stateNow().checks.size(), 2U);
        EXPECT_EQ(checkNamed(stateNow(), "A"), nullptr);
    }
    EXPECT_TRUE(stateNow().checks.empty()) << "an updater that is gone vouches for nothing";
}

TEST(DiagnosticUpdaterState, ARefusedReportThrowsAndTheArrayStillLeaves)
{
    hypha::Node       node("checks_refused");
    Collect           sink;
    DiagnosticUpdater updater{sink};
    for (int index = 0; index < 65; ++index) {
        updater.add("check " + std::to_string(index), [](DiagnosticStatusWrapper& status) {
            status.summary(DiagnosticStatus::OK, "ok");
        });
    }

    EXPECT_THROW(updater.forceUpdate(at(1.0)), std::runtime_error);

    ASSERT_EQ(sink.sent.size(), 1U);
    EXPECT_EQ(sink.sent[0].status.size(), 65U);
    EXPECT_TRUE(stateNow().checks.empty()) << "nothing of a refused report is kept";
}

// What a ported driver writes: its step calls update(), and `~/state` carries the result.
TEST(DiagnosticUpdaterState, ADriverWhoseDeviceIsAbsentIsDegradedOnTheBus)
{
    hypha::Node node("checks_driver");
    Driver      driver;
    driver.connected = false;
    hypha::testing::StepHarness harness{driver};
    harness.step(driver, hypha::CycleTime{.now = at(50.0)});
    harness.step(driver, hypha::CycleTime{.now = at(51.0)});

    const hypha_msgs::NodeState state = stateNow();
    EXPECT_EQ(state.state, hypha_msgs::NodeState::DEGRADED);
    const hypha_msgs::NodeCheck* connection = checkNamed(state, "Connection");
    ASSERT_NE(connection, nullptr);
    EXPECT_EQ(connection->message, "device absent");
}

namespace {

// The ApplicationNode example of docs/cpp-api.md section 2.5, as written there.
class ArenaCamera final : public hypha::ApplicationNode
{
  public:
    ArenaCamera()
    {
        updater_.add("Device", this, &ArenaCamera::diagnoseDevice);
    }

    void setup(hypha::Node& node) override
    {
        // The timer is the period; its task runs on the run thread.
        timer_ = node.timer(hypha::Duration::fromSeconds(1.0), [this] {
            updater_.forceUpdate();
        });
    }

    void started(hypha::Node&) override
    {
        updater_.forceUpdate();  // the first row at once, not after a period
    }

    std::atomic<bool> connected_{false};

  private:
    // Reads what the worker publishes; never waits for the device.
    void diagnoseDevice(hypha::DiagnosticStatusWrapper& status)
    {
        if (connected_) {
            status.summary(diagnostic_msgs::DiagnosticStatus::OK, "camera connected");
        } else {
            status.summary(diagnostic_msgs::DiagnosticStatus::ERROR, "camera absent");
        }
        status.add("grabs without image", noImage_.load());
    }

    hypha::DiagnosticUpdater updater_;
    hypha::Timer             timer_;
    std::atomic<uint64_t>    noImage_{0};
};

}  // namespace

TEST(DiagnosticUpdaterState, TheDocumentedApplicationExampleReportsTheAbsentCamera)
{
    hypha::Node node("doc_camera");
    ArenaCamera camera;
    camera.setup(node);
    camera.started(node);

    const hypha_msgs::NodeState state = stateNow();
    EXPECT_EQ(state.state, hypha_msgs::NodeState::DEGRADED);
    ASSERT_EQ(state.checks.size(), 1U);
    EXPECT_EQ(state.checks[0].name, "Device");
    EXPECT_EQ(state.checks[0].message, "camera absent");
}
