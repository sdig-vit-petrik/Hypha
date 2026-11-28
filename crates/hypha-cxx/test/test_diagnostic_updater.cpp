// The status tasks of a node, published as one DiagnosticArray: what ROS's
// diagnostic_updater makes and a diagnostic_aggregator reads.

#include "hypha/cycle_node.h"
#include "hypha/diagnostic_functions.h"
#include "hypha/diagnostic_updater.h"
#include "hypha/hypha.h"
#include "hypha/publisher.h"
#include "hypha/testing.h"

#include "test_types.h"

#include <diagnostic_msgs/DiagnosticArray.h>
#include <gtest/gtest.h>

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

/// What the updater sends to, in place of a port.
struct Collect
{
    std::vector<DiagnosticArray> sent;

    void send(const DiagnosticArray& array)
    {
        this->sent.push_back(array);
    }
};

std::string valueOf(const DiagnosticStatus& status, const std::string& key)
{
    for (const diagnostic_msgs::KeyValue& entry : status.values) {
        if (entry.key == key) {
            return entry.value;
        }
    }
    return "<missing>";
}

/// A task runs on a fresh status in the updater; so does a task of these tests.
template <typename Task>
DiagnosticStatusWrapper runAt(Task& task, Time now)
{
    DiagnosticStatusWrapper status;
    status.setNow(now);
    task.run(status);
    return status;
}

/// The array of one forced update of @p updater at 10 s.
DiagnosticArray forced(DiagnosticUpdater& updater, Collect& sink)
{
    updater.forceUpdate(at(10.0));
    return sink.sent.back();
}

}  // namespace

TEST(DiagnosticUpdater, TwoTasksMakeOneArrayWithNamesLevelsValuesAndHardwareId)
{
    Collect           sink;
    DiagnosticUpdater updater{sink, "lidar"};
    updater.setHardwareID("hesai-1");
    updater.add("Connection", [](DiagnosticStatusWrapper& status) {
        status.summary(DiagnosticStatus::OK, "connected");
        status.add("peer", "192.168.1.201");
        status.add("locked", true);
        status.add("temperature", 41.5);
        status.addf("packets", "%d of %d", 3, 4);
    });
    updater.add("Rate", [](DiagnosticStatusWrapper& status) {
        status.summaryf(DiagnosticStatus::WARN, "%.1f Hz", 8.5);
    });

    const DiagnosticArray array = forced(updater, sink);

    ASSERT_EQ(array.status.size(), 2U);
    EXPECT_EQ(array.header.stamp.secs, 10U);
    EXPECT_EQ(array.header.stamp.nsecs, 0U);
    const DiagnosticStatus& connection = array.status[0];
    EXPECT_EQ(connection.name, "lidar: Connection");
    EXPECT_EQ(connection.hardware_id, "hesai-1");
    EXPECT_EQ(connection.level, DiagnosticStatus::OK);
    EXPECT_EQ(connection.message, "connected");
    ASSERT_EQ(connection.values.size(), 4U);
    EXPECT_EQ(valueOf(connection, "peer"), "192.168.1.201");
    EXPECT_EQ(valueOf(connection, "locked"), "True");
    EXPECT_EQ(valueOf(connection, "temperature"), "41.5");
    EXPECT_EQ(valueOf(connection, "packets"), "3 of 4");
    EXPECT_EQ(array.status[1].name, "lidar: Rate");
    EXPECT_EQ(array.status[1].level, DiagnosticStatus::WARN);
    EXPECT_EQ(array.status[1].message, "8.5 Hz");
}

TEST(DiagnosticUpdater, AnArrayLeavesOncePerPeriodAndTheFirstCallOnlyArms)
{
    Collect           sink;
    DiagnosticUpdater updater{sink, "n"};
    int               runs = 0;
    updater.add("t", [&runs](DiagnosticStatusWrapper& status) {
        ++runs;
        status.summary(DiagnosticStatus::OK, "ok");
    });

    updater.update(at(100.0));
    EXPECT_TRUE(sink.sent.empty()) << "the first array leaves one period after the first call";
    updater.update(at(100.5));
    EXPECT_TRUE(sink.sent.empty());
    updater.update(at(101.0));
    ASSERT_EQ(sink.sent.size(), 1U);
    EXPECT_EQ(sink.sent[0].header.stamp.secs, 101U);
    updater.update(at(101.5));
    EXPECT_EQ(sink.sent.size(), 1U);
    updater.update(at(102.2));
    EXPECT_EQ(sink.sent.size(), 2U);
    EXPECT_EQ(runs, 2) << "a task runs for an array only";
}

