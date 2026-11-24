// testing::McapReplay replays test/data/mcap_replay_fixture.mcap into a StepHarness. Base
// B = 100 s, a 50 ms period; every message's publish time is 3 ms before its log time.
//
//   /position (Latest, silence_ms = 120): 1.0 at B+10, 2.0 at B+60, 3.0 at B+100 (on a
//                                         deadline), 4.0 at B+260
//   /marks    (Arrivals, silence_ms = 1000): 1 at B+20, 2 at B+70, 3 at B+90, 4 at B+210
//   /golden   (Float64): 10 at B+50, 20 at B+100, 30 at B+150, 30 at B+200, 40 at B+300

#include "hypha/config.h"
#include "hypha/cycle_node.h"
#include "hypha/hypha.h"
#include "hypha/testing.h"
#include "hypha/testing/mcap_replay.h"

#include "test_types.h"

#include <gtest/gtest.h>
#include <std_msgs/Float64.h>
#include <std_msgs/UInt32.h>

#include <algorithm>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <memory>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

using Float64 = std_msgs::Float64;
using UInt32 = std_msgs::UInt32;
using hypha::InputVerdict;
using hypha::Time;
using hypha::testing::McapReplay;
using hypha::testing::Recorded;
using hypha::testing::StepHarness;

namespace {

const std::string kFixture = std::string(HYPHA_TEST_DATA_DIR) + "/mcap_replay_fixture.mcap";
const std::string kZstdFixture =
    std::string(HYPHA_TEST_DATA_DIR) + "/mcap_replay_fixture_zstd.mcap";

constexpr int64_t kBase = 100'000'000'000;
constexpr int64_t kMilli = 1'000'000;
constexpr int64_t kPeriod = 50 * kMilli;
const hypha::Hz   kRate{20.0};

struct Seen
{
    int64_t                now{0};
    uint32_t               stampSecs{0};
    uint32_t               stampNsecs{0};
    std::optional<int64_t> dt;
    uint64_t               missed{0};
    InputVerdict           verdict{InputVerdict::NeverArrived};
    bool                   usable{false};
    bool                   fresh{false};
    std::optional<double>  lastKnown;
    std::vector<uint32_t>  marks;
    InputVerdict           marksVerdict{InputVerdict::NeverArrived};

    bool operator==(const Seen&) const = default;
};

/// A Latest and an Arrivals input; sends ten times the position while it is usable.
class Tracker final : public hypha::CycleNode
{
  public:
    Latest<Float64>   position{*this, "position"};
    Arrivals<UInt32>  marks{*this, "marks"};
    Latest<Float64>   spare{*this, "spare", hypha::optionalTopic};
    Output<Float64>   estimate{*this, "estimate"};
    std::vector<Seen> steps;

    void step(const hypha::CycleTime& time) override
    {
        Seen seen;
        seen.now = time.now.nanos();
        seen.stampSecs = time.stamp.secs;
        seen.stampNsecs = time.stamp.nsecs;
        if (time.dt.has_value()) {
            seen.dt = time.dt->nanos();
        }
        seen.missed = time.missed;
        seen.verdict = this->position.verdict();
        seen.usable = this->position.usable();
        seen.fresh = this->position.fresh();
        if (this->position.has()) {
            seen.lastKnown = this->position.lastKnown().data;
        }
        for (const UInt32& mark : this->marks.samples()) {
            seen.marks.push_back(mark.data);
        }
        seen.marksVerdict = this->marks.verdict();
        this->steps.push_back(seen);
        if (this->position.usable()) {
            this->estimate.send(Float64{this->position->data * 10.0});
        }
    }
};

/// One Latest input whose topic the test sets.
class Single final : public hypha::CycleNode
{
  public:
    Latest<Float64> input{*this, "input"};

