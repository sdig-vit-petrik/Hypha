// testing::StepHarness runs a step with no node and no bus. Only the last test starts a
// hypha::Node, to compare the harness with a real cycle.

#include "hypha/config.h"
#include "hypha/cycle_node.h"
#include "hypha/cycle_transforms.h"
#include "hypha/hypha.h"
#include "hypha/testing.h"

#include "test_types.h"

#include <geometry_msgs/TransformStamped.h>
#include <gtest/gtest.h>
#include <std_msgs/Float64.h>
#include <std_msgs/UInt32.h>
#include <tf2_msgs/TFMessage.h>

#include <chrono>
#include <cstdint>
#include <expected>
#include <filesystem>
#include <fstream>
#include <memory>
#include <optional>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>

using Float64 = std_msgs::Float64;

namespace {

class Relay final : public hypha::CycleNode
{
  public:
    Latest<Float64>   speed{*this, "speed"};
    Arrivals<Float64> marks{*this, "marks"};
    Output<Float64>   command{*this, "command", perStep(2)};
    int               sends{1};
    bool              throws{false};
    int               steps{0};
    bool              sawFresh{false};
    size_t            sawMarks{0};

    void step(const hypha::CycleTime&) override
    {
        ++this->steps;
        this->sawFresh = this->speed.fresh();
        this->sawMarks = this->marks.samples().size();
        for (int send = 0; send < this->sends; ++send) {
            this->command.send(Float64{static_cast<double>(send)});
        }
        if (this->throws) {
            throw std::runtime_error("step refused");
        }
    }
};

}  // namespace

TEST(StepHarness, SendOutsideAStepThrowsAndStoresNothing)
{
    Relay relay;
    try {
        relay.command.send(Float64{1.0});
        FAIL() << "send() outside a step must throw";
    } catch (const std::logic_error& error) {
        EXPECT_STREQ(error.what(), "hypha: output 'command': send() outside a step");
    }
    EXPECT_FALSE(relay.command.sent());
    EXPECT_EQ(relay.command.sentCount(), 0U);
}

TEST(StepHarness, AStepCollectsTheSentMessagesInOrderAndTheNextStepClearsThem)
{
    Relay                       relay;
    hypha::testing::StepHarness harness{relay};
    EXPECT_TRUE(harness.sent(relay.command).empty());
    EXPECT_FALSE(relay.command.sent()) << "nothing sent before the first step";

    relay.sends = 2;
    harness.step(relay);
    const std::vector<Float64>& sent = harness.sent(relay.command);
    ASSERT_EQ(sent.size(), 2U);
    EXPECT_DOUBLE_EQ(sent[0].data, 0.0);
    EXPECT_DOUBLE_EQ(sent[1].data, 1.0);
    EXPECT_TRUE(relay.command.sent());

    relay.sends = 0;
    harness.step(relay);
    EXPECT_TRUE(harness.sent(relay.command).empty());
    EXPECT_FALSE(relay.command.sent());
}

TEST(StepHarness, AStepKeepsThePerStepBound)
{
    Relay                       relay;
    hypha::testing::StepHarness harness{relay};
    relay.sends = 3;
    EXPECT_THROW(harness.step(relay), std::runtime_error);
    EXPECT_EQ(harness.sent(relay.command).size(), 2U);
}

TEST(StepHarness, AnExceptionOfTheStepLeavesTheHarnessUsable)
{
    Relay                       relay;
    hypha::testing::StepHarness harness{relay};
    relay.throws = true;
    EXPECT_THROW(harness.step(relay), std::runtime_error);
    relay.throws = false;
    harness.step(relay);
    EXPECT_EQ(relay.steps, 2);
    EXPECT_EQ(harness.sent(relay.command).size(), 1U);
}

TEST(StepHarness, SetValuesAnswerLikeACapture)
{
    Relay                       relay;
    hypha::testing::StepHarness harness{relay};
    harness.set(relay.marks, {});

    harness.setNeverArrived(relay.speed);
    EXPECT_FALSE(relay.speed.has());
    EXPECT_EQ(relay.speed.verdict(), hypha::InputVerdict::NeverArrived);
    EXPECT_THROW(static_cast<void>(*relay.speed), hypha::InputNotUsable);
    EXPECT_THROW(static_cast<void>(relay.speed.lastKnown()), std::logic_error);
    EXPECT_FALSE(relay.inputsOk());

    harness.set(relay.speed, Float64{2.5});
    EXPECT_TRUE(relay.speed.usable());
    EXPECT_EQ(relay.speed.verdict(), hypha::InputVerdict::Usable);
    EXPECT_DOUBLE_EQ(relay.speed->data, 2.5);
    EXPECT_FALSE(relay.speed.stale());
    EXPECT_TRUE(relay.inputsOk());

    harness.setStagnant(relay.speed, true);
    EXPECT_TRUE(relay.speed.stagnant());
    EXPECT_TRUE(relay.speed.usable()) << "stagnation does not make a value unusable";
    EXPECT_FALSE(relay.inputsOk());
    harness.setStagnant(relay.speed, false);

    harness.setStale(relay.speed);
    EXPECT_EQ(relay.speed.verdict(), hypha::InputVerdict::Stale);
    EXPECT_TRUE(relay.speed.stale());
    EXPECT_FALSE(relay.speed.usable());
    EXPECT_THROW(static_cast<void>(*relay.speed), hypha::InputNotUsable);
    EXPECT_DOUBLE_EQ(relay.speed.lastKnown().data, 2.5) << "lastKnown() keeps the last value";
    EXPECT_FALSE(relay.inputsOk());

    harness.setStale(relay.marks);
    EXPECT_TRUE(relay.marks.stale());
    EXPECT_TRUE(relay.marks.samples().empty());
}

TEST(StepHarness, ALatestValueIsFreshForOneStepAndArrivalsAreDeliveredOnce)
{
    Relay                       relay;
    hypha::testing::StepHarness harness{relay};
    harness.set(relay.speed, Float64{1.0});
    harness.set(relay.marks, {Float64{1.0}, Float64{2.0}});

    harness.step(relay);
    EXPECT_TRUE(relay.sawFresh);
    EXPECT_EQ(relay.sawMarks, 2U);

    harness.step(relay);
    EXPECT_FALSE(relay.sawFresh) << "the value stays, no longer new";
    EXPECT_TRUE(relay.speed.usable());
    EXPECT_EQ(relay.sawMarks, 0U) << "samples reach the next step only";
}

TEST(StepHarness, ACycleWithAHarnessDoesNotStartAndASecondHarnessIsRefused)
{
    Relay                       relay;
    hypha::testing::StepHarness harness{relay};
    EXPECT_THROW(hypha::testing::StepHarness{relay}, std::logic_error);
}

/// Catches the perStep refusal of its second send and returns normally.
class CatchesTheRefusal final : public hypha::CycleNode
{
  public:
    Output<Float64> command{*this, "command"};

