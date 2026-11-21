// testing::StepHarness runs a step with no node and no bus. Only the last test starts a
// hypha::Node, to compare the harness with a real cycle.

#include "hypha/config.h"
#include "hypha/cycle_node.h"
#include "hypha/hypha.h"
#include "hypha/testing.h"
#include <hypha_msgs/std_msgs/Float64.h>
#include <hypha_msgs/std_msgs/UInt32.h>

#include "test_types.h"

#include <gtest/gtest.h>

#include <chrono>
#include <cstdint>
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

/// A parameter file with a 100 ms limit on `limited`, no limit on `loose`, and a table
/// for `open`, which is unsupervised.
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

TEST_F(DeliverFixture, ADeliveredValueIsUsableAtTheLimitAndStaleOneNanosecondAfter)
{
    Judged                      judged;
    hypha::testing::StepHarness harness{judged};
    harness.deliver(judged.limited, Float64{3.0}, Time::fromNanos(kStartNanos));

    harness.step(judged, stepAt(kStartNanos + kLimitNanos));
    EXPECT_TRUE(judged.limited.usable());
    EXPECT_FALSE(judged.limited.stale());
    EXPECT_EQ(judged.limited.verdict(), InputVerdict::Usable);
    EXPECT_DOUBLE_EQ(judged.limited->data, 3.0);
    EXPECT_TRUE(judged.inputsOk());

    harness.step(judged, stepAt(kStartNanos + kLimitNanos + 1));
    EXPECT_FALSE(judged.limited.usable());
    EXPECT_TRUE(judged.limited.stale());
    EXPECT_EQ(judged.limited.verdict(), InputVerdict::Stale);
    EXPECT_FALSE(judged.inputsOk());
    EXPECT_THROW(static_cast<void>(*judged.limited), hypha::InputNotUsable);
}

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

TEST_F(DeliverFixture, APortWithoutALimitIsNeverStale)
{
    Judged                      judged;
    hypha::testing::StepHarness harness{judged};
    harness.deliver(judged.loose, Float64{1.0}, Time::fromNanos(kStartNanos));
    harness.step(judged, stepAt(kStartNanos + 3'600'000'000'000));
    EXPECT_TRUE(judged.loose.usable());
    EXPECT_FALSE(judged.loose.stale());
    EXPECT_EQ(judged.loose.verdict(), InputVerdict::Usable);
}

TEST_F(DeliverFixture, AnUnsupervisedPortIsNeverStale)
{
    Judged                      judged;
    hypha::testing::StepHarness harness{judged};
    harness.deliver(judged.open, Float64{1.0}, Time::fromNanos(kStartNanos));
    harness.step(judged, stepAt(kStartNanos + 3'600'000'000'000));
    EXPECT_TRUE(judged.open.usable());
    EXPECT_FALSE(judged.open.stale());
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

TEST_F(DeliverFixture, AValueSentAfterTheStepIsStaleWithAgeZeroUntilItsTimeComes)
{
    Judged                      judged;
    hypha::testing::StepHarness harness{judged};
    harness.deliver(judged.limited, Float64{3.0}, Time::fromNanos(kStartNanos + 50));

    harness.step(judged, stepAt(kStartNanos + 49));
    EXPECT_EQ(judged.limited.verdict(), InputVerdict::Stale);
    EXPECT_FALSE(judged.limited.has());
    try {
        static_cast<void>(*judged.limited);
        FAIL() << "a value from the future is not usable";
    } catch (const hypha::InputNotUsable& error) {
        EXPECT_NE(std::string(error.what()).find("newest sample is 0 ms old"), std::string::npos)
            << error.what();
    }

    harness.step(judged, stepAt(kStartNanos + 50));
    EXPECT_TRUE(judged.limited.usable());
    EXPECT_TRUE(judged.sawFresh);
    EXPECT_DOUBLE_EQ(judged.limited->data, 3.0);
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
    }

    std::vector<int64_t>      times;
    std::vector<InputVerdict> verdicts;
    std::vector<bool>         usables;
};

/// What the steps of the started cycle showed, kept after the cycle and its node are gone.
struct Observed
{
    std::vector<int64_t>      times;
    std::vector<InputVerdict> verdicts;
    std::vector<bool>         usables;
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