    void step(const hypha::CycleTime&) override
    {
    }
};

/// A step as the fixture makes it: the deadline at B + @p deadlineMs, the position input
/// with @p verdict, @p fresh and the last value @p value, and the marks of the step.
Seen expected(
    int64_t deadlineMs, InputVerdict verdict, bool fresh, double value, std::vector<uint32_t> marks
)
{
    Seen seen;
    seen.now = kBase + deadlineMs * kMilli;
    seen.stampSecs = 100;
    seen.stampNsecs = static_cast<uint32_t>(deadlineMs * kMilli);
    if (deadlineMs != 50) {
        seen.dt = kPeriod;
    }
    seen.missed = 0;
    seen.verdict = verdict;
    seen.usable = verdict == InputVerdict::Usable;
    seen.fresh = fresh;
    seen.lastKnown = value;
    seen.marks = std::move(marks);
    seen.marksVerdict = InputVerdict::Usable;
    return seen;
}

std::vector<Seen> expectedSteps()
{
    std::vector<Seen> steps;
    // First deadline: the first multiple of 50 ms strictly after B+10, the first sample.
    // 1.0 sent at B+10 is 40 ms old; mark 1 sent at B+20.
    steps.push_back(expected(50, InputVerdict::Usable, true, 1.0, {1}));
    // 2.0 sent at B+60, 40 ms old; two arrivals, 2 and 3. 3.0 sent at B+100 (published at
    // B+97) is not before this deadline: the send time is the log time.
    steps.push_back(expected(100, InputVerdict::Usable, true, 2.0, {2, 3}));
    // 3.0, sent exactly on the previous deadline, is this step's: 50 ms old. No arrival.
    steps.push_back(expected(150, InputVerdict::Usable, true, 3.0, {}));
    // No new sample at all: 3.0 is 100 ms old, within 120 ms.
    steps.push_back(expected(200, InputVerdict::Usable, false, 3.0, {}));
    // 3.0 is 150 ms old: stale by silence_ms; lastKnown() keeps it. Mark 4 from B+210.
    steps.push_back(expected(250, InputVerdict::Stale, false, 3.0, {4}));
    // 4.0 sent at B+260, the last fed sample, 40 ms old: the last step.
    steps.push_back(expected(300, InputVerdict::Usable, true, 4.0, {}));
    return steps;
}

constexpr int64_t kTailEndMs = 1300;

/// Every step of the fixture, then the silent tail through B + 1300 ms. Position 4.0 was
/// sent at B+260 with a limit of 120 ms: usable through B+350 (90 ms old), stale from
/// B+400. Mark 4 was sent at B+210 with a limit of 1000 ms: the marks port is usable
/// through B+1200 (990 ms old) and stale from B+1250.
std::vector<Seen> expectedStepsWithTail()
{
    std::vector<Seen> steps = expectedSteps();
    for (int64_t deadlineMs = 350; deadlineMs <= kTailEndMs; deadlineMs += 50) {
        const bool positionUsable = deadlineMs - 260 <= 120;
        const bool marksUsable = deadlineMs - 210 <= 1000;
        Seen       seen = expected(
            deadlineMs, positionUsable ? InputVerdict::Usable : InputVerdict::Stale, false, 4.0, {}
        );
        seen.stampSecs = static_cast<uint32_t>(100 + deadlineMs / 1000);
        seen.stampNsecs = static_cast<uint32_t>((deadlineMs % 1000) * kMilli);
        seen.marksVerdict = marksUsable ? InputVerdict::Usable : InputVerdict::Stale;
        steps.push_back(seen);
    }
    return steps;
}

/// A parameter file in the temporary directory, removed with the object.
class ParameterFile
{
  public:
    ParameterFile(const std::string& name, const std::string& text)
        : path_(std::filesystem::temp_directory_path() / name)
    {
        std::ofstream output(this->path_);
        output << text;
    }

    ParameterFile(const ParameterFile&) = delete;
    ParameterFile& operator=(const ParameterFile&) = delete;