    void step(const hypha::CycleTime&) override
    {
        this->command.send(Float64{1.0});
        try {
            this->command.send(Float64{2.0});
        } catch (const std::exception&) {
            this->caught = true;
        }
    }

    bool caught{false};
};

TEST(StepHarness, ARefusedSendFailsTheStepAlsoWhenTheStepCaughtIt)
{
    CatchesTheRefusal           node;
    hypha::testing::StepHarness harness{node};
    EXPECT_THROW(harness.step(node), std::runtime_error);
    EXPECT_TRUE(node.caught);
}

namespace {

using hypha::InputVerdict;
using hypha::Time;

constexpr int64_t kLimitNanos = 100'000'000;
constexpr int64_t kStartNanos = 5'000'000'000;

/// A cycle with one port for each way the table can set a limit, and a step that records
/// what the ports showed.
class Judged final : public hypha::CycleNode
{
  public:
    Latest<Float64> limited{*this, "limited"};
    Latest<Float64> loose{*this, "loose", advisory};
    Latest<Float64> open{*this, "open", unsupervised};

    void step(const hypha::CycleTime&) override
    {
        ++this->steps;
        this->sawFresh = this->limited.fresh();
        this->sawUsable = this->limited.usable();
        this->sawVerdict = this->limited.verdict();
    }

    int          steps{0};
    bool         sawFresh{false};
    bool         sawUsable{false};
    InputVerdict sawVerdict{InputVerdict::NeverArrived};
};

hypha::CycleTime stepAt(int64_t nanos)
{
    hypha::CycleTime time;
    time.now = Time::fromNanos(nanos);
    return time;
}

/// A parameter file with a 100 ms limit on `limited`, no limit on `loose`
/// (`silence_ms = 0`), and a table for `open`, which is unsupervised.
class DeliverFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->path = std::filesystem::temp_directory_path() / "hypha_step_harness_deliver.toml";
        std::ofstream output(this->path);
        output << "[ports.inputs.limited]\n"
                  "silence_ms = 100\n"
                  "[ports.inputs.loose]\n"
                  "silence_ms = 0\n"
                  "[ports.inputs.open]\n"
                  "[ports.inputs.queue]\n"
                  "silence_ms = 100\n";
        output.close();
        this->context = std::make_unique<hypha::ConfigContext>(this->path.string());
    }

    void TearDown() override
    {
        this->context.reset();
        std::filesystem::remove(this->path);
    }

    std::unique_ptr<hypha::ConfigContext> context;
    std::filesystem::path                 path;
};

}  // namespace

TEST_F(DeliverFixture, AStaleVerdictReportsTheAgeAndTheLimitOfTheTable)
{
    Judged                      judged;
    hypha::testing::StepHarness harness{judged};
    harness.deliver(judged.limited, Float64{3.0}, Time::fromNanos(kStartNanos));
    harness.step(judged, stepAt(kStartNanos + 1));
    harness.step(judged, stepAt(kStartNanos + 250'000'000));
    try {
        static_cast<void>(*judged.limited);
        FAIL() << "a stale port must throw";
    } catch (const hypha::InputNotUsable& error) {
        const std::string text = error.what();
        EXPECT_NE(
            text.find("is stale: its newest sample is 250 ms old, silence_ms = 100"),
            std::string::npos
        ) << text;
    }
}

TEST(StepHarnessLimits, DeliverToAPortWithoutSilenceMsIsRefusedAsAStartRefusesIt)
{
    const std::filesystem::path path =
        std::filesystem::temp_directory_path() / "hypha_step_harness_no_limit.toml";
    std::ofstream(path) << "[ports.inputs.limited]\nsilence_ms = 100\n[ports.inputs.loose]\n"
                           "[ports.inputs.open]\n";
    {
        const hypha::ConfigContext  context(path.string());
        Judged                      judged;
        hypha::testing::StepHarness harness{judged};
        EXPECT_THROW(
            harness.deliver(judged.loose, Float64{1.0}, Time::fromNanos(kStartNanos)),
            hypha::ParameterError
        ) << "an advisory port states its limit too";
        EXPECT_THROW(static_cast<void>(judged.loose.untimed()), hypha::ParameterError);
    }
    std::filesystem::remove(path);
}

TEST_F(DeliverFixture, ADeliveredValueIsFreshInTheFirstStepOnly)
{
    Judged                      judged;
    hypha::testing::StepHarness harness{judged};
    harness.deliver(judged.limited, Float64{3.0}, Time::fromNanos(kStartNanos));

    harness.step(judged, stepAt(kStartNanos + 1));
    EXPECT_TRUE(judged.sawFresh);
    harness.step(judged, stepAt(kStartNanos + 2));
    EXPECT_FALSE(judged.sawFresh);
    EXPECT_TRUE(judged.sawUsable);

    harness.deliver(judged.limited, Float64{4.0}, Time::fromNanos(kStartNanos + 3));
    harness.step(judged, stepAt(kStartNanos + 4));
    EXPECT_TRUE(judged.sawFresh);
    EXPECT_DOUBLE_EQ(judged.limited->data, 4.0);
    harness.step(judged, stepAt(kStartNanos + 5));
    EXPECT_FALSE(judged.sawFresh);
}

TEST_F(DeliverFixture, ADeliveredValueReadsItsSendTimeAndASetValueReadsZero)
{
    Judged                      judged;
    hypha::testing::StepHarness harness{judged};
    EXPECT_FALSE(judged.limited.sentAt().has_value());
    EXPECT_FALSE(judged.limited.sequence().has_value());

    harness.deliver(judged.limited, Float64{3.0}, Time::fromNanos(kStartNanos));
    harness.step(judged, stepAt(kStartNanos + 1));
    ASSERT_TRUE(judged.limited.sentAt().has_value());
    EXPECT_EQ(judged.limited.sentAt()->nanos(), kStartNanos);
    // A harness has no publisher: the sequence and the publisher read zero.
    EXPECT_EQ(judged.limited.sequence(), std::optional<uint64_t>{0U});
    EXPECT_EQ(judged.limited.publisherId(), std::optional<uint64_t>{0U});

    harness.set(judged.limited, Float64{4.0});
    ASSERT_TRUE(judged.limited.sentAt().has_value());
    EXPECT_EQ(judged.limited.sentAt()->nanos(), 0);
}

TEST(StepHarness, SetArrivalsReadZeroStampsAndRefuseAnIndexPastTheEnd)
{
    Relay                       relay;
    hypha::testing::StepHarness harness{relay};
    harness.set(relay.marks, {Float64{1.0}, Float64{2.0}});
    EXPECT_EQ(relay.marks.sentAt(1).nanos(), 0);
    EXPECT_EQ(relay.marks.sequence(0), 0U);
    EXPECT_EQ(relay.marks.publisherId(1), 0U);
    EXPECT_THROW(static_cast<void>(relay.marks.sentAt(2)), std::out_of_range);
}