TEST(DiagnosticUpdater, ThePeriodIsSetAndABadOneIsRefused)
{
    Collect           sink;
    DiagnosticUpdater updater{sink, "n"};
    updater.add("t", [](DiagnosticStatusWrapper& status) {
        status.summary(DiagnosticStatus::OK, "ok");
    });
    updater.setPeriod(0.25);
    updater.update(at(1.0));
    updater.update(at(1.25));
    EXPECT_EQ(sink.sent.size(), 1U);

    EXPECT_THROW(updater.setPeriod(0.0), std::invalid_argument);
    EXPECT_THROW(updater.setPeriod(-1.0), std::invalid_argument);
    EXPECT_THROW(updater.setPeriod(std::nan("")), std::invalid_argument);
}

TEST(DiagnosticUpdater, TimeThatWentBackStartsThePeriodAgainInsteadOfSilencingTheNode)
{
    Collect           sink;
    DiagnosticUpdater updater{sink, "n"};
    updater.add("t", [](DiagnosticStatusWrapper& status) {
        status.summary(DiagnosticStatus::OK, "ok");
    });
    updater.update(at(1000.0));
    updater.update(at(1001.0));
    ASSERT_EQ(sink.sent.size(), 1U);
    updater.update(at(5.0));
    updater.update(at(6.0));
    EXPECT_EQ(sink.sent.size(), 2U);
}

TEST(DiagnosticUpdater, ATaskThatThrowsIsAnErrorAndTheOthersStillRun)
{
    Collect           sink;
    DiagnosticUpdater updater{sink, "n"};
    updater.add("Broken", [](DiagnosticStatusWrapper& status) {
        status.add("half", 1);
        throw std::runtime_error("serial port gone");
    });
    updater.add("Opaque", [](DiagnosticStatusWrapper&) {
        throw 42;
    });
    updater.add("Fine", [](DiagnosticStatusWrapper& status) {
        status.summary(DiagnosticStatus::OK, "fine");
    });

    const DiagnosticArray array = forced(updater, sink);

    ASSERT_EQ(array.status.size(), 3U);
    EXPECT_EQ(array.status[0].level, DiagnosticStatus::ERROR);
    EXPECT_EQ(array.status[0].message, "Exception in diagnostic task: serial port gone");
    EXPECT_TRUE(array.status[0].values.empty()) << "a failed task reports no half-written values";
    EXPECT_EQ(array.status[1].level, DiagnosticStatus::ERROR);
    EXPECT_EQ(array.status[1].message, "Exception in diagnostic task: unknown");
    EXPECT_EQ(array.status[2].level, DiagnosticStatus::OK);
}

TEST(DiagnosticUpdater, ATaskThatSetsNothingOrAnInvalidLevelIsAnErrorAndStaleIsKept)
{
    Collect           sink;
    DiagnosticUpdater updater{sink, "n"};
    updater.add("Silent", [](DiagnosticStatusWrapper&) {
    });
    updater.add("Invalid", [](DiagnosticStatusWrapper& status) {
        status.summary(7, "what level");
    });
    updater.add("Negative", [](DiagnosticStatusWrapper& status) {
        status.summary(-1, "below");
    });
    updater.add("Stale", [](DiagnosticStatusWrapper& status) {
        status.summary(DiagnosticStatus::STALE, "no data for 12 s");
    });

    const DiagnosticArray array = forced(updater, sink);

    ASSERT_EQ(array.status.size(), 4U);
    EXPECT_EQ(array.status[0].level, DiagnosticStatus::ERROR);
    EXPECT_EQ(array.status[0].message, "No message was set");
    EXPECT_EQ(array.status[1].level, DiagnosticStatus::ERROR);
    EXPECT_EQ(array.status[1].message, "invalid level 7: what level");
    EXPECT_EQ(array.status[2].level, DiagnosticStatus::ERROR);
    EXPECT_EQ(array.status[2].message, "invalid level -1: below");
    EXPECT_EQ(array.status[3].level, DiagnosticStatus::STALE);
    EXPECT_EQ(array.status[3].message, "no data for 12 s");
}