    ~ParameterFile()
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

const std::string kTrackerParameters = "[ports.inputs.position]\n"
                                       "topic = '/position'\n"
                                       "silence_ms = 120\n"
                                       "[ports.inputs.marks]\n"
                                       "topic = '/marks'\n"
                                       "silence_ms = 1000\n"
                                       "[ports.inputs.spare]\n"
                                       "[ports.outputs.estimate]\n"
                                       "topic = '/estimate'\n";

std::string singleParameters(const std::string& topic)
{
    return "[ports.inputs.input]\ntopic = '" + topic + "'\nsilence_ms = 120\n";
}

/// Replays @p file into a new Tracker and returns what each step saw and sent.
struct Replayed
{
    std::vector<Seen>                 steps;
    std::vector<std::vector<Float64>> sent;
};

Replayed replayTracker(const std::string& file, std::optional<Time> end = std::nullopt)
{
    Replayed    result;
    Tracker     tracker;
    StepHarness harness{tracker};
    McapReplay  replay{file, kRate};
    if (end.has_value()) {
        replay.endAt(*end);
    }
    replay.feed(tracker.position);
    replay.feed(tracker.marks);
    replay.feed(tracker.spare);
    while (replay.stepDue()) {
        const hypha::CycleTime time = replay.next(harness);
        harness.step(tracker, time);
        result.sent.push_back(harness.sent(tracker.estimate));
    }
    result.steps = tracker.steps;
    return result;
}

/// The text of the exception that @p call throws, of type @p E; empty when it throws none.
template <typename E, typename Call>
std::string thrownText(Call&& call)
{
    try {
        call();
    } catch (const E& error) {
        return error.what();
    }
    return std::string{};
}

/// A Node over a parameter file, for the tests whose ports read their topic from a table.
class McapReplayTest : public ::testing::Test
{
  protected:
    void useParameters(const std::string& text)
    {
        this->node.reset();
        this->file = std::make_unique<ParameterFile>("hypha_mcap_replay.toml", text);
        this->node = std::make_unique<hypha::Node>("mcap_replay_test", this->file->path());
    }

    void TearDown() override
    {
        this->node.reset();
        this->file.reset();
    }