TEST_F(DeliverFixture, LastKnownKeepsTheValueOfAPortThatWentStale)
{
    Judged                      judged;
    hypha::testing::StepHarness harness{judged};
    harness.deliver(judged.limited, Float64{3.0}, Time::fromNanos(kStartNanos));
    harness.step(judged, stepAt(kStartNanos + 10));
    harness.step(judged, stepAt(kStartNanos + 2 * kLimitNanos));
    EXPECT_FALSE(judged.limited.usable());
    EXPECT_TRUE(judged.limited.has());
    EXPECT_DOUBLE_EQ(judged.limited.lastKnown().data, 3.0);
}

TEST_F(DeliverFixture, AValueFirstJudgedStaleIsNotHeldAsACaptureWouldNotHoldIt)
{
    Judged                      judged;
    hypha::testing::StepHarness harness{judged};
    harness.deliver(judged.limited, Float64{3.0}, Time::fromNanos(kStartNanos));
    harness.step(judged, stepAt(kStartNanos + 2 * kLimitNanos));
    EXPECT_EQ(judged.limited.verdict(), InputVerdict::Stale);
    EXPECT_FALSE(judged.limited.has());
    EXPECT_FALSE(judged.sawFresh);
}

TEST_F(DeliverFixture, ADeliverAfterSetStaleReplacesTheStaleState)
{
    Judged                      judged;
    hypha::testing::StepHarness harness{judged};
    harness.set(judged.limited, Float64{1.0});
    harness.setStale(judged.limited);
    EXPECT_TRUE(judged.limited.stale());

    harness.deliver(judged.limited, Float64{2.0}, Time::fromNanos(kStartNanos));
    harness.step(judged, stepAt(kStartNanos + 1));
    EXPECT_TRUE(judged.limited.usable());
    EXPECT_FALSE(judged.limited.stale());
    EXPECT_DOUBLE_EQ(judged.limited->data, 2.0);
}

TEST_F(DeliverFixture, ASetAfterADeliverReplacesTheDeliveredState)
{
    Judged                      judged;
    hypha::testing::StepHarness harness{judged};
    harness.deliver(judged.limited, Float64{2.0}, Time::fromNanos(kStartNanos));
    harness.set(judged.limited, Float64{9.0});
    harness.step(judged, stepAt(kStartNanos + 10 * kLimitNanos));
    EXPECT_TRUE(judged.limited.usable()) << "set() states a verdict; no age follows";
    EXPECT_DOUBLE_EQ(judged.limited->data, 9.0);

    harness.deliver(judged.limited, Float64{2.0}, Time::fromNanos(kStartNanos));
    harness.setNeverArrived(judged.limited);
    harness.step(judged, stepAt(kStartNanos + 1));
    EXPECT_EQ(judged.limited.verdict(), InputVerdict::NeverArrived);
    EXPECT_FALSE(judged.limited.has());
}

TEST_F(DeliverFixture, ALaterDeliverBeforeAStepReplacesTheEarlierOne)
{
    Judged                      judged;
    hypha::testing::StepHarness harness{judged};
    harness.deliver(judged.limited, Float64{1.0}, Time::fromNanos(kStartNanos));
    harness.deliver(judged.limited, Float64{2.0}, Time::fromNanos(kStartNanos + 5));
    harness.step(judged, stepAt(kStartNanos + 10));
    EXPECT_DOUBLE_EQ(judged.limited->data, 2.0);
}

TEST(StepHarnessDeliver, ADeliverWithoutALoadedConfigurationIsRefused)
{
    Judged                      judged;
    hypha::testing::StepHarness harness{judged};
    EXPECT_THROW(
        harness.deliver(judged.limited, Float64{1.0}, Time::fromNanos(kStartNanos)),
        std::logic_error
    );
    harness.deliver(judged.open, Float64{1.0}, Time::fromNanos(kStartNanos));
    harness.step(judged, stepAt(kStartNanos + 1));
    EXPECT_TRUE(judged.open.usable()) << "an unsupervised port reads no table";
}

/// #271: the counters of an input advance under the harness as they do on the bus. A
/// code that decides "a new sample arrived" from received() or deliveredCount() sees every
/// sample in a step test, not only the first.
TEST(StepHarness, AnInputCountsTheSamplesAStepTestFeedsItAsTheBusDoes)
{
    Relay                       relay;
    hypha::testing::StepHarness harness{relay};
    EXPECT_EQ(relay.speed.received(), 0U);
    EXPECT_EQ(relay.speed.deliveredCount(), 0U);

    harness.set(relay.speed, Float64{1.0});
    harness.step(relay);
    EXPECT_EQ(relay.speed.received(), 1U);
    EXPECT_EQ(relay.speed.deliveredCount(), 1U);

    harness.step(relay);
    EXPECT_EQ(relay.speed.received(), 1U) << "a step with no new sample counts nothing";
    EXPECT_EQ(relay.speed.deliveredCount(), 1U);

    harness.set(relay.speed, Float64{2.0});
    harness.step(relay);
    EXPECT_EQ(relay.speed.received(), 2U);
    EXPECT_EQ(relay.speed.deliveredCount(), 2U);
}

TEST_F(DeliverFixture, ADeliveredLatestValueAdvancesTheCountersOncePerSample)
{
    Judged                      judged;
    hypha::testing::StepHarness harness{judged};
    harness.deliver(judged.limited, Float64{1.0}, Time::fromNanos(kStartNanos));
    EXPECT_EQ(judged.limited.received(), 0U) << "a capture counts it, not the sender";
    harness.step(judged, stepAt(kStartNanos + 1));
    EXPECT_EQ(judged.limited.received(), 1U);
    EXPECT_EQ(judged.limited.deliveredCount(), 1U);

    harness.step(judged, stepAt(kStartNanos + 2));
    EXPECT_EQ(judged.limited.received(), 1U);
    EXPECT_EQ(judged.limited.deliveredCount(), 1U);

    harness.deliver(judged.limited, Float64{2.0}, Time::fromNanos(kStartNanos + 3));
    harness.step(judged, stepAt(kStartNanos + 4));
    EXPECT_EQ(judged.limited.received(), 2U);
    EXPECT_EQ(judged.limited.deliveredCount(), 2U);
}

TEST_F(DeliverFixture, TwoValuesDeliveredBeforeOneStepAreTwoReceivedAndOneDelivered)
{
    Judged                      judged;
    hypha::testing::StepHarness harness{judged};
    harness.deliver(judged.limited, Float64{1.0}, Time::fromNanos(kStartNanos));
    harness.deliver(judged.limited, Float64{2.0}, Time::fromNanos(kStartNanos + 5));
    harness.step(judged, stepAt(kStartNanos + 10));
    EXPECT_EQ(judged.limited.received(), 2U) << "the source accepted both";
    EXPECT_EQ(judged.limited.deliveredCount(), 1U) << "the step took the newest";
}