TEST(DiagnosticUpdater, MethodTasksRemovalAndBroadcast)
{
    struct Device
    {
        void diagnose(DiagnosticStatusWrapper& status)
        {
            status.summary(DiagnosticStatus::OK, "device ok");
        }
    };

    Device            device;
    Collect           sink;
    DiagnosticUpdater updater{sink, "n"};
    updater.add("Device", &device, &Device::diagnose);
    updater.add("Other", [](DiagnosticStatusWrapper& status) {
        status.summary(DiagnosticStatus::OK, "other ok");
    });
    EXPECT_EQ(forced(updater, sink).status[0].message, "device ok");

    EXPECT_TRUE(updater.removeByName("Other"));
    EXPECT_FALSE(updater.removeByName("Other"));
    EXPECT_EQ(forced(updater, sink).status.size(), 1U);

    updater.broadcast(DiagnosticStatus::ERROR, "shutting down", at(11.0));
    const DiagnosticArray& broadcast = sink.sent.back();
    ASSERT_EQ(broadcast.status.size(), 1U);
    EXPECT_EQ(broadcast.status[0].name, "n: Device");
    EXPECT_EQ(broadcast.status[0].level, DiagnosticStatus::ERROR);
    EXPECT_EQ(broadcast.status[0].message, "shutting down");
}

TEST(DiagnosticStatusWrapper, MergeSummaryKeepsTheWorstLevelAndJoinsMessagesOfOneKind)
{
    DiagnosticStatusWrapper status;
    status.mergeSummary(DiagnosticStatus::OK, "a");
    status.mergeSummary(DiagnosticStatus::OK, "b");
    EXPECT_EQ(status.level, DiagnosticStatus::OK);
    EXPECT_EQ(status.message, "a; b");
    status.mergeSummary(DiagnosticStatus::WARN, "slow");
    EXPECT_EQ(status.level, DiagnosticStatus::WARN);
    EXPECT_EQ(status.message, "slow") << "a fault replaces the message of OK";
    status.mergeSummary(DiagnosticStatus::ERROR, "gone");
    status.mergeSummary(DiagnosticStatus::OK, "fine");
    EXPECT_EQ(status.level, DiagnosticStatus::ERROR);
    EXPECT_EQ(status.message, "slow; gone");
    status.clearSummary();
    EXPECT_EQ(status.level, DiagnosticStatus::OK);
    EXPECT_TRUE(status.message.empty());
}

TEST(DiagnosticUpdater, ACompositeTaskReportsTheWorstOfItsTasks)
{
    hypha::FunctionDiagnosticTask  good{"good", [](DiagnosticStatusWrapper& status) {
                                           status.summary(DiagnosticStatus::OK, "up");
                                           status.add("a", 1);
                                        }};
    hypha::FunctionDiagnosticTask  bad{"bad", [](DiagnosticStatusWrapper& status) {
                                          status.summary(DiagnosticStatus::ERROR, "down");
                                          status.add("b", 2);
                                       }};
    hypha::CompositeDiagnosticTask both{"both"};
    both.addTask(&good);
    both.addTask(&bad);
    EXPECT_THROW(both.addTask(nullptr), std::invalid_argument);
    Collect           sink;
    DiagnosticUpdater updater{sink, "n"};
    updater.add(both);

    const DiagnosticArray array = forced(updater, sink);

    ASSERT_EQ(array.status.size(), 1U);
    EXPECT_EQ(array.status[0].name, "n: both");
    EXPECT_EQ(array.status[0].level, DiagnosticStatus::ERROR);
    EXPECT_EQ(array.status[0].message, "down");
    EXPECT_EQ(array.status[0].values.size(), 2U);
}