    std::unique_ptr<ParameterFile> file;
    std::unique_ptr<hypha::Node>   node;
};

}  // namespace

TEST_F(McapReplayTest, EveryStepShowsTheValuesAndVerdictsTheFixtureTimesGive)
{
    this->useParameters(kTrackerParameters);
    const Replayed          replayed = replayTracker(kFixture);
    const std::vector<Seen> wanted = expectedSteps();
    ASSERT_EQ(replayed.steps.size(), wanted.size());
    for (size_t index = 0; index < wanted.size(); ++index) {
        EXPECT_EQ(replayed.steps[index], wanted[index]) << "step " << index;
    }
}

TEST_F(McapReplayTest, TheGoldenTopicIsReadInLogTimeOrderWithBothTimes)
{
    this->useParameters(kTrackerParameters);
    const McapReplay                     replay{kFixture, kRate};
    const std::vector<Recorded<Float64>> golden = replay.read<Float64>("/golden");
    const std::vector<int64_t>           times{50, 100, 150, 200, 300};
    const std::vector<double>            values{10.0, 20.0, 30.0, 30.0, 40.0};
    ASSERT_EQ(golden.size(), times.size());
    for (size_t index = 0; index < golden.size(); ++index) {
        const int64_t logged = kBase + times[index] * kMilli;
        EXPECT_EQ(golden[index].logTime.nanos(), logged);
        EXPECT_EQ(golden[index].publishTime.nanos(), logged - 3 * kMilli);
        EXPECT_EQ(golden[index].message.data, values[index]);
    }
}

TEST_F(McapReplayTest, AGoldenTestComparesWhatEachStepSentWithTheGoldenTopic)
{
    this->useParameters(kTrackerParameters);
    const Replayed                       replayed = replayTracker(kFixture);
    const McapReplay                     replay{kFixture, kRate};
    const std::vector<Recorded<Float64>> golden = replay.read<Float64>("/golden");
    size_t                               next = 0;
    for (size_t index = 0; index < replayed.steps.size(); ++index) {
        const int64_t               now = replayed.steps[index].now;
        const std::vector<Float64>& sent = replayed.sent[index];
        const bool goldenAtThisStep = next < golden.size() && golden[next].logTime.nanos() == now;
        if (!goldenAtThisStep) {
            EXPECT_TRUE(sent.empty()) << "step " << index;
            continue;
        }
        ASSERT_EQ(sent.size(), 1U) << "step " << index;
        EXPECT_EQ(sent[0].data, golden[next].message.data) << "step " << index;
        ++next;
    }
    EXPECT_EQ(next, golden.size());
}

TEST_F(McapReplayTest, ReadingDuringTheReplayDoesNotMoveIt)
{
    this->useParameters(kTrackerParameters);
    Tracker     tracker;
    StepHarness harness{tracker};
    McapReplay  replay{kFixture, kRate};
    replay.feed(tracker.position);
    replay.feed(tracker.marks);
    ASSERT_TRUE(replay.stepDue());
    harness.step(tracker, replay.next(harness));
    EXPECT_EQ(replay.read<Float64>("/position").size(), 4U);
    EXPECT_EQ(replay.read<UInt32>("/marks").size(), 4U);
    while (replay.stepDue()) {
        harness.step(tracker, replay.next(harness));
    }
    const std::vector<Seen> wanted = expectedSteps();
    ASSERT_EQ(tracker.steps.size(), wanted.size());
    for (size_t index = 0; index < wanted.size(); ++index) {
        EXPECT_EQ(tracker.steps[index], wanted[index]) << "step " << index;
    }
}

TEST_F(McapReplayTest, TheCompressedCopyReplaysTheSame)
{
    this->useParameters(kTrackerParameters);
    const Replayed plain = replayTracker(kFixture);
    const Replayed zstd = replayTracker(kZstdFixture);
    ASSERT_EQ(zstd.steps.size(), plain.steps.size());
    for (size_t index = 0; index < plain.steps.size(); ++index) {
        EXPECT_EQ(zstd.steps[index], plain.steps[index]) << "step " << index;
    }
    const McapReplay                     plainFile{kFixture, kRate};
    const McapReplay                     zstdFile{kZstdFixture, kRate};
    const std::vector<Recorded<Float64>> plainGolden = plainFile.read<Float64>("/golden");
    const std::vector<Recorded<Float64>> zstdGolden = zstdFile.read<Float64>("/golden");
    ASSERT_EQ(zstdGolden.size(), plainGolden.size());
    for (size_t index = 0; index < plainGolden.size(); ++index) {
        EXPECT_EQ(zstdGolden[index].logTime, plainGolden[index].logTime);
        EXPECT_EQ(zstdGolden[index].message.data, plainGolden[index].message.data);
    }
}

TEST_F(McapReplayTest, ADisabledOptionalPortIsSkippedAndNeverFed)
{
    this->useParameters(kTrackerParameters);
    const Replayed replayed = replayTracker(kFixture);
    ASSERT_EQ(replayed.steps.size(), expectedSteps().size());
    Tracker     tracker;
    StepHarness harness{tracker};
    McapReplay  replay{kFixture, kRate};
    replay.feed(tracker.spare);
    replay.feed(tracker.position);
    while (replay.stepDue()) {
        harness.step(tracker, replay.next(harness));
    }
    EXPECT_FALSE(tracker.spare.has());
    EXPECT_EQ(tracker.spare.verdict(), InputVerdict::NeverArrived);
}

TEST_F(McapReplayTest, ALiteralTopicNeedsOnlyAConfigContextForItsLimit)
{
    const ParameterFile literalFile(
        "hypha_mcap_replay_literal.toml", "[ports.inputs.position]\nsilence_ms = 120\n"
    );
    const hypha::ConfigContext context(literalFile.path());

    /// The position input on a topic that code fixes, with its limit in its table.
    class Literal final : public hypha::CycleNode
    {
      public:
        Latest<Float64>           position{*this, "position", hypha::Topic{"/position"}};
        std::vector<InputVerdict> verdicts;

        void step(const hypha::CycleTime&) override
        {
            this->verdicts.push_back(this->position.verdict());
        }
    };

    Literal     literal;
    StepHarness harness{literal};
    McapReplay  replay{kFixture, kRate};
    replay.feed(literal.position);
    while (replay.stepDue()) {
        harness.step(literal, replay.next(harness));
    }
    const std::vector<InputVerdict> wanted{
        InputVerdict::Usable,
        InputVerdict::Usable,
        InputVerdict::Usable,
        InputVerdict::Usable,
        InputVerdict::Stale,
        InputVerdict::Usable
    };
    EXPECT_EQ(literal.verdicts, wanted);
}

TEST(McapReplayRefusal, AFileThatDoesNotOpenIsRefusedWithTheReadersText)
{
    const std::string missing = std::string(HYPHA_TEST_DATA_DIR) + "/no_such_fixture.mcap";
    const std::string text = thrownText<std::runtime_error>([&missing]() {
        const McapReplay replay{missing, kRate};
    });
    EXPECT_NE(text.find("cannot read it"), std::string::npos) << text;
    EXPECT_NE(text.find(missing), std::string::npos) << text;
}

TEST(McapReplayRefusal, ARateWithoutAPeriodIsRefused)
{
    const std::string text = thrownText<std::invalid_argument>([]() {
        const McapReplay replay{kFixture, hypha::Hz(0.0)};
    });
    EXPECT_NE(text.find(kFixture), std::string::npos) << text;
}

TEST(McapReplayRefusal, ATablePortWithoutANodeIsRefused)
{
    const ParameterFile file("hypha_mcap_replay_no_node.toml", singleParameters("/position"));
    const hypha::ConfigContext context(file.path());
    Single                     single;
    McapReplay                 replay{kFixture, kRate};
    const std::string          text = thrownText<std::logic_error>([&replay, &single]() {
        replay.feed(single.input);
    });
    EXPECT_NE(text.find("ports.inputs.input"), std::string::npos) << text;
    EXPECT_NE(text.find("needs a Node"), std::string::npos) << text;
}

TEST_F(McapReplayTest, ATopicTheFileDoesNotHoldIsRefused)
{
    this->useParameters(singleParameters("/absent"));
    Single            single;
    McapReplay        replay{kFixture, kRate};
    const std::string text = thrownText<std::runtime_error>([&replay, &single]() {
        replay.feed(single.input);
    });
    EXPECT_NE(text.find("no topic '/absent'"), std::string::npos) << text;
    EXPECT_NE(text.find(kFixture), std::string::npos) << text;
}

TEST_F(McapReplayTest, AChannelThatIsNotRos1IsRefused)
{
    this->useParameters(singleParameters("/json"));
    Single            single;
    McapReplay        replay{kFixture, kRate};
    const std::string text = thrownText<std::runtime_error>([&replay, &single]() {
        replay.feed(single.input);
    });
    EXPECT_NE(text.find("'/json'"), std::string::npos) << text;
    EXPECT_NE(text.find("schema encoding is 'jsonschema'"), std::string::npos) << text;
    EXPECT_NE(text.find(kFixture), std::string::npos) << text;
}

TEST_F(McapReplayTest, ATopicRecordedAsAnotherTypeIsRefusedNamingBoth)
{
    this->useParameters(singleParameters("/marks"));
    Single            single;
    McapReplay        replay{kFixture, kRate};
    const std::string text = thrownText<std::runtime_error>([&replay, &single]() {
        replay.feed(single.input);
    });
    EXPECT_NE(text.find("'/marks'"), std::string::npos) << text;
    EXPECT_NE(text.find("recorded as std_msgs/UInt32"), std::string::npos) << text;
    EXPECT_NE(text.find("reads std_msgs/Float64"), std::string::npos) << text;
    const std::string read = thrownText<std::runtime_error>([&replay]() {
        static_cast<void>(replay.read<Float64>("/marks"));
    });
    EXPECT_NE(read.find("recorded as std_msgs/UInt32"), std::string::npos) << read;
}

TEST_F(McapReplayTest, ATopicRecordedWithAnotherMd5IsRefusedNamingBoth)
{
    this->useParameters(singleParameters("/bad_md5"));
    Single            single;
    McapReplay        replay{kFixture, kRate};
    const std::string text = thrownText<std::runtime_error>([&replay, &single]() {
        replay.feed(single.input);
    });
    EXPECT_NE(text.find("'/bad_md5'"), std::string::npos) << text;
    EXPECT_NE(text.find("MD5 00000000000000000000000000000000"), std::string::npos) << text;
    EXPECT_NE(text.find("MD5 fdb28210bfa9d7c91146260178d9a584"), std::string::npos) << text;
}

TEST_F(McapReplayTest, APayloadThatDoesNotDecodeIsRefusedWithItsTopicAndLogTime)
{
    this->useParameters(singleParameters("/truncated"));
    Single      single;
    StepHarness harness{single};
    McapReplay  replay{kFixture, kRate};
    replay.feed(single.input);
    ASSERT_TRUE(replay.stepDue());
    const std::string text = thrownText<std::runtime_error>([&replay, &harness]() {
        replay.next(harness);
    });
    EXPECT_NE(text.find("'/truncated'"), std::string::npos) << text;
    EXPECT_NE(text.find("logged at 100030000000 ns"), std::string::npos) << text;
    EXPECT_NE(text.find(kFixture), std::string::npos) << text;
    const std::string read = thrownText<std::runtime_error>([&replay]() {
        static_cast<void>(replay.read<Float64>("/truncated"));
    });
    EXPECT_NE(read.find("logged at 100030000000 ns"), std::string::npos) << read;
}

TEST_F(McapReplayTest, AFileWithNoMessageOnAnyFedTopicIsRefused)
{
    this->useParameters(singleParameters("/silent"));
    Single     single;
    McapReplay replay{kFixture, kRate};
    replay.feed(single.input);
    const std::string text = thrownText<std::runtime_error>([&replay]() {
        static_cast<void>(replay.stepDue());
    });
    EXPECT_NE(text.find("no message on any fed topic ('/silent')"), std::string::npos) << text;
    EXPECT_NE(text.find(kFixture), std::string::npos) << text;
}

TEST_F(McapReplayTest, FeedingAPortTwiceIsRefused)
{
    this->useParameters(kTrackerParameters);
    Tracker    tracker;
    McapReplay replay{kFixture, kRate};
    replay.feed(tracker.position);
    const std::string text = thrownText<std::logic_error>([&replay, &tracker]() {
        replay.feed(tracker.position);
    });
    EXPECT_NE(text.find("fed already"), std::string::npos) << text;
}

TEST_F(McapReplayTest, FeedingAfterTheReplayStartedIsRefused)
{
    this->useParameters(kTrackerParameters);
    Tracker     tracker;
    StepHarness harness{tracker};
    McapReplay  replay{kFixture, kRate};
    replay.feed(tracker.position);
    ASSERT_TRUE(replay.stepDue());
    harness.step(tracker, replay.next(harness));
    const std::string text = thrownText<std::logic_error>([&replay, &tracker]() {
        replay.feed(tracker.marks);
    });
    EXPECT_NE(text.find("after the replay started"), std::string::npos) << text;
}

TEST_F(McapReplayTest, NextWithNoStepDueIsRefused)
{
    this->useParameters(kTrackerParameters);
    Tracker     tracker;
    StepHarness harness{tracker};
    McapReplay  replay{kFixture, kRate};
    replay.feed(tracker.position);
    size_t steps = 0;
    while (replay.stepDue()) {
        harness.step(tracker, replay.next(harness));
        ++steps;
    }
    EXPECT_EQ(steps, 6U);
    const std::string text = thrownText<std::logic_error>([&replay, &harness]() {
        replay.next(harness);
    });
    EXPECT_NE(text.find("no step due"), std::string::npos) << text;
}

TEST_F(McapReplayTest, AnEndTimeStepsOnThroughTheSilentTailAndThePortsGoStale)
{
    this->useParameters(kTrackerParameters);
    const Time              end = Time::fromNanos(kBase + kTailEndMs * kMilli + 49 * kMilli);
    const Replayed          replayed = replayTracker(kFixture, end);
    const std::vector<Seen> wanted = expectedStepsWithTail();
    ASSERT_EQ(replayed.steps.size(), wanted.size());
    for (size_t index = 0; index < wanted.size(); ++index) {
        EXPECT_EQ(replayed.steps[index], wanted[index]) << "step " << index;
    }
    // The output follows the input: nothing is sent once the position is stale.
    EXPECT_EQ(replayed.sent[7].size(), 0U);
}

TEST_F(McapReplayTest, AnEndTimeBeforeTheLastFedSampleChangesNothing)
{
    this->useParameters(kTrackerParameters);
    const Replayed replayed = replayTracker(kFixture, Time::fromNanos(kBase + 100 * kMilli));
    EXPECT_EQ(replayed.steps.size(), expectedSteps().size());
}

TEST_F(McapReplayTest, NextAfterTheEndTimeIsRefused)
{
    this->useParameters(kTrackerParameters);
    Tracker     tracker;
    StepHarness harness{tracker};
    McapReplay  replay{kFixture, kRate};
    replay.feed(tracker.position);
    replay.endAt(Time::fromNanos(kBase + 400 * kMilli));
    size_t steps = 0;
    while (replay.stepDue()) {
        harness.step(tracker, replay.next(harness));
        ++steps;
    }
    EXPECT_EQ(steps, 8U);
    EXPECT_THROW(replay.next(harness), std::logic_error);
}

TEST_F(McapReplayTest, AnOptionalTopicTheFileDoesNotHoldIsLeftUnfed)
{
    this->useParameters(
        "[ports.inputs.position]\ntopic = '/position'\nsilence_ms = 120\n"
        "[ports.inputs.marks]\ntopic = '/absent_marks'\nsilence_ms = 1000\n"
        "[ports.inputs.spare]\ntopic = '/absent'\n"
        "[ports.outputs.estimate]\ntopic = '/estimate'\n"
    );
    Tracker     tracker;
    StepHarness harness{tracker};
    McapReplay  replay{kFixture, kRate};
    EXPECT_TRUE(replay.feedIfPresent(tracker.position));
    EXPECT_FALSE(replay.feedIfPresent(tracker.spare));
    EXPECT_FALSE(replay.feedIfPresent(tracker.marks));
    size_t steps = 0;
    while (replay.stepDue()) {
        harness.step(tracker, replay.next(harness));
        ++steps;
    }
    EXPECT_EQ(steps, 6U);
    EXPECT_FALSE(tracker.spare.has());
    EXPECT_EQ(tracker.spare.verdict(), InputVerdict::NeverArrived);
    EXPECT_EQ(tracker.marks.verdict(), InputVerdict::NeverArrived);
    // The port is known to the replay: a second feed is refused, as for a fed port.
    EXPECT_THROW(static_cast<void>(replay.feedIfPresent(tracker.spare)), std::logic_error);
}

TEST_F(McapReplayTest, FeedIfPresentSkipsADisabledPort)
{
    this->useParameters(kTrackerParameters);
    Tracker    tracker;
    McapReplay replay{kFixture, kRate};
    EXPECT_FALSE(replay.feedIfPresent(tracker.spare));
}

TEST_F(McapReplayTest, FeedIfPresentStillChecksAPresentChannel)
{
    this->useParameters(singleParameters("/marks"));
    Single            single;
    McapReplay        other{kFixture, kRate};
    const std::string text = thrownText<std::runtime_error>([&other, &single]() {
        static_cast<void>(other.feedIfPresent(single.input));
    });
    EXPECT_NE(text.find("recorded as std_msgs/UInt32"), std::string::npos) << text;
}

namespace {

/// A fed sample in log-time order, for the bus replay.
struct Sample
{
    int64_t  sentAt;
    bool     isPosition;
    double   position;
    uint32_t mark;
};

}  // namespace

/// Each sample is published with its log time as its framework send stamp (the clock is set
/// to that time first), and the clock then moves on to the next sample. The clock starts at
/// the first sample, as the player's first /clock does.
TEST_F(McapReplayTest, TheStepReplayMatchesTheStartedCycleOnTheBusUnderSimulatedTime)
{
    this->useParameters(kTrackerParameters);
    const McapReplay    files{kFixture, kRate};
    std::vector<Sample> samples;
    for (const Recorded<Float64>& item : files.read<Float64>("/position")) {
        samples.push_back(Sample{item.logTime.nanos(), true, item.message.data, 0});
    }
    for (const Recorded<UInt32>& item : files.read<UInt32>("/marks")) {
        samples.push_back(Sample{item.logTime.nanos(), false, 0.0, item.message.data});
    }
    std::stable_sort(samples.begin(), samples.end(), [](const Sample& left, const Sample& right) {
        return left.sentAt < right.sentAt;
    });
    // With the silent tail: the verdicts of both ports after their publishers stopped.
    const Time     tailEnd = Time::fromNanos(kBase + kTailEndMs * kMilli);
    const Replayed stepped = replayTracker(kFixture, tailEnd);
    const size_t   stepCount = expectedStepsWithTail().size();
    ASSERT_EQ(stepped.steps.size(), stepCount);

    this->node.reset();
    ASSERT_TRUE(hypha::enableSimTime(Time::fromNanos(samples.front().sentAt)));
    std::vector<Seen> real;
    {
        hypha::Node               busNode("mcap_replay_bus", this->file->path());
        hypha::Publisher<Float64> positions =
            hypha::Node().advertise<Float64>(hypha::Topic{"/position"});
        hypha::Publisher<UInt32> marks = hypha::Node().advertise<UInt32>(hypha::Topic{"/marks"});
        ASSERT_TRUE(positions.enabled());
        ASSERT_TRUE(marks.enabled());
        Tracker tracker;
        tracker.setRate(kRate);
        ASSERT_TRUE(tracker.start(busNode).has_value());
        for (const Sample& sample : samples) {
            ASSERT_TRUE(hypha::setSimTime(Time::fromNanos(sample.sentAt)));
            if (sample.isPosition) {
                ASSERT_TRUE(positions.publish(Float64{sample.position}));
            } else {
                ASSERT_TRUE(marks.publish(UInt32{sample.mark}));
            }
            ASSERT_TRUE(busNode.runFor(hypha::Duration::fromMillis(1)).ok());
        }
        ASSERT_TRUE(hypha::setSimTime(Time::fromNanos(stepped.steps.back().now)));
        const bool allSteps = hypha_test::runUntil(busNode, [&tracker, stepCount]() {
            return tracker.steps.size() >= stepCount;
        });
        ASSERT_TRUE(allSteps);
        real = tracker.steps;
    }
    ASSERT_TRUE(hypha::disableSimTime());
    ASSERT_EQ(real.size(), stepped.steps.size());
    for (size_t index = 0; index < real.size(); ++index) {
        EXPECT_EQ(stepped.steps[index], real[index]) << "step " << index;
    }
}