TEST_F(DeliverFixture, AValueThatIsStaleWhenCapturedIsReceivedAndNotDelivered)
{
    Judged                      judged;
    hypha::testing::StepHarness harness{judged};
    harness.deliver(judged.limited, Float64{1.0}, Time::fromNanos(kStartNanos));
    harness.step(judged, stepAt(kStartNanos + 10 * kLimitNanos));
    EXPECT_EQ(judged.limited.received(), 1U);
    EXPECT_EQ(judged.limited.deliveredCount(), 0U);
}

TEST_F(DeliverFixture, SetNeverArrivedStartsTheCountersOver)
{
    Judged                      judged;
    hypha::testing::StepHarness harness{judged};
    harness.set(judged.limited, Float64{1.0});
    harness.step(judged);
    ASSERT_EQ(judged.limited.received(), 1U);
    harness.setNeverArrived(judged.limited);
    EXPECT_EQ(judged.limited.received(), 0U);
    EXPECT_EQ(judged.limited.deliveredCount(), 0U);
}

namespace {

using UInt32 = std_msgs::UInt32;
using namespace hypha::literals;

/// One Latest port that records, at every step, the time of the step and the verdict the
/// port gave.
class Recorded final : public hypha::CycleNode
{
  public:
    Latest<UInt32> input{*this, "speed"};

    void step(const hypha::CycleTime& time) override
    {
        this->times.push_back(time.now.nanos());
        this->verdicts.push_back(this->input.verdict());
        this->usables.push_back(this->input.usable());
        this->received.push_back(this->input.received());
        this->delivered.push_back(this->input.deliveredCount());
    }

    std::vector<int64_t>      times;
    std::vector<InputVerdict> verdicts;
    std::vector<bool>         usables;
    std::vector<uint64_t>     received;
    std::vector<uint64_t>     delivered;
};

/// What the steps of the started cycle showed, kept after the cycle and its node are gone.
struct Observed
{
    std::vector<int64_t>      times;
    std::vector<InputVerdict> verdicts;
    std::vector<bool>         usables;
    std::vector<uint64_t>     received;
    std::vector<uint64_t>     delivered;
};

/// A value the test sent on the real bus: when, and how many steps had run before.
struct Sent
{
    int64_t sentAt;
    size_t  stepsBefore;
};

}  // namespace

/// Under simulated time a started cycle captures on the simulated clock, so the test fixes
/// both times to the nanosecond: ages of exactly the limit (100 ms) and one step after it
/// occur. A value stamped after the step is not part of the sequence: a capture under
/// simulated time does not admit it at all, where the harness judges it stale.
TEST(StepHarnessDeliver, TheHarnessGivesTheVerdictOfARealCycleForOneInputSequence)
{
    const std::filesystem::path path =
        std::filesystem::temp_directory_path() / "hypha_step_harness_honesty.toml";
    const std::string topic = hypha_test::testTopic("/harness_honesty");
    {
        std::ofstream output(path);
        output << "[ports.inputs.speed]\ntopic='" << topic << "'\nsilence_ms = 100\n";
    }
    ASSERT_TRUE(hypha::enableSimTime(Time::fromNanos(1000000000)));
    std::vector<Sent> sends;
    Observed          real;
    {
        hypha::Node node("harness_honesty", path.string());
        auto        publisher = hypha::Node().advertise<UInt32>(hypha::Topic{topic});
        ASSERT_TRUE(publisher.enabled());
        Recorded cycle;
        cycle.setRate(hypha::Hz(100.0));
        ASSERT_TRUE(cycle.start(node).has_value());
        const auto publish = [&publisher, &sends, &cycle]() {
            sends.push_back(Sent{hypha::now().nanos(), cycle.times.size()});
            return publisher.publish(UInt32{static_cast<uint32_t>(sends.size())});
        };
        const auto slice = [&node]() {
            hypha_test::advance(10_ms);
            return node.runFor(1_ms).ok();
        };
        for (int index = 0; index < 3; ++index) {
            ASSERT_TRUE(slice());
        }
        ASSERT_TRUE(publish());  // at a step time
        for (int index = 0; index < 13; ++index) {
            ASSERT_TRUE(slice());
        }
        hypha_test::advance(3_ms);  // between two steps
        ASSERT_TRUE(node.runFor(1_ms).ok());
        ASSERT_TRUE(publish());
        hypha_test::advance(7_ms);
        ASSERT_TRUE(node.runFor(1_ms).ok());
        for (int index = 0; index < 12; ++index) {
            ASSERT_TRUE(slice());
        }
        ASSERT_TRUE(publish());  // two sends before one step
        hypha_test::advance(4_ms);
        ASSERT_TRUE(node.runFor(1_ms).ok());
        ASSERT_TRUE(publish());
        hypha_test::advance(6_ms);
        ASSERT_TRUE(node.runFor(1_ms).ok());
        for (int index = 0; index < 5; ++index) {
            ASSERT_TRUE(slice());
        }
        real.times = cycle.times;
        real.verdicts = cycle.verdicts;
        real.usables = cycle.usables;
        real.received = cycle.received;
        real.delivered = cycle.delivered;
    }
    ASSERT_GT(real.times.size(), 30U);
    ASSERT_EQ(sends.size(), 4U);

    hypha::ConfigContext        context(path.string());
    Recorded                    simulated;
    hypha::testing::StepHarness harness{simulated};
    size_t                      nextSend = 0;
    size_t                      staleSteps = 0;
    size_t                      usableSteps = 0;
    size_t                      atTheLimit = 0;
    for (size_t index = 0; index < real.times.size(); ++index) {
        while (nextSend < sends.size() && sends[nextSend].stepsBefore <= index) {
            harness.deliver(
                simulated.input,
                UInt32{static_cast<uint32_t>(nextSend + 1)},
                Time::fromNanos(sends[nextSend].sentAt)
            );
            ++nextSend;
        }
        harness.step(simulated, stepAt(real.times[index]));
        EXPECT_EQ(simulated.input.verdict(), real.verdicts[index]) << "step " << index;
        EXPECT_EQ(simulated.input.usable(), static_cast<bool>(real.usables[index]))
            << "step " << index;
        EXPECT_EQ(simulated.input.received(), real.received[index]) << "step " << index;
        EXPECT_EQ(simulated.input.deliveredCount(), real.delivered[index]) << "step " << index;
        const bool stale = real.verdicts[index] == InputVerdict::Stale;
        staleSteps += stale ? 1U : 0U;
        usableSteps += real.usables[index] ? 1U : 0U;
        const bool sentBefore = nextSend != 0;
        const bool onTheLimit =
            sentBefore && real.times[index] - sends[nextSend - 1].sentAt == kLimitNanos;
        atTheLimit += onTheLimit ? 1U : 0U;
    }
    EXPECT_GE(staleSteps, 5U) << "the sequence must cross the limit, or it proves nothing";
    EXPECT_GE(usableSteps, 15U);
    EXPECT_GE(atTheLimit, 1U) << "a step at an age of exactly the limit must occur";
    EXPECT_EQ(real.received.back(), 4U) << "all four sends were accepted";
    EXPECT_LT(real.delivered.back(), real.received.back()) << "two sends shared one step";
    std::filesystem::remove(path);
}