TEST(DiagnosticUpdater, AFrequencyCheckReadsTheRateOfTheWindow)
{
    double                 minimum = 9.0;
    double                 maximum = 11.0;
    hypha::FrequencyStatus rate{hypha::FrequencyStatusParam{&minimum, &maximum, 0.0, 2}};

    DiagnosticStatusWrapper status = runAt(rate, at(0.0));
    EXPECT_EQ(status.level, DiagnosticStatus::ERROR) << "no event since the window began";
    EXPECT_EQ(status.message, "No events recorded.");

    for (int tick = 1; tick <= 10; ++tick) {
        rate.tick(at(0.1 * tick));
    }
    status = runAt(rate, at(1.0));
    EXPECT_EQ(status.level, DiagnosticStatus::OK);
    EXPECT_EQ(status.message, "Desired frequency met");
    EXPECT_EQ(valueOf(status, "Events in window"), "10");
    EXPECT_EQ(valueOf(status, "Actual frequency (Hz)"), "10");
    EXPECT_EQ(valueOf(status, "Events since startup"), "10");

    for (int tick = 0; tick < 5; ++tick) {
        rate.tick(at(1.5));
    }
    status = runAt(rate, at(2.0));
    EXPECT_EQ(status.level, DiagnosticStatus::WARN);
    EXPECT_EQ(status.message, "Frequency too low.");

    for (int tick = 0; tick < 40; ++tick) {
        rate.tick(at(2.5));
    }
    status = runAt(rate, at(3.0));
    EXPECT_EQ(status.level, DiagnosticStatus::WARN);
    EXPECT_EQ(status.message, "Frequency too high.");
}

TEST(DiagnosticUpdater, AFrequencyCheckRefusesALimitThatCannotBeRead)
{
    double minimum = 1.0;
    EXPECT_THROW(hypha::FrequencyStatusParam(&minimum, nullptr), std::invalid_argument);
    EXPECT_THROW(hypha::FrequencyStatusParam(&minimum, &minimum, 0.1, 0), std::invalid_argument);
}