namespace {

/// One Arrivals port with the 100 ms limit of the fixture; the step records what it showed.
class Queued final : public hypha::CycleNode
{
  public:
    Arrivals<Float64> queue{*this, "queue"};

    void step(const hypha::CycleTime&) override
    {
        this->seen.clear();
        for (const Float64& sample : this->queue.samples()) {
            this->seen.push_back(sample.data);
        }
        this->sawVerdict = this->queue.verdict();
    }

    std::vector<double> seen;
    InputVerdict        sawVerdict{InputVerdict::NeverArrived};
};

}  // namespace

TEST_F(DeliverFixture, DeliveredArrivalsGoToOneStepAndThePortGoesStaleByItsNewestSample)
{
    Queued                      queued;
    hypha::testing::StepHarness harness{queued};
    const int64_t               newest = kStartNanos + 10;
    harness.deliver(queued.queue, {Float64{1.0}, Float64{2.0}}, Time::fromNanos(newest));

    harness.step(queued, stepAt(kStartNanos + 20));
    EXPECT_EQ(queued.seen, (std::vector<double>{1.0, 2.0}));
    EXPECT_EQ(queued.sawVerdict, InputVerdict::Usable);

    // No new sample: usable while the newest one is within the limit, inclusive.
    harness.step(queued, stepAt(newest + kLimitNanos));
    EXPECT_TRUE(queued.seen.empty());
    EXPECT_EQ(queued.sawVerdict, InputVerdict::Usable);
    EXPECT_FALSE(queued.queue.stale());
    EXPECT_TRUE(queued.inputsOk());

    harness.step(queued, stepAt(newest + kLimitNanos + 1));
    EXPECT_TRUE(queued.seen.empty());
    EXPECT_EQ(queued.sawVerdict, InputVerdict::Stale);
    EXPECT_TRUE(queued.queue.stale());
    EXPECT_FALSE(queued.inputsOk());

    // A new delivery ends the staleness.
    const int64_t later = kStartNanos + 500'000'000;
    harness.deliver(queued.queue, {Float64{3.0}}, Time::fromNanos(later));
    harness.step(queued, stepAt(later + 1));
    EXPECT_EQ(queued.seen, (std::vector<double>{3.0}));
    EXPECT_EQ(queued.sawVerdict, InputVerdict::Usable);
}

TEST_F(DeliverFixture, ArrivalsFirstJudgedStaleAreNotDeliveredAsACaptureRefusesThem)
{
    Queued                      queued;
    hypha::testing::StepHarness harness{queued};
    harness.deliver(queued.queue, {Float64{4.0}}, Time::fromNanos(kStartNanos));
    harness.step(queued, stepAt(kStartNanos + 10 * kLimitNanos));
    EXPECT_TRUE(queued.seen.empty());
    EXPECT_EQ(queued.sawVerdict, InputVerdict::Stale);
}

TEST_F(DeliverFixture, ASetAfterDeliveredArrivalsReplacesTheDeliveredState)
{
    Queued                      queued;
    hypha::testing::StepHarness harness{queued};
    harness.deliver(queued.queue, {Float64{1.0}}, Time::fromNanos(kStartNanos));
    harness.set(queued.queue, {Float64{7.0}});
    harness.step(queued, stepAt(kStartNanos + 50 * kLimitNanos));
    EXPECT_EQ(queued.seen, (std::vector<double>{7.0}));
    EXPECT_EQ(queued.sawVerdict, InputVerdict::Usable);
    harness.step(queued, stepAt(kStartNanos + 60 * kLimitNanos));
    EXPECT_EQ(queued.sawVerdict, InputVerdict::Usable);
}

TEST_F(DeliverFixture, DeliveredArrivalsAreCountedBySampleAndOnlyWhenUsable)
{
    Queued                      queued;
    hypha::testing::StepHarness harness{queued};
    harness.deliver(queued.queue, {Float64{1.0}, Float64{2.0}}, Time::fromNanos(kStartNanos));
    harness.step(queued, stepAt(kStartNanos + 1));
    EXPECT_EQ(queued.queue.received(), 2U);
    EXPECT_EQ(queued.queue.deliveredCount(), 2U);

    harness.step(queued, stepAt(kStartNanos + 2));
    EXPECT_EQ(queued.queue.received(), 2U);
    EXPECT_EQ(queued.queue.deliveredCount(), 2U);

    harness.deliver(queued.queue, {Float64{3.0}}, Time::fromNanos(kStartNanos + 3));
    harness.step(queued, stepAt(kStartNanos + 4));
    EXPECT_EQ(queued.queue.received(), 3U);
    EXPECT_EQ(queued.queue.deliveredCount(), 3U);

    harness.deliver(queued.queue, {Float64{4.0}}, Time::fromNanos(kStartNanos));
    harness.step(queued, stepAt(kStartNanos + 10 * kLimitNanos));
    EXPECT_EQ(queued.queue.received(), 4U) << "accepted";
    EXPECT_EQ(queued.queue.deliveredCount(), 3U) << "but too old to deliver";
}

TEST_F(DeliverFixture, SetArrivalsAreCountedBySample)
{
    Queued                      queued;
    hypha::testing::StepHarness harness{queued};
    harness.set(queued.queue, {Float64{1.0}, Float64{2.0}, Float64{3.0}});
    harness.step(queued);
    EXPECT_EQ(queued.queue.received(), 3U);
    EXPECT_EQ(queued.queue.deliveredCount(), 3U);
    harness.step(queued);
    EXPECT_EQ(queued.queue.received(), 3U);
}

namespace {

/// A step that reads the framework clock instead of its CycleTime, as ported code may.
class ReadsTheClock final : public hypha::CycleNode
{
  public:
    void step(const hypha::CycleTime&) override
    {
        this->clock = hypha::now().nanos();
    }

    int64_t clock{0};
};

}  // namespace

// The harness passes the step its CycleTime and does not move the framework clock. A test
// of code that calls hypha::now() sets simulated time to the step's time itself.
TEST(StepHarness, NowInAStepIsTheStepTimeWhenTheTestSetsSimulatedTime)
{
    ReadsTheClock               node;
    hypha::testing::StepHarness harness{node};
    const hypha::CycleTime      first = stepAt(kStartNanos);
    const hypha::CycleTime      second = stepAt(kStartNanos + kLimitNanos);

    harness.step(node, first);
    EXPECT_NE(node.clock, first.now.nanos());

    ASSERT_TRUE(hypha::enableSimTime(first.now));
    harness.step(node, first);
    EXPECT_EQ(node.clock, first.now.nanos());
    ASSERT_TRUE(hypha::setSimTime(second.now));
    harness.step(node, second);
    EXPECT_EQ(node.clock, second.now.nanos());
    ASSERT_TRUE(hypha::disableSimTime());
}