TEST(DiagnosticUpdater, AStampCheckJudgesTheAgeOfEachStampSinceTheLastUpdate)
{
    hypha::TimeStampStatus stamps{hypha::TimeStampStatusParam{-0.5, 2.0}};
    const Time             now = at(100.0);

    DiagnosticStatusWrapper status = runAt(stamps, now);
    EXPECT_EQ(status.level, DiagnosticStatus::WARN);
    EXPECT_EQ(status.message, "No data since last update.");

    stamps.tick(at(99.5), now);
    status = runAt(stamps, now);
    EXPECT_EQ(status.level, DiagnosticStatus::OK);
    EXPECT_EQ(valueOf(status, "Latest timestamp delay since last update (s)"), "0.5");

    stamps.tick(at(90.0), now);
    status = runAt(stamps, now);
    EXPECT_EQ(status.level, DiagnosticStatus::ERROR);
    EXPECT_EQ(status.message, "Timestamp too far in past seen since last update.");

    stamps.tick(at(101.0), now);
    status = runAt(stamps, now);
    EXPECT_EQ(status.level, DiagnosticStatus::ERROR);
    EXPECT_EQ(status.message, "Timestamp too far in future seen since last update.");

    stamps.tick(at(0.0), now);
    stamps.tick(hypha_msgs::RosTime{.secs = 5U, .nsecs = 1'000'000'000U});
    status = runAt(stamps, now);
    EXPECT_EQ(status.level, DiagnosticStatus::ERROR);
    EXPECT_EQ(
        status.message,
        "No data since last update.; Timestamp of zero was seen since last update.; Invalid "
        "timestamp seen since last update."
    ) << "WARN and ERROR are both faults, so the messages join";
}

TEST(DiagnosticUpdater, ATopicDiagnosticAddsItselfAndReportsRateAndStamp)
{
    Collect                sink;
    DiagnosticUpdater      updater{sink, "driver"};
    double                 minimum = 4.0;
    double                 maximum = 6.0;
    hypha::TopicDiagnostic scan{"scan", updater, hypha::FrequencyStatusParam{&minimum, &maximum}};
    hypha::HeaderlessTopicDiagnostic raw{
        "raw", updater, hypha::FrequencyStatusParam{&minimum, &maximum}
    };

    const Time start = hypha::now();
    for (int tick = 0; tick < 5; ++tick) {
        scan.tick(start);
        raw.tick();
    }
    updater.forceUpdate(start + hypha::Duration::fromSeconds(1.0));

    const DiagnosticArray& array = sink.sent.back();
    ASSERT_EQ(array.status.size(), 2U);
    EXPECT_EQ(array.status[0].name, "driver: scan");
    EXPECT_EQ(array.status[0].level, DiagnosticStatus::OK);
    EXPECT_EQ(array.status[0].message, "Desired frequency met; Timestamps are reasonable.");
    EXPECT_EQ(array.status[1].name, "driver: raw");
    EXPECT_EQ(array.status[1].level, DiagnosticStatus::OK);
}

TEST(DiagnosticUpdater, ARefusedPublishIsAnErrorOfTheCallerNotALostArray)
{
    hypha::Publisher<DiagnosticArray> disabled;
    DiagnosticUpdater                 updater{disabled, "n"};
    updater.add("t", [](DiagnosticStatusWrapper& status) {
        status.summary(DiagnosticStatus::OK, "ok");
    });
    EXPECT_THROW(updater.forceUpdate(at(1.0)), std::runtime_error);
}

TEST(DiagnosticUpdater, WithoutANameAndWithoutANodeTheUpdateIsRefused)
{
    if (!hypha::nodeName().empty()) {
        GTEST_SKIP() << "an earlier test of this process holds a node";
    }
    Collect           sink;
    DiagnosticUpdater updater{sink};
    updater.add("t", [](DiagnosticStatusWrapper& status) {
        status.summary(DiagnosticStatus::OK, "ok");
    });
    EXPECT_THROW(updater.forceUpdate(at(1.0)), std::logic_error);
}

namespace {

/// A node as a port writes it: the updater sends on the Output of the cycle.
class Driver final : public hypha::CycleNode
{
  public:
    Output<DiagnosticArray> diagnostics{*this, "diagnostics"};
    DiagnosticUpdater       updater{this->diagnostics, "driver"};
    bool                    connected{true};

    Driver()
    {
        this->updater.setHardwareID("hesai");
        this->updater.add("Connection", this, &Driver::diagnoseConnection);
        this->updater.add("Cycle", [](DiagnosticStatusWrapper& status) {
            status.summary(DiagnosticStatus::OK, "stepping");
        });
    }

    void step(const hypha::CycleTime& time) override
    {
        this->updater.update(time.now);
    }

  private:
    void diagnoseConnection(DiagnosticStatusWrapper& status)
    {
        if (this->connected) {
            status.summary(DiagnosticStatus::OK, "connected");
        } else {
            status.summary(DiagnosticStatus::ERROR, "device absent");
        }
    }
};

}  // namespace

TEST(DiagnosticUpdater, ACycleNodeSendsTheArrayOnItsOutputInTheStepThatIsDue)
{
    Driver                      driver;
    hypha::testing::StepHarness harness{driver};
    harness.step(driver, hypha::CycleTime{.now = at(50.0)});
    EXPECT_TRUE(harness.sent(driver.diagnostics).empty());

    driver.connected = false;
    harness.step(driver, hypha::CycleTime{.now = at(51.0)});
    const std::vector<DiagnosticArray>& sent = harness.sent(driver.diagnostics);
    ASSERT_EQ(sent.size(), 1U);
    ASSERT_EQ(sent[0].status.size(), 2U);
    EXPECT_EQ(sent[0].status[0].name, "driver: Connection");
    EXPECT_EQ(sent[0].status[0].level, DiagnosticStatus::ERROR);
    EXPECT_EQ(sent[0].status[0].message, "device absent");
    EXPECT_EQ(sent[0].status[0].hardware_id, "hesai");
    EXPECT_EQ(sent[0].status[1].level, DiagnosticStatus::OK);

    harness.step(driver, hypha::CycleTime{.now = at(51.5)});
    EXPECT_TRUE(harness.sent(driver.diagnostics).empty()) << "one array per period";
}

// What a diagnostic_aggregator subscribes: the array on the bus, as the ROS message.
TEST(DiagnosticUpdater, AnAggregatorReceivesTheArrayOfAPublisherOnTheBus)
{
    hypha::Node node("diagnostics_test");
    auto        publisher =
        node.advertise<DiagnosticArray>(hypha::Topic{hypha_test::testTopic("/diagnostics")});
    auto subscriber =
        node.subscribe<DiagnosticArray>(hypha::Topic{hypha_test::testTopic("/diagnostics")});
    DiagnosticUpdater updater{publisher, "arm"};
    updater.setHardwareID("arm-1");
    updater.add("Joint 1", [](DiagnosticStatusWrapper& status) {
        status.summary(DiagnosticStatus::WARN, "hot");
        status.add("temperature", 71.5);
    });
    updater.add("Joint 2", [](DiagnosticStatusWrapper& status) {
        status.summary(DiagnosticStatus::OK, "fine");
    });

    updater.forceUpdate(hypha::now());
    for (int wait = 0; wait < 100 && subscriber.messageCount() == 0U; ++wait) {
        static_cast<void>(node.runFor(hypha::Duration::fromMillis(20)));
    }

    const auto received = subscriber.latestSample();
    ASSERT_TRUE(received.has_value());
    const DiagnosticArray& array = *received->message;
    ASSERT_EQ(array.status.size(), 2U);
    EXPECT_EQ(array.status[0].name, "arm: Joint 1");
    EXPECT_EQ(array.status[0].hardware_id, "arm-1");
    EXPECT_EQ(array.status[0].level, DiagnosticStatus::WARN);
    EXPECT_EQ(valueOf(array.status[0], "temperature"), "71.5");
    EXPECT_EQ(array.status[1].name, "arm: Joint 2");
}

// A system has many nodes with status tasks: more publishers than the default policy admits.
TEST(DiagnosticUpdater, ManyNodesPublishOnTheDiagnosticsTopicUnderItsOwnPolicy)
{
    static_assert(hypha::QosOf<DiagnosticArray>::value.name == hypha::diagnosticsQos.name);
    static_assert(hypha::diagnosticsQos.publishers > hypha::defaultQos.publishers);
    hypha::Node                                    node("diagnostics_many_test");
    std::vector<hypha::Publisher<DiagnosticArray>> publishers;
    for (int index = 0; index < 20; ++index) {
        publishers.push_back(node.advertise<DiagnosticArray>(
            hypha::Topic{hypha_test::testTopic("/diagnostics_many")}
        ));
    }
    for (hypha::Publisher<DiagnosticArray>& publisher : publishers) {
        EXPECT_TRUE(publisher.publish(DiagnosticArray{}));
    }
}

namespace {

// The example of docs/cpp-api.md section 2.7, as written there.
class DocDriver final : public hypha::CycleNode
{
  public:
    Output<diagnostic_msgs::DiagnosticArray> diagnostics{*this, "diagnostics"};
    hypha::DiagnosticUpdater                 updater{diagnostics};

    DocDriver()
    {
        updater.setHardwareID("lidar");
        updater.setPeriod(hypha::config().optional<double>("diagnostic_period", 1.0));
        updater.add("Connection", this, &DocDriver::diagnoseConnection);
    }

    void step(const hypha::CycleTime&) override
    {
        updater.update();
    }

    void diagnoseConnection(hypha::DiagnosticStatusWrapper& status)
    {
        if (connected) {
            status.summary(diagnostic_msgs::DiagnosticStatus::OK, "connected");
        } else {
            status.summary(diagnostic_msgs::DiagnosticStatus::ERROR, "device absent");
        }
        status.add("peer", peer);
    }

    bool        connected{true};
    std::string peer{"192.168.1.201"};
};

}  // namespace

TEST(DiagnosticUpdater, TheDocumentedExampleBuildsAndAnOutputRefusesASendOutsideAStep)
{
    DocDriver driver;
    EXPECT_EQ(driver.diagnostics.sentCount(), 0U);
    Driver outside;
    EXPECT_THROW(outside.updater.forceUpdate(at(1.0)), std::logic_error);
}