namespace {

/// A magnetometer-like node: every port is optional, and each has its own state in the
/// parameter file of OptionalPortsFixture.
///   mag    wired, 100 ms limit        debug  wired
///   aux    `topic = ""`               tf     no table
///   absent no table
class Optionals final : public hypha::CycleNode
{
  public:
    Arrivals<Float64> mag{*this, "mag", optionalTopic};
    Latest<Float64>   tilt{*this, "tilt", optionalTopic};
    Arrivals<Float64> aux{*this, "aux", optionalTopic};
    Latest<Float64>   absent{*this, "absent", optionalTopic};
    Output<Float64>   debug{*this, "debug", optionalTopic};
    Output<Float64>   tf{*this, "tf", optionalTopic};

    /// Reads an input only behind its enabled(), as a step with an optional input does, and
    /// sends on both outputs, which a disabled output accepts and drops.
    void step(const hypha::CycleTime&) override
    {
        if (this->mag.enabled()) {
            this->sawMag.assign(this->mag.samples().begin(), this->mag.samples().end());
            this->sawMagVerdict = this->mag.verdict();
        }
        if (this->tilt.enabled()) {
            this->sawTiltVerdict = this->tilt.verdict();
        }
        this->debug.send(Float64{1.0});
        this->tf.send(Float64{2.0});
    }

    std::vector<bool> enabledPorts() const
    {
        return {
            this->mag.enabled(),
            this->tilt.enabled(),
            this->aux.enabled(),
            this->absent.enabled(),
            this->debug.enabled(),
            this->tf.enabled(),
        };
    }

    /// What the step read; no verdict when it skipped the port because it is disabled.
    std::vector<Float64>        sawMag;
    std::optional<InputVerdict> sawMagVerdict;
    std::optional<InputVerdict> sawTiltVerdict;
};

/// The tables of Optionals, with `topic` set for `mag`, `tilt` and `debug` only.
std::string optionalPortsFile(const std::string& topicPrefix)
{
    std::string text;
    for (const std::string input : {"mag", "tilt"}) {
        text += "[ports.inputs." + input + "]\ntopic = '" + topicPrefix + "/" + input + "'\n";
        text += "silence_ms = 100\n";
    }
    text += "[ports.inputs.aux]\ntopic = ''\n";
    text += "[ports.outputs.debug]\ntopic = '" + topicPrefix + "/debug'\n";
    return text;
}

/// A parameter file that is removed when the test ends, also when an assertion failed.
class TempToml
{
  public:
    TempToml(const std::string& name, const std::string& text)
        : path_(std::filesystem::temp_directory_path() / name)
    {
        std::ofstream(this->path_) << text;
    }

    TempToml(const TempToml&) = delete;
    TempToml& operator=(const TempToml&) = delete;

    ~TempToml()
    {
        std::error_code ignored;
        std::filesystem::remove(this->path_, ignored);
    }

    std::string path() const
    {
        return this->path_.string();
    }

  private:
    std::filesystem::path path_;
};

class OptionalPortsFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->path =
            std::filesystem::temp_directory_path() / "hypha_step_harness_optional_ports.toml";
        std::ofstream(this->path) << optionalPortsFile("/harness");
        this->context = std::make_unique<hypha::ConfigContext>(this->path.string());
    }

    void TearDown() override
    {
        this->context.reset();
        std::filesystem::remove(this->path);
    }

    std::unique_ptr<hypha::ConfigContext> context;
    std::filesystem::path                 path;
};

}  // namespace

TEST_F(OptionalPortsFixture, APortThatTheFileWiresIsEnabledAndOneItLeavesOutIsNot)
{
    Optionals                   node;
    hypha::testing::StepHarness harness{node};
    EXPECT_EQ(node.enabledPorts(), (std::vector<bool>{true, true, false, false, true, false}));
}

TEST_F(OptionalPortsFixture, AWiredOptionalArrivalsPortGivesEachOfTheThreeVerdicts)
{
    Optionals                   node;
    hypha::testing::StepHarness harness{node};

    harness.step(node, stepAt(kStartNanos));
    EXPECT_EQ(node.sawMagVerdict, InputVerdict::NeverArrived);
    EXPECT_TRUE(node.sawMag.empty());

    harness.deliver(node.mag, {Float64{1.0}, Float64{2.0}}, Time::fromNanos(kStartNanos));
    harness.step(node, stepAt(kStartNanos + kLimitNanos));
    EXPECT_EQ(node.sawMagVerdict, InputVerdict::Usable);
    ASSERT_EQ(node.sawMag.size(), 2U);
    EXPECT_DOUBLE_EQ(node.sawMag[1].data, 2.0);

    harness.step(node, stepAt(kStartNanos + kLimitNanos + 1));
    EXPECT_EQ(node.sawMagVerdict, InputVerdict::Stale);
    EXPECT_TRUE(node.sawMag.empty());
    EXPECT_TRUE(node.mag.stale());
}

TEST_F(OptionalPortsFixture, AWiredOptionalLatestPortGivesEachOfTheThreeVerdicts)
{
    Optionals                   node;
    hypha::testing::StepHarness harness{node};

    harness.step(node, stepAt(kStartNanos));
    EXPECT_EQ(node.sawTiltVerdict, InputVerdict::NeverArrived);

    harness.deliver(node.tilt, Float64{0.5}, Time::fromNanos(kStartNanos));
    harness.step(node, stepAt(kStartNanos + kLimitNanos));
    EXPECT_EQ(node.sawTiltVerdict, InputVerdict::Usable);
    EXPECT_DOUBLE_EQ(node.tilt->data, 0.5);

    harness.step(node, stepAt(kStartNanos + kLimitNanos + 1));
    EXPECT_EQ(node.sawTiltVerdict, InputVerdict::Stale);
    EXPECT_DOUBLE_EQ(node.tilt.lastKnown().data, 0.5);
}

TEST_F(OptionalPortsFixture, ADeliverToADisabledPortIsRefusedWithTheReason)
{
    Optionals                   node;
    hypha::testing::StepHarness harness{node};
    const auto                  reasonOf = [](const auto& deliver) {
        try {
            deliver();
        } catch (const std::logic_error& error) {
            return std::string(error.what());
        }
        return std::string{};
    };

    const std::string noTable = reasonOf([&]() {
        harness.deliver(node.absent, Float64{1.0}, Time::fromNanos(kStartNanos));
    });
    EXPECT_NE(noTable.find("deliver()"), std::string::npos) << noTable;
    EXPECT_NE(noTable.find("ports.inputs.absent"), std::string::npos) << noTable;
    EXPECT_NE(noTable.find("disabled"), std::string::npos) << noTable;
    EXPECT_NE(noTable.find("`topic`"), std::string::npos) << noTable;

    const std::string emptyTopic = reasonOf([&]() {
        harness.deliver(node.aux, {Float64{1.0}}, Time::fromNanos(kStartNanos));
    });
    EXPECT_NE(emptyTopic.find("ports.inputs.aux"), std::string::npos) << emptyTopic;

    harness.step(node, stepAt(kStartNanos + 1));
    EXPECT_EQ(node.absent.verdict(), InputVerdict::NeverArrived);
    EXPECT_FALSE(node.absent.has());
    EXPECT_TRUE(node.aux.samples().empty());
}

TEST_F(OptionalPortsFixture, AnOutputTheFileWiresKeepsItsSendsAndOneItLeavesOutDropsThem)
{
    Optionals                   node;
    hypha::testing::StepHarness harness{node};
    harness.step(node, stepAt(kStartNanos));

    ASSERT_EQ(harness.sent(node.debug).size(), 1U);
    EXPECT_DOUBLE_EQ(harness.sent(node.debug)[0].data, 1.0);
    EXPECT_TRUE(harness.sent(node.tf).empty()) << "a disabled output sends nothing";
    EXPECT_TRUE(node.tf.sent()) << "yet the step did send, and the port counts it";
    EXPECT_EQ(node.tf.sentCount(), 1U);
}

TEST(StepHarnessOptionalPorts, WithoutAParameterFileEveryOptionalPortIsDisabled)
{
    Optionals                   node;
    hypha::testing::StepHarness harness{node};
    EXPECT_EQ(node.enabledPorts(), std::vector<bool>(6, false));
    EXPECT_THROW(
        harness.deliver(node.mag, {Float64{1.0}}, Time::fromNanos(kStartNanos)), std::logic_error
    );
}

TEST(StepHarnessOptionalPorts, APortThatRequiresItsTopicIsEnabledWithOrWithoutAFile)
{
    Relay                       relay;
    hypha::testing::StepHarness harness{relay};
    EXPECT_TRUE(relay.speed.enabled());
    EXPECT_TRUE(relay.marks.enabled());
    EXPECT_TRUE(relay.command.enabled());
}

TEST_F(OptionalPortsFixture, TheHarnessLeavesThePortsAsItFoundThemWhenItIsGone)
{
    Optionals node;
    {
        hypha::testing::StepHarness harness{node};
        ASSERT_TRUE(node.mag.enabled());
    }
    EXPECT_EQ(node.enabledPorts(), std::vector<bool>(6, false));
}

TEST(StepHarnessOptionalPorts, ATopicNameAStartWouldRefuseIsRefusedAndTheCycleStaysFree)
{
    const TempToml file(
        "hypha_step_harness_bad_topic.toml", "[ports.inputs.mag]\ntopic = 'no slash'\n"
    );
    const hypha::ConfigContext context(file.path());
    Optionals                  node;
    EXPECT_THROW(hypha::testing::StepHarness{node}, hypha::ConfigError);
    EXPECT_THROW(hypha::testing::StepHarness{node}, hypha::ConfigError)
        << "the refusal left no harness holding the cycle";
}

TEST(StepHarnessOptionalPorts, AnEntryThatIsAPlainStringIsRefusedAsAStartRefusesIt)
{
    const TempToml file(
        "hypha_step_harness_plain_entry.toml", "[ports.inputs]\nmag = '/plain/mag'\n"
    );
    {
        const hypha::ConfigContext context(file.path());
        Optionals                  node;
        try {
            hypha::testing::StepHarness harness{node};
            FAIL() << "a plain string entry must be refused";
        } catch (const hypha::ConfigError& error) {
            EXPECT_NE(std::string(error.what()).find("ports.inputs.mag"), std::string::npos)
                << error.what();
        }
    }
    hypha::Node node("harness_plain_entry", file.path());
    Optionals   cycle;
    cycle.setRate(hypha::Hz(100.0));
    EXPECT_FALSE(cycle.start(node).has_value()) << "a start refuses it as well";
}

TEST_F(OptionalPortsFixture, ASetOrAStaleVerdictOnADisabledInputIsRefusedAndNeverArrivedIsNot)
{
    Optionals                   node;
    hypha::testing::StepHarness harness{node};
    EXPECT_THROW(harness.set(node.absent, Float64{1.0}), std::logic_error);
    EXPECT_THROW(harness.setStale(node.absent), std::logic_error);
    EXPECT_THROW(harness.setStagnant(node.absent, true), std::logic_error);
    EXPECT_THROW(harness.set(node.aux, {Float64{1.0}}), std::logic_error);
    EXPECT_THROW(harness.setStale(node.aux), std::logic_error);
    try {
        harness.set(node.absent, Float64{1.0});
    } catch (const std::logic_error& error) {
        EXPECT_NE(
            std::string(error.what()).find("set() on input ports.inputs.absent"), std::string::npos
        ) << error.what();
    }

    harness.setNeverArrived(node.absent);
    harness.setNeverArrived(node.aux);
    harness.step(node, stepAt(kStartNanos));
    EXPECT_EQ(node.absent.verdict(), InputVerdict::NeverArrived);
    EXPECT_FALSE(node.absent.has());

    harness.set(node.tilt, Float64{1.0});
    EXPECT_TRUE(node.tilt.usable()) << "a wired port takes a verdict";
}

TEST_F(OptionalPortsFixture, AnEnabledOutputReportsASubscriberAndADisabledOneDoesNot)
{
    Optionals node;
    EXPECT_FALSE(node.debug.hasSubscribers());
    hypha::testing::StepHarness harness{node};
    EXPECT_TRUE(node.debug.hasSubscribers());
    EXPECT_FALSE(node.tf.hasSubscribers());
    EXPECT_FALSE(node.debug.subscriberCount().has_value()) << "no transport counts one";
}

TEST(StepHarnessOptionalPorts, TwoHarnessesOneAfterTheOtherEachReadTheirOwnFile)
{
    Optionals node;
    {
        const TempToml file(
            "hypha_step_harness_first.toml",
            "[ports.inputs.mag]\ntopic = '/first/mag'\nsilence_ms = 100\n"
        );
        const hypha::ConfigContext  context(file.path());
        hypha::testing::StepHarness harness{node};
        EXPECT_EQ(
            node.enabledPorts(), (std::vector<bool>{true, false, false, false, false, false})
        );
    }
    EXPECT_EQ(node.enabledPorts(), std::vector<bool>(6, false));
    {
        const TempToml file(
            "hypha_step_harness_second.toml",
            "[ports.inputs.tilt]\ntopic = '/second/tilt'\nsilence_ms = 100\n"
        );
        const hypha::ConfigContext  context(file.path());
        hypha::testing::StepHarness harness{node};
        EXPECT_EQ(
            node.enabledPorts(), (std::vector<bool>{false, true, false, false, false, false})
        );
    }
}

TEST(StepHarnessOptionalPorts, ANodeStartedAfterAHarnessEnablesWhatItsOwnFileSays)
{
    Optionals node;
    {
        const TempToml file("hypha_step_harness_before_start.toml", optionalPortsFile("/before"));
        const hypha::ConfigContext  context(file.path());
        hypha::testing::StepHarness harness{node};
        ASSERT_TRUE(node.mag.enabled());
    }
    const std::string topic = hypha_test::testTopic("/after_harness");
    const TempToml    file(
        "hypha_step_harness_after_harness.toml",
        "[ports.inputs.tilt]\ntopic = '" + topic
            + "/tilt'\nsilence_ms = 100\n"
              "[ports.outputs.tf]\ntopic = '"
            + topic + "/tf'\n"
    );
    hypha::Node node2("harness_after", file.path());
    node.setRate(hypha::Hz(100.0));
    const std::expected<void, std::string> started = node.start(node2);
    ASSERT_TRUE(started.has_value()) << started.error();
    EXPECT_EQ(node.enabledPorts(), (std::vector<bool>{false, true, false, false, false, true}));
}

TEST_F(DeliverFixture, APortThatRequiresItsTopicIsEnabledEvenWhenItsTableLacksOne)
{
    Judged                      judged;
    hypha::testing::StepHarness harness{judged};
    EXPECT_TRUE(judged.limited.enabled())
        << "a start would refuse the table; the harness does not check it";
    EXPECT_TRUE(judged.open.enabled());
}

/// What a start enables is what the harness enables, for the same parameter file.
TEST(StepHarnessOptionalPorts, TheHarnessEnablesTheOptionalPortsThatAStartEnables)
{
    const TempToml file(
        "hypha_step_harness_enabled_like_start.toml",
        optionalPortsFile(hypha_test::testTopic("/harness_enabled"))
    );
    std::vector<bool> started;
    {
        hypha::Node node("harness_enabled", file.path());
        Optionals   cycle;
        cycle.setRate(hypha::Hz(100.0));
        const std::expected<void, std::string> result = cycle.start(node);
        ASSERT_TRUE(result.has_value()) << result.error();
        started = cycle.enabledPorts();
    }
    const hypha::ConfigContext  context(file.path());
    Optionals                   node;
    hypha::testing::StepHarness harness{node};
    EXPECT_EQ(node.enabledPorts(), started);
    EXPECT_EQ(started, (std::vector<bool>{true, true, false, false, true, false}));
}

// ---- #248: the Transforms port under the harness ----------------------------------

namespace {

using Looked = std::expected<geometry_msgs::TransformStamped, hypha::TransformError>;

/// One question a step asks its Transforms port.
struct Question
{
    std::string            target;
    std::string            source;
    std::optional<int64_t> at;  // a lookup at this time, else the latest common time
};

/// A step that asks its Transforms port each of its questions.
class Mapper final : public hypha::CycleNode
{
  public:
    Mapper()
    {
        this->setRate(hypha::Hz(100.0));
    }

    hypha::Transforms     tf{*this};
    std::vector<Question> questions;
    std::vector<Looked>   answers;

    void step(const hypha::CycleTime&) override
    {
        this->answers.clear();
        for (const Question& question : this->questions) {
            this->answers.push_back(
                question.at.has_value()
                    ? this->tf.lookup(
                          question.target, question.source, Time::fromNanos(*question.at)
                      )
                    : this->tf.lookupLatest(question.target, question.source)
            );
        }
    }
};

constexpr int64_t kSecond = 1'000'000'000;

geometry_msgs::TransformStamped stamped(
    const std::string& parent, const std::string& child, uint32_t seconds, double x, double w = 1.0
)
{
    geometry_msgs::TransformStamped item{};
    item.header.frame_id = parent;
    item.header.stamp.secs = seconds;
    item.child_frame_id = child;
    item.transform.translation.x = x;
    item.transform.rotation.w = w;
    return item;
}

}  // namespace

TEST(StepHarnessTransforms, AStepThatLooksUpATransformGetsTheAnswerOfAnEmptyBuffer)
{
    Mapper mapper;
    mapper.questions = {{"map", "lidar", std::nullopt}, {"map", "lidar", kSecond}};
    hypha::testing::StepHarness harness{mapper};
    harness.step(mapper);
    ASSERT_EQ(mapper.answers.size(), 2U);
    for (const Looked& answer : mapper.answers) {
        ASSERT_FALSE(answer.has_value());
        EXPECT_EQ(answer.error(), hypha::TransformError::UnknownFrame);
    }
}

TEST(StepHarnessTransforms, ATransformThatWasNotDeliveredIsMissedAsACycleMissesIt)
{
    Mapper                      mapper;
    hypha::testing::StepHarness harness{mapper};
    harness.deliver(mapper.tf, {stamped("map", "base", 5, 1.0), stamped("map", "base", 6, 2.0)});
    harness.deliver(mapper.tf, {stamped("odom", "wheel", 5, 1.0)});
    mapper.questions = {
        {"map", "lidar", 5 * kSecond},
        {"map", "base", 4 * kSecond},
        {"map", "base", 7 * kSecond},
        {"map", "wheel", 5 * kSecond},
        {"", "base", std::nullopt},
    };
    harness.step(mapper);
    const std::vector<hypha::TransformError> expected{
        hypha::TransformError::UnknownFrame,
        hypha::TransformError::BeforeBuffer,
        hypha::TransformError::AfterBuffer,
        hypha::TransformError::NotConnected,
        hypha::TransformError::InvalidFrame,
    };
    ASSERT_EQ(mapper.answers.size(), expected.size());
    for (size_t index = 0; index < expected.size(); ++index) {
        ASSERT_FALSE(mapper.answers[index].has_value()) << "question " << index;
        EXPECT_EQ(mapper.answers[index].error(), expected[index]) << "question " << index;
    }
}

TEST(StepHarnessTransforms, ATransformTheCycleRefusesIsLeftOutAndItsFrameAnswersRefused)
{
    Mapper                      mapper;
    hypha::testing::StepHarness harness{mapper};
    harness.deliver(mapper.tf, {stamped("map", "base", 1, 1.0, 0.0)});  // no unit quaternion
    mapper.questions = {{"map", "base", kSecond}};
    harness.step(mapper);
    ASSERT_FALSE(mapper.answers.at(0).has_value());
    EXPECT_EQ(mapper.answers[0].error(), hypha::TransformError::Refused);
}

TEST(StepHarnessTransforms, ALookupOutsideAStepStillThrows)
{
    Mapper                      mapper;
    hypha::testing::StepHarness harness{mapper};
    harness.deliver(mapper.tf, {stamped("map", "base", 1, 1.0)});
    EXPECT_THROW(static_cast<void>(mapper.tf.lookupLatest("map", "base")), std::logic_error);
}

TEST(StepHarnessTransforms, ACacheThatAStartRefusesIsRefusedByDeliver)
{
    class NoCache final : public hypha::CycleNode
    {
      public:
        NoCache()
        {
            this->setRate(hypha::Hz(100.0));
        }

        hypha::Transforms tf{*this, hypha::Duration::fromNanos(0)};

        void step(const hypha::CycleTime&) override
        {
        }
    };

    NoCache                     node;
    hypha::testing::StepHarness harness{node};
    EXPECT_THROW(harness.deliver(node.tf, {stamped("map", "base", 1, 1.0)}), std::invalid_argument);
}
