#include "hypha/cycle_node.h"
#include "hypha/hypha.h"
#include "hypha/testing.h"

#include "introspection_text.h"
#include "test_types.h"

#include <gtest/gtest.h>
#include <ros/serialization.h>
#include <std_msgs/Float64.h>

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <deque>
#include <filesystem>
#include <fstream>
#include <functional>
#include <memory>
#include <mutex>
#include <optional>
#include <stdexcept>
#include <string>
#include <thread>
#include <type_traits>
#include <vector>

using Float64 = std_msgs::Float64;
using namespace hypha::literals;

namespace {

/// Cycles, cycle nodes and ports are neither copyable nor movable: a port holds its cycle.
struct ContractNode final : hypha::CycleNode
{
    void step(const hypha::CycleTime&) override
    {
    }
};

template <typename T>
constexpr bool PINNED = !std::is_copy_constructible_v<T> && !std::is_move_constructible_v<T>;

static_assert(PINNED<hypha::Cycle>);
static_assert(PINNED<ContractNode>);
static_assert(PINNED<hypha::Latest<Float64>>);
static_assert(PINNED<hypha::Arrivals<Float64>>);
static_assert(PINNED<hypha::Output<Float64>>);

/// A message whose decode is counted. The port decodes once per new held
/// sample, so the count shows the held rule.
struct CountedValue
{
    double value{0.0};
};

int countedDecodes = 0;

std::string topicFor(const std::string& name)
{
    return hypha_test::testTopic("/" + name);
}

/// Writes a parameter file with `in_0..in_19` and `out_0..out_6` for this test.
class CycleNodeFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->path = std::filesystem::temp_directory_path() / "hypha_cycle_node_test.toml";
        std::ofstream output(this->path);
        for (int index = 0; index < 20; ++index) {
            // One hour: these ports are not about staleness; `held` below is.
            output << "[ports.inputs.in_" << index << "]\ntopic='"
                   << topicFor("in_" + std::to_string(index)) << "'\nsilence_ms=3600000\n";
        }
        output << "[ports.inputs.off]\ntopic=''\n";
        // The held tests read in_0's topic through a port with a 200 ms silence limit.
        output << "[ports.inputs.held]\ntopic='" << topicFor("in_0") << "'\nsilence_ms=200\n";
        for (int index = 0; index < 7; ++index) {
            output << "[ports.outputs.out_" << index << "]\ntopic='"
                   << topicFor("out_" + std::to_string(index)) << "'\n";
        }
        output.close();
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("cycle_node_test", this->path.string());
    }

    void TearDown() override
    {
        this->node.reset();
        std::filesystem::remove(this->path);
    }

    std::optional<hypha::Node> node;
    std::filesystem::path      path;
};

/// Runs short slices of simulated time until @p done holds, or fails after a
/// generous real-time bound. Ages are real monotonic time, so a fixed sleep
/// would race the host; polling the observable event does not.
bool runUntil(hypha::Node& node, const std::function<bool()>& done, std::chrono::seconds bound)
{
    const auto deadline = std::chrono::steady_clock::now() + bound;
    while (!done()) {
        if (std::chrono::steady_clock::now() >= deadline) {
            return false;
        }
        hypha_test::advance(10_ms);
        if (!node.runFor(1_ms).ok()) {
            return false;
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(1));
    }
    return true;
}

struct Observed
{
    int                         steps{0};
    hypha::InputVerdict         verdict{hypha::InputVerdict::NeverArrived};
    bool                        usable{false};
    bool                        fresh{false};
    std::optional<double>       value;
    std::vector<const Float64*> valueAddresses;
};

class Held final : public hypha::CycleNode
{
  public:
    explicit Held(Observed& observed) : observed_(&observed)
    {
    }

    hypha::Latest<Float64> input{*this, "held"};

    void step(const hypha::CycleTime&) override
    {
        ++this->observed_->steps;
        this->observed_->verdict = this->input.verdict();
        this->observed_->usable = this->input.usable();
        this->observed_->fresh = this->input.fresh();
        if (this->input.has()) {
            this->observed_->value = this->input.lastKnown().data;
            this->observed_->valueAddresses.push_back(&this->input.lastKnown());
        }
    }

  private:
    Observed* observed_;
};

class Wide final : public hypha::CycleNode
{
  public:
    hypha::Latest<Float64> in0{*this, "in_0"};
    hypha::Latest<Float64> in1{*this, "in_1"};
    hypha::Latest<Float64> in2{*this, "in_2"};
    hypha::Latest<Float64> in3{*this, "in_3"};
    hypha::Latest<Float64> in4{*this, "in_4"};
    hypha::Latest<Float64> in5{*this, "in_5"};
    hypha::Latest<Float64> in6{*this, "in_6"};
    hypha::Latest<Float64> in7{*this, "in_7"};
    hypha::Latest<Float64> in8{*this, "in_8"};
    hypha::Latest<Float64> in9{*this, "in_9"};
    hypha::Latest<Float64> in10{*this, "in_10"};
    hypha::Latest<Float64> in11{*this, "in_11"};
    hypha::Latest<Float64> in12{*this, "in_12"};
    hypha::Latest<Float64> in13{*this, "in_13"};
    hypha::Latest<Float64> in14{*this, "in_14"};
    hypha::Latest<Float64> in15{*this, "in_15"};
    hypha::Latest<Float64> in16{*this, "in_16"};
    hypha::Latest<Float64> in17{*this, "in_17"};
    hypha::Latest<Float64> in18{*this, "in_18"};
    hypha::Latest<Float64> in19{*this, "in_19"};

    hypha::Output<Float64> out0{*this, "out_0"};
    hypha::Output<Float64> out1{*this, "out_1"};
    hypha::Output<Float64> out2{*this, "out_2"};
    hypha::Output<Float64> out3{*this, "out_3"};
    hypha::Output<Float64> out4{*this, "out_4"};
    hypha::Output<Float64> out5{*this, "out_5"};
    hypha::Output<Float64> out6{*this, "out_6"};

    void step(const hypha::CycleTime&) override
    {
    }
};

class MissingTopic final : public hypha::CycleNode
{
  public:
    hypha::Latest<Float64> absent{*this, "topic_absent"};

    void step(const hypha::CycleTime&) override
    {
    }
};

class OptionalAbsent final : public hypha::CycleNode
{
  public:
    Latest<Float64>     absent{*this, "off", optionalTopic};
    int                 steps{0};
    hypha::InputVerdict verdict{hypha::InputVerdict::Usable};
    bool                valueThrew{false};

    void step(const hypha::CycleTime&) override
    {
        ++this->steps;
        this->verdict = this->absent.verdict();
        try {
            static_cast<void>(*this->absent);
        } catch (const hypha::InputNotUsable&) {
            this->valueThrew = true;
        }
    }
};

/// One Latest port whose table comes from the test. The table may leave the topic out.
class OneInput final : public hypha_test::StepCycle
{
  public:
    OneInput(const std::string& table, bool optional)
        : hypha_test::StepCycle("one_input_" + table)
        , input(*this, table, hypha::optionalTopic, hypha::advisory(optional))
    {
    }

    hypha::Latest<Float64> input;

    void step(const hypha::CycleTime&) override
    {
    }
};

/// One Output port whose table comes from the test. The table may leave the topic out.
class OneOutput final : public hypha_test::StepCycle
{
  public:
    explicit OneOutput(const std::string& table)
        : hypha_test::StepCycle("one_output_" + table), output(*this, table, hypha::optionalTopic)
    {
    }

    hypha::Output<Float64> output;
    int                    steps{0};

    void step(const hypha::CycleTime&) override
    {
        ++this->steps;
        this->output.send(Float64{1.0});
    }
};

/// Parameter file with one entry per configuration state of a topic parameter.
class TopicStateFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->path = std::filesystem::temp_directory_path() / "hypha_topic_state_test.toml";
        std::ofstream output(this->path);
        output << "[ports.inputs.enabled]\ntopic='" << topicFor("enabled")
               << "'\nsilence_ms=3600000\n";
        output << "[ports.inputs.empty]\ntopic=''\n";
        output << "[ports.inputs.invalid]\ntopic='not a valid topic'\n";
        output << "[ports.outputs.empty]\ntopic=''\n[ports.outputs.live]\ntopic='"
               << topicFor("live") << "'\n";
        output.close();
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("topic_state_test", this->path.string());
    }

    void TearDown() override
    {
        this->node.reset();
        std::filesystem::remove(this->path);
    }

    std::optional<hypha::Node> node;

    std::filesystem::path path;
};

class CountedHeld final : public hypha::CycleNode
{
  public:
    hypha::Latest<CountedValue> input{*this, "held"};
    hypha::InputVerdict         verdict{hypha::InputVerdict::NeverArrived};

    void step(const hypha::CycleTime&) override
    {
        this->verdict = this->input.verdict();
    }
};

class Sender final : public hypha::CycleNode
{
  public:
    hypha::Output<Float64> out{*this, "out_0"};

    void step(const hypha::CycleTime&) override
    {
        this->out.send(Float64{1.0});
        this->out.send(Float64{1.0});
    }
};

struct Shared
{
    int fast{0};
    int slow{0};
};

struct FastStep
{
    Shared* state;

    void operator()(const hypha::CycleTime&) const
    {
        ++this->state->fast;
    }
};

struct SlowStep
{
    Shared* state;

    void operator()(const hypha::CycleTime&) const
    {
        ++this->state->slow;
    }
};

struct RelayStep
{
    std::deque<hypha::Latest<Float64>>* inputs;
    hypha::Output<Float64>*             sum;

    void operator()(const hypha::CycleTime&) const
    {
        double total = 0.0;
        for (const auto& input : *this->inputs) {
            if (input.usable()) {
                total += input->data;
            }
        }
        this->sum->send(Float64{total});
    }
};

int runNodeStepParameter = -1;

/// Requests the stop from inside the run, so no other thread is needed.
std::string runNodeSnapshot;

/// Each firing advances the test clock, so the periodic timer stays due. Only a
/// firing after 200 ms of host time requests the stop. The stop then lands in
/// run(), after runNode's one-millisecond executor turn.
struct StopRun
{
    std::chrono::steady_clock::time_point started;

    void operator()() const
    {
        runNodeSnapshot = test_support::introspectionText();
        const auto elapsed = std::chrono::steady_clock::now() - this->started;
        if (elapsed >= std::chrono::milliseconds(200)) {
            hypha::requestStop();
            return;
        }
        hypha_test::advance(hypha::Duration::fromMillis(2));
    }
};

class RunNodeApp final : public hypha::CycleNode
{
  public:
    void setup(hypha::Node& node) override
    {
        runNodeStepParameter = hypha::config().optional<int>("count", -1);
        this->setRate(hypha::Hz(100.0));
        this->stop_ =
            node.timer(hypha::Duration::fromMillis(1), StopRun{std::chrono::steady_clock::now()});
        hypha_test::advance(hypha::Duration::fromMillis(2));
    }

    void step(const hypha::CycleTime&) override
    {
    }

  private:
    hypha::Timer stop_;
};

}  // namespace

namespace ros::message_traits {

template <>
struct DataType<CountedValue>
{
    static const char* value()
    {
        return "hypha_test/CountedValue";
    }

    static const char* value(const CountedValue&)
    {
        return value();
    }
};

template <>
struct MD5Sum<CountedValue>
{
    static const char* value()
    {
        return "1b1594d2b74931ef8fe7be8e2d594455";
    }

    static const char* value(const CountedValue&)
    {
        return value();
    }
};

template <>
struct Definition<CountedValue>
{
    static const char* value()
    {
        return "float64 value\n";
    }

    static const char* value(const CountedValue&)
    {
        return value();
    }
};

}  // namespace ros::message_traits

namespace ros::serialization {

template <>
struct Serializer<CountedValue>
{
    template <typename Stream>
    static void write(Stream& stream, const CountedValue& message)
    {
        stream.next(message.value);
    }

    template <typename Stream>
    static void read(Stream& stream, CountedValue& message)
    {
        ++countedDecodes;
        stream.next(message.value);
    }

    static uint32_t serializedLength(const CountedValue&)
    {
        return sizeof(double);
    }
};

}  // namespace ros::serialization

TEST_F(CycleNodeFixture, ClassWithTwentyInputsAndSevenOutputsStartsInDeclarationOrder)
{
    hypha::Node& nodeRef = *this->node;
    Wide         wide;
    wide.setRate(hypha::Hz(100.0));
    const auto started = wide.start(nodeRef);
    ASSERT_TRUE(started.has_value()) << started.error();

    EXPECT_TRUE(wide.health().has_value());

    const std::string table = test_support::introspectionText();
    size_t            previous = 0;
    for (int index = 0; index < 20; ++index) {
        const size_t position = table.find(topicFor("in_" + std::to_string(index)));
        ASSERT_NE(position, std::string::npos) << "in_" << index;
        EXPECT_GT(position, previous) << "in_" << index;
        previous = position;
    }
    previous = 0;
    for (int index = 0; index < 7; ++index) {
        const size_t position = table.find(topicFor("out_" + std::to_string(index)));
        ASSERT_NE(position, std::string::npos) << "out_" << index;
        EXPECT_GT(position, previous) << "out_" << index;
        previous = position;
    }
}

/// An L3 node with 18 inputs (16 Latest, 2 Arrivals) and 3 outputs, for the integration test.
class EighteenInputs final : public hypha::CycleNode
{
  public:
    hypha::Latest<Float64>   in0{*this, "in_0"};
    hypha::Latest<Float64>   in1{*this, "in_1"};
    hypha::Latest<Float64>   in2{*this, "in_2"};
    hypha::Latest<Float64>   in3{*this, "in_3"};
    hypha::Latest<Float64>   in4{*this, "in_4"};
    hypha::Latest<Float64>   in5{*this, "in_5"};
    hypha::Latest<Float64>   in6{*this, "in_6"};
    hypha::Latest<Float64>   in7{*this, "in_7"};
    hypha::Latest<Float64>   in8{*this, "in_8"};
    hypha::Latest<Float64>   in9{*this, "in_9"};
    hypha::Latest<Float64>   in10{*this, "in_10"};
    hypha::Latest<Float64>   in11{*this, "in_11"};
    hypha::Latest<Float64>   in12{*this, "in_12"};
    hypha::Latest<Float64>   in13{*this, "in_13"};
    hypha::Latest<Float64>   in14{*this, "in_14"};
    hypha::Latest<Float64>   in15{*this, "in_15"};
    hypha::Arrivals<Float64> in16{*this, "in_16"};
    hypha::Arrivals<Float64> in17{*this, "in_17"};

    hypha::Output<Float64> sum{*this, "out_0"};
    hypha::Output<Float64> count{*this, "out_1"};
    hypha::Output<Float64> arrivals{*this, "out_2"};

    void setup(hypha::Node&) override
    {
        this->setRate(hypha::Hz(100.0));
    }

    void step(const hypha::CycleTime&) override
    {
        double   total = 0.0;
        uint64_t usable = 0;
        this->addUsable(this->in0, total, usable);
        this->addUsable(this->in1, total, usable);
        this->addUsable(this->in2, total, usable);
        this->addUsable(this->in3, total, usable);
        this->addUsable(this->in4, total, usable);
        this->addUsable(this->in5, total, usable);
        this->addUsable(this->in6, total, usable);
        this->addUsable(this->in7, total, usable);
        this->addUsable(this->in8, total, usable);
        this->addUsable(this->in9, total, usable);
        this->addUsable(this->in10, total, usable);
        this->addUsable(this->in11, total, usable);
        this->addUsable(this->in12, total, usable);
        this->addUsable(this->in13, total, usable);
        this->addUsable(this->in14, total, usable);
        this->addUsable(this->in15, total, usable);
        this->arrivalsSeen += this->in16.samples().size() + this->in17.samples().size();
        this->sum.send({.data = total});
        this->count.send({.data = static_cast<double>(usable)});
        this->arrivals.send({.data = static_cast<double>(this->arrivalsSeen)});
    }

  private:
    void addUsable(const hypha::Latest<Float64>& port, double& total, uint64_t& usable) const
    {
        if (port.usable()) {
            total += port->data;
            ++usable;
        }
    }

    uint64_t arrivalsSeen{0};
};

TEST_F(CycleNodeFixture, AnL3NodeWithEighteenInputsPublishesItsOutputs)
{
    hypha::Node& nodeRef = *this->node;
    auto         sum = hypha::Node().subscribe<Float64>(hypha::Topic{topicFor("out_0")});
    auto         count = hypha::Node().subscribe<Float64>(hypha::Topic{topicFor("out_1")});
    auto         arrivals = hypha::Node().subscribe<Float64>(hypha::Topic{topicFor("out_2")});
    ASSERT_TRUE(sum.enabled() && count.enabled() && arrivals.enabled());

    std::vector<hypha::Publisher<Float64>> publishers;
    for (int index = 0; index < 18; ++index) {
        publishers.push_back(
            hypha::Node().advertise<Float64>(hypha::Topic{topicFor("in_" + std::to_string(index))})
        );
        ASSERT_TRUE(publishers.back().enabled());
    }

    EighteenInputs integration;
    integration.setup(nodeRef);
    const auto started = integration.start(nodeRef);
    ASSERT_TRUE(started.has_value()) << started.error();

    for (auto& publisher : publishers) {
        ASSERT_TRUE(publisher.publish(Float64{1.0}));
    }
    // Event-driven: wait until the outputs show all 18 inputs, not a fixed time.
    ASSERT_TRUE(runUntil(
        nodeRef,
        [&sum, &count, &arrivals]() {
            const auto total = sum.latestSample();
            const auto usable = count.latestSample();
            const auto seen = arrivals.latestSample();
            return total.has_value() && usable.has_value() && seen.has_value()
                && total->message->data == 16.0 && usable->message->data == 16.0
                && seen->message->data == 2.0;
        },
        std::chrono::seconds(10)
    ));

    // Keep the node running for 2 s of simulated time. The outputs stay correct.
    for (int tick = 0; tick < 200; ++tick) {
        hypha_test::advance(10_ms);
        ASSERT_TRUE(nodeRef.runFor(1_ms).ok());
    }
    const auto total = sum.latestSample();
    ASSERT_TRUE(total.has_value());
    EXPECT_DOUBLE_EQ(total->message->data, 16.0);
    const auto usable = count.latestSample();
    ASSERT_TRUE(usable.has_value());
    EXPECT_DOUBLE_EQ(usable->message->data, 16.0);
}

TEST(NodeParameterFile, ASecondParameterFileIsRefusedWhileTheNodeLives)
{
    const std::filesystem::path dir = std::filesystem::temp_directory_path();
    const std::filesystem::path first = dir / "hypha_init_first.toml";
    const std::filesystem::path second = dir / "hypha_init_second.toml";
    {
        std::ofstream(first) << "count=1\n";
    }
    {
        std::ofstream(second) << "count=2\n";
    }
    {
        hypha::Node node("init_twice", first.string());
        try {
            hypha::Node refused("init_twice", second.string());
            ADD_FAILURE() << "a second parameter file was accepted";
        } catch (const std::runtime_error& error) {
            EXPECT_NE(std::string(error.what()).find("InvalidState"), std::string::npos)
                << error.what();
        }
        EXPECT_NO_THROW(hypha::Node sameFile("init_twice", first.string()));
    }
    std::filesystem::remove(first);
    std::filesystem::remove(second);
}

TEST_F(CycleNodeFixture, MissingTopicParameterNamesThePortInTheError)
{
    hypha::Node& nodeRef = *this->node;
    MissingTopic missing;
    missing.setRate(hypha::Hz(100.0));
    const auto started = missing.start(nodeRef);
    ASSERT_FALSE(started.has_value());
    EXPECT_NE(started.error().find("cycle 'cycle_node_test'"), std::string::npos)
        << started.error();
    EXPECT_NE(started.error().find("input 'topic_absent'"), std::string::npos) << started.error();
}

TEST_F(CycleNodeFixture, ADisabledPortStartsAndNeverHasAValue)
{
    hypha::Node&   nodeRef = *this->node;
    OptionalAbsent port;
    port.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(port.start(nodeRef).has_value());
    ASSERT_TRUE(runUntil(
        nodeRef,
        [&port]() {
            return port.steps >= 3;
        },
        std::chrono::seconds(10)
    ));
    EXPECT_NE(port.verdict, hypha::InputVerdict::Usable);
    EXPECT_TRUE(port.valueThrew);
    EXPECT_FALSE(port.absent.has());
}

TEST_F(TopicStateFixture, ADisabledOutputPortStartsAndDropsWhatItIsGiven)
{
    hypha::Node& nodeRef = *this->node;
    OneOutput    port("empty");
    port.setRate(hypha::Hz(100.0));
    const auto started = port.start(nodeRef);
    ASSERT_TRUE(started.has_value()) << started.error();
    ASSERT_TRUE(runUntil(
        nodeRef,
        [&port]() {
            return port.steps >= 3;
        },
        std::chrono::seconds(10)
    ));
    EXPECT_GE(port.output.sentCount(), 3U);
}

TEST_F(TopicStateFixture, EnabledTellsADisabledInputFromAnEnabledOne)
{
    hypha::Node& nodeRef = *this->node;
    OneInput     live("enabled", false);
    OneInput     off("empty", false);
    EXPECT_FALSE(live.input.enabled()) << "a port that was not started has no source";
    live.setRate(hypha::Hz(100.0));
    off.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(live.start(nodeRef).has_value());
    ASSERT_TRUE(off.start(nodeRef).has_value());
    EXPECT_TRUE(live.input.enabled());
    EXPECT_FALSE(off.input.enabled());
}

TEST_F(TopicStateFixture, EnabledTellsADisabledOutputFromAnEnabledOne)
{
    hypha::Node& nodeRef = *this->node;
    OneOutput    live("live");
    OneOutput    off("empty");
    live.setRate(hypha::Hz(100.0));
    off.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(live.start(nodeRef).has_value());
    ASSERT_TRUE(off.start(nodeRef).has_value());
    EXPECT_TRUE(live.output.enabled());
    EXPECT_FALSE(off.output.enabled());
}

TEST_F(CycleNodeFixture, AStaleValueStaysReadableThroughLastKnownAndIsNotUsable)
{
    hypha::Node&      nodeRef = *this->node;
    const std::string topic = topicFor("in_0");
    auto              publisher = hypha::Node().advertise<Float64>(hypha::Topic{topic});
    ASSERT_TRUE(publisher.enabled());

    Observed observed;
    Held     held(observed);
    held.setRate(hypha::Hz(100.0));
    EXPECT_FALSE(held.input.has());
    EXPECT_THROW(static_cast<void>(*held.input), hypha::InputNotUsable);
    EXPECT_THROW(static_cast<void>(held.input.lastKnown()), std::logic_error);
    const Float64 fallback{-1.0};
    EXPECT_EQ(held.input.valueOr(fallback).data, -1.0);
    ASSERT_TRUE(held.start(nodeRef).has_value());

    ASSERT_TRUE(publisher.publish(Float64{1.0}));
    ASSERT_TRUE(runUntil(
        nodeRef,
        [&observed]() {
            return observed.verdict == hypha::InputVerdict::Usable;
        },
        std::chrono::seconds(10)
    ));
    EXPECT_TRUE(observed.usable);
    ASSERT_TRUE(observed.value.has_value());
    EXPECT_DOUBLE_EQ(*observed.value, 1.0);

    ASSERT_TRUE(runUntil(
        nodeRef,
        [&observed]() {
            return observed.verdict == hypha::InputVerdict::Stale;
        },
        std::chrono::seconds(10)
    ));
    // The value outlives its limit: lastKnown() reads it, *port refuses it, and it is
    // neither usable nor fresh.
    EXPECT_FALSE(observed.usable);
    EXPECT_FALSE(observed.fresh);
    EXPECT_TRUE(held.input.has());
    EXPECT_DOUBLE_EQ(held.input.valueOr(fallback).data, -1.0) << "valueOr needs a usable value";
    EXPECT_DOUBLE_EQ(*observed.value, 1.0);
    EXPECT_THROW(static_cast<void>(*held.input), hypha::InputNotUsable);

    for (int step = 0; step < 99; ++step) {
        hypha_test::advance(100_ms);
        ASSERT_TRUE(nodeRef.runFor(1_ms).ok());
    }
    ASSERT_GE(observed.valueAddresses.size(), 100U);
    for (const Float64* address : observed.valueAddresses) {
        EXPECT_EQ(address, &held.input.lastKnown());
    }
}

TEST_F(CycleNodeFixture, UnchangedHeldValueIsDecodedOnce)
{
    hypha::Node&      nodeRef = *this->node;
    const std::string topic = topicFor("in_0");
    auto              publisher = hypha::Node().advertise<CountedValue>(hypha::Topic{topic});
    ASSERT_TRUE(publisher.enabled());

    CountedHeld counted;
    counted.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(counted.start(nodeRef).has_value());

    countedDecodes = 0;
    ASSERT_TRUE(publisher.publish(CountedValue{2.0}));
    ASSERT_TRUE(runUntil(
        nodeRef,
        [&counted]() {
            return counted.verdict == hypha::InputVerdict::Usable;
        },
        std::chrono::seconds(10)
    ));
    EXPECT_GT(countedDecodes, 0);

    ASSERT_TRUE(runUntil(
        nodeRef,
        [&counted]() {
            return counted.verdict == hypha::InputVerdict::Stale;
        },
        std::chrono::seconds(10)
    ));
    const int decodesAfterStale = countedDecodes;

    for (int step = 0; step < 10; ++step) {
        hypha_test::advance(100_ms);
        ASSERT_TRUE(nodeRef.runFor(1_ms).ok());
    }
    EXPECT_EQ(countedDecodes, decodesAfterStale);
}

TEST_F(CycleNodeFixture, DefaultSendLimitFailsWithResourceLimitAndNoOutputReachesTheBus)
{
    hypha::Node& nodeRef = *this->node;
    auto         outSub = hypha::Node().subscribe<Float64>(hypha::Topic{topicFor("out_0")});
    ASSERT_TRUE(outSub.enabled());

    Sender sender;
    sender.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(sender.start(nodeRef).has_value());

    // The first tick runs the double send. Poll until the run reports it.
    hypha::RunResult run = nodeRef.runFor(1_ms);
    for (int pass = 0; pass < 2000 && run.ok(); ++pass) {
        hypha_test::advance(10_ms);
        run = nodeRef.runFor(1_ms);
    }
    EXPECT_FALSE(run.ok());
    EXPECT_EQ(run.status(), hypha::Status::ResourceLimit);
    EXPECT_FALSE(outSub.latestSample().has_value());
}

TEST_F(CycleNodeFixture, LateAndStagedPortsAreRefusedAfterStart)
{
    hypha::Node& nodeRef = *this->node;
    Observed     observed;
    Held         held(observed);
    held.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(held.start(nodeRef).has_value());

    EXPECT_THROW(
        {
            hypha::Latest<Float64> late(held, "in_1");
            (void)late;
        },
        std::logic_error
    );
    EXPECT_THROW(hypha::testing::StepHarness{held}, std::logic_error) << "a started cycle";
}

TEST_F(CycleNodeFixture, StartingACycleThatAHarnessHoldsIsRefused)
{
    Observed                    observed;
    Held                        held(observed);
    hypha::testing::StepHarness harness{held};
    held.setRate(hypha::Hz(100.0));
    EXPECT_THROW(static_cast<void>(held.start(*this->node)), std::logic_error);
}

TEST_F(CycleNodeFixture, SendAfterStartOutsideAStepThrows)
{
    Sender sender;
    sender.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(sender.start(*this->node).has_value());
    try {
        sender.out.send(Float64{1.0});
        FAIL() << "send() outside a step must throw";
    } catch (const std::logic_error& error) {
        EXPECT_STREQ(error.what(), "hypha: output 'out_0': send() outside a step");
    }
}

TEST_F(CycleNodeFixture, DestroyingARunningCycleNodeDetachesItFirst)
{
    hypha::Node& nodeRef = *this->node;
    Observed     observed;
    {
        auto held = std::make_unique<Held>(observed);
        held->setRate(hypha::Hz(100.0));
        ASSERT_TRUE(held->start(nodeRef).has_value());
        // Each 10 ms advance crosses exactly one deadline of the 100 Hz cycle. The
        // next advance waits until that step ran, so a slow host cannot make one
        // advance cross two deadlines.
        hypha_test::advance(10_ms);
        ASSERT_TRUE(hypha_test::runUntil(nodeRef, [&] {
            return observed.steps >= 1;
        }));
        ASSERT_EQ(observed.steps, 1);
        held.reset();
    }
    const int stepsAtRelease = observed.steps;
    hypha_test::advance(100_ms);
    EXPECT_TRUE(nodeRef.runFor(1_ms).ok());
    EXPECT_EQ(observed.steps, stepsAtRelease);
}

TEST_F(CycleNodeFixture, LevelTwoCycleRunsPortsBuiltFromAParameterList)
{
    hypha::Node& nodeRef = *this->node;
    auto         outSub = hypha::Node().subscribe<Float64>(hypha::Topic{topicFor("out_0")});
    ASSERT_TRUE(outSub.enabled());

    hypha::Cycle                       cycle("deque_cycle");
    std::deque<hypha::Latest<Float64>> inputs;
    hypha::Output<Float64>             sum{cycle, "out_0"};
    cycle.setRate(hypha::Hz(100.0));
    for (int index = 0; index < 3; ++index) {
        inputs.emplace_back(cycle, "in_" + std::to_string(index));
    }
    ASSERT_TRUE(cycle.start(nodeRef, RelayStep{&inputs, &sum}).has_value());

    auto first = hypha::Node().advertise<Float64>(hypha::Topic{topicFor("in_0")});
    auto second = hypha::Node().advertise<Float64>(hypha::Topic{topicFor("in_1")});
    auto third = hypha::Node().advertise<Float64>(hypha::Topic{topicFor("in_2")});
    ASSERT_TRUE(first.enabled());
    ASSERT_TRUE(second.enabled());
    ASSERT_TRUE(third.enabled());
    ASSERT_TRUE(first.publish(Float64{1.0}));
    ASSERT_TRUE(second.publish(Float64{2.0}));
    ASSERT_TRUE(third.publish(Float64{3.0}));
    ASSERT_TRUE(runUntil(
        nodeRef,
        [&outSub]() {
            const auto latest = outSub.latestSample();
            return latest.has_value() && latest->message->data == 6.0;
        },
        std::chrono::seconds(10)
    ));
}

TEST_F(CycleNodeFixture, TwoLevelTwoCyclesAtDifferentRatesShareOneState)
{
    hypha::Node& nodeRef = *this->node;
    Shared       shared;
    hypha::Cycle fast("shared_fast");
    hypha::Cycle slow("shared_slow");
    fast.setRate(hypha::Hz(100.0));
    slow.setRate(hypha::Hz(10.0));
    ASSERT_TRUE(fast.start(nodeRef, FastStep{&shared}).has_value());
    ASSERT_TRUE(slow.start(nodeRef, SlowStep{&shared}).has_value());

    for (int tick = 0; tick < 100; ++tick) {
        hypha_test::advance(10_ms);
        ASSERT_TRUE(nodeRef.runFor(1_ms).ok());
    }
    EXPECT_GT(shared.slow, 0);
    EXPECT_GT(shared.fast, shared.slow);
}

struct NoStep
{
    void operator()(const hypha::CycleTime&) const
    {
    }
};

TEST_F(CycleNodeFixture, APerStepOfZeroIsRefusedAtConstruction)
{
    hypha::Cycle cycle("zero_depth");
    EXPECT_THROW(hypha::Arrivals<Float64>(cycle, "in_0", hypha::perStep(0)), std::invalid_argument);
    EXPECT_THROW(hypha::Output<Float64>(cycle, "out_0", hypha::perStep(0)), std::invalid_argument);
}

TEST_F(CycleNodeFixture, ALatestInputTakesTheHostClock)
{
    hypha::Node&      nodeRef = *this->node;
    const std::string topic = topicFor("host_clock");

    hypha::Cycle           latestCycle("host_clock_latest");
    hypha::Latest<Float64> latest{
        latestCycle, hypha::Topic{topic}, hypha::hostClock, hypha::unsupervised
    };
    latestCycle.setRate(hypha::Hz(100.0));
    const auto started = latestCycle.start(nodeRef, NoStep{});
    ASSERT_TRUE(started.has_value()) << started.error();
}

TEST_F(CycleNodeFixture, SharedSourceAcceptsArrivalsPerStepBound)
{
    hypha::Node&      nodeRef = *this->node;
    const std::string topic = topicFor("shared_arrivals_depth");

    hypha::Cycle             cycle("shared_source_depth");
    hypha::Arrivals<Float64> arrivals{
        cycle, hypha::Topic{topic}, hypha::perStep(21), hypha::unsupervised
    };
    cycle.setRate(hypha::Hz(100.0));
    const auto started = cycle.start(nodeRef, NoStep{});
    ASSERT_TRUE(started.has_value()) << started.error();
}

TEST(CycleStamp, AnInvalidStampFailsTheIterationBeforeTheStep)
{
    struct Counting
    {
        int* calls;

        void operator()(const hypha::CycleTime&) const
        {
            ++*this->calls;
        }
    };

    int                                  calls = 0;
    auto                                 link = std::make_shared<hypha::detail::CycleLink>();
    hypha::detail::CycleRunner<Counting> runner(Counting{&calls}, link);
    HyphaCycleTime                       raw{};
    raw.message_stamp_valid = 0;
    raw.message_stamp_nanos = -1;

    const HyphaCallbackResult result = runner.invoke(nullptr, raw, nullptr);
    EXPECT_EQ(result.action, HYPHA_ACTION_FAIL);
    EXPECT_EQ(result.status, static_cast<int32_t>(HYPHA_STATUS_INVALID_STATE));
    EXPECT_EQ(calls, 0);
}

TEST(CycleStamp, AValidStampReachesTheStepAsRosTime)
{
    struct Recording
    {
        uint32_t* secs;

        void operator()(const hypha::CycleTime& time) const
        {
            *this->secs = time.stamp.secs;
        }
    };

    uint32_t                              secs = 0;
    auto                                  link = std::make_shared<hypha::detail::CycleLink>();
    hypha::detail::CycleRunner<Recording> runner(Recording{&secs}, link);
    HyphaCycleTime                        raw{};
    raw.message_stamp_valid = 1;
    raw.message_stamp_nanos = 7'000'000'000LL;

    const HyphaCallbackResult result = runner.invoke(nullptr, raw, nullptr);
    EXPECT_EQ(result.action, HYPHA_ACTION_CONTINUE);
    EXPECT_EQ(result.status, static_cast<int32_t>(HYPHA_STATUS_OK));
    EXPECT_EQ(secs, 7U);
}

struct ResetSlot
{
    std::unique_ptr<Held>* slot;

    void operator()() const
    {
        this->slot->reset();
    }
};

TEST_F(CycleNodeFixture, DestroyingAStartedCycleNodeInsideTheRunFailsTheRunWithoutTouchingPorts)
{
    hypha::Node& nodeRef = *this->node;
    Observed     observed;
    auto         held = std::make_unique<Held>(observed);
    held->setRate(hypha::Hz(100.0));
    ASSERT_TRUE(held->start(nodeRef).has_value());
    hypha::Timer reset = nodeRef.timer(hypha::Duration::fromMillis(50), ResetSlot{&held});

    // The timer destroys the node while the WaitSet runs. The cycle must not
    // step over the freed port afterwards. Each pass gives the runtime one tick.
    bool destroyed = false;
    for (int pass = 0; pass < 50 && !destroyed; ++pass) {
        hypha_test::advance(10_ms);
        static_cast<void>(nodeRef.runFor(1_ms));
        destroyed = held == nullptr;
    }
    ASSERT_TRUE(destroyed);
    hypha::RunResult run = nodeRef.runFor(1_ms);
    for (int pass = 0; pass < 50 && run.ok(); ++pass) {
        hypha_test::advance(10_ms);
        run = nodeRef.runFor(1_ms);
    }
    EXPECT_FALSE(run.ok());
    EXPECT_EQ(run.status(), hypha::Status::InvalidState);
}

TEST_F(CycleNodeFixture, NeverStartedCycleNodeIsDestroyedQuietly)
{
    Observed observed;
    {
        Held held(observed);
        EXPECT_FALSE(held.health().has_value());
    }
    EXPECT_EQ(observed.steps, 0);
}

TEST_F(CycleNodeFixture, StartedCycleNodeOutlivingItsNodeIsDestroyedQuietly)
{
    Observed observed;
    Held     held(observed);
    held.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(held.start(*this->node).has_value());
    this->node.reset();
    // Destroyed at the end of the test, after its node is gone.
}

class ThrowsNonStandard final : public hypha::CycleNode
{
  public:
    void setup(hypha::Node&) override
    {
        throw 42;
    }

    void step(const hypha::CycleTime&) override
    {
    }
};

class ConstructorThrows final : public hypha::CycleNode
{
  public:
    ConstructorThrows()
    {
        throw std::runtime_error("constructor refused");
    }

    void step(const hypha::CycleTime&) override
    {
    }
};

class CommitFails final : public hypha::CycleNode
{
  public:
    void setup(hypha::Node&) override
    {
        this->setRate(hypha::Hz(100.0));
    }

    hypha::Latest<Float64> absent{*this, "topic_absent"};

    void step(const hypha::CycleTime&) override
    {
    }
};

TEST_F(CycleNodeFixture, ASetterAfterStartThrows)
{
    hypha::Node& nodeRef = *this->node;
    Observed     observed;
    Held         held(observed);
    held.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(held.start(nodeRef).has_value());
    EXPECT_THROW(held.setRate(hypha::Hz(50.0)), std::logic_error);
}

TEST_F(CycleNodeFixture, AUsableValueIsDecodedOnceWhileItStaysUsable)
{
    hypha::Node&      nodeRef = *this->node;
    const std::string topic = topicFor("in_0");
    auto              publisher = hypha::Node().advertise<CountedValue>(hypha::Topic{topic});
    ASSERT_TRUE(publisher.enabled());

    CountedHeld counted;
    counted.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(counted.start(nodeRef).has_value());

    countedDecodes = 0;
    ASSERT_TRUE(publisher.publish(CountedValue{3.0}));
    ASSERT_TRUE(runUntil(
        nodeRef,
        [&counted]() {
            return counted.verdict == hypha::InputVerdict::Usable;
        },
        std::chrono::seconds(10)
    ));
    // Later steps within the 200 ms silence limit see the same value: no new decode.
    for (int step = 0; step < 5; ++step) {
        hypha_test::advance(10_ms);
        ASSERT_TRUE(nodeRef.runFor(1_ms).ok());
    }
    EXPECT_EQ(counted.verdict, hypha::InputVerdict::Usable);
    EXPECT_EQ(countedDecodes, 1);
}

class RunNodeTest : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->path = std::filesystem::temp_directory_path() / "hypha_cycle_node_run.toml";
        std::ofstream output(this->path);
        output << "count=7\n";
        output.close();
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        runNodeStepParameter = -1;
    }

    void TearDown() override
    {
        std::filesystem::remove(this->path);
    }

    std::filesystem::path path;
};

TEST_F(RunNodeTest, RunNodeSnapshotHasTheCycleRowWhileItRuns)
{
    char        first[] = "prog";
    std::string file = this->path.string();
    char*       argv[] = {first, file.data(), nullptr};
    runNodeSnapshot.clear();
    const int exitCode = hypha::runNode<RunNodeApp>(2, argv, "run_node_snapshot");
    EXPECT_EQ(exitCode, 0);
    EXPECT_EQ(runNodeStepParameter, 7) << "runNode passes the parameter file";
    // The snapshot taken while runNode runs has the cycle row with its name and rate.
    EXPECT_NE(runNodeSnapshot.find("cycles"), std::string::npos) << runNodeSnapshot;
    EXPECT_NE(runNodeSnapshot.find("run_node_snapshot"), std::string::npos) << runNodeSnapshot;
    EXPECT_NE(runNodeSnapshot.find("100.0"), std::string::npos) << runNodeSnapshot;
}

class StopInSetup final : public hypha::CycleNode
{
  public:
    void setup(hypha::Node&) override
    {
        std::ignore = hypha::config().optional<int>("count", -1);
        this->setRate(hypha::Hz(100.0));
        // A stop that arrives before run() must not let run() start and block.
        hypha::requestStop();
    }

    void step(const hypha::CycleTime&) override
    {
    }
};

/// A timer callback takes about 2 ms and asks for a stop. Its timer is due before
/// runNode's 1 ms executor turn, so the callback runs inside the turn.
struct BusyStop
{
    void operator()() const
    {
        const auto begin = std::chrono::steady_clock::now();
        while (std::chrono::steady_clock::now() - begin < std::chrono::milliseconds(2)) {
            std::this_thread::yield();
        }
        hypha::requestStop();
    }
};

class StopInExecutorTurn final : public hypha::CycleNode
{
  public:
    void setup(hypha::Node& node) override
    {
        std::ignore = hypha::config().optional<int>("count", -1);
        this->setRate(hypha::Hz(100.0));
        this->stop_ = node.timer(hypha::Duration::fromMillis(1), BusyStop{});
        hypha_test::advance(hypha::Duration::fromMillis(2));
    }

    void step(const hypha::CycleTime&) override
    {
    }

  private:
    hypha::Timer stop_;
};

/// Bounds a runNode call that runs on the test's own thread. Node creation needs
/// the main-thread stack (iceoryx2 config parsing recurses under ASan).
/// The watchdog thread only requests the stop; it never runs hypha calls otherwise.
class RunWatchdog
{
  public:
    explicit RunWatchdog(std::chrono::seconds bound)
        : bound_(bound), thread_(&RunWatchdog::watch, this)
    {
    }

    RunWatchdog(const RunWatchdog&) = delete;
    RunWatchdog& operator=(const RunWatchdog&) = delete;

    ~RunWatchdog()
    {
        this->finish();
    }

    /// Call it after the run returns.
    void finish()
    {
        {
            const std::lock_guard<std::mutex> lock(this->mutex_);
            this->finished_ = true;
        }
        this->wake_.notify_all();
        if (this->thread_.joinable()) {
            this->thread_.join();
        }
    }

    bool timedOut() const
    {
        return this->timedOut_.load();
    }

  private:
    void watch()
    {
        std::unique_lock<std::mutex> lock(this->mutex_);
        const bool finishedInTime = this->wake_.wait_for(lock, this->bound_, [this]() {
            return this->finished_;
        });
        if (!finishedInTime) {
            this->timedOut_.store(true);
            hypha::requestStop();
        }
    }

    std::chrono::seconds    bound_;
    std::mutex              mutex_;
    std::condition_variable wake_;
    bool                    finished_{false};
    std::atomic<bool>       timedOut_{false};
    std::thread             thread_;
};

TEST_F(RunNodeTest, RunNodeReturnsZeroWhenAStopEndsTheExecutorTurn)
{
    char        first[] = "prog";
    std::string file = this->path.string();
    char*       argv[] = {first, file.data(), nullptr};
    RunWatchdog watchdog(std::chrono::seconds(10));
    const int   exitCode = hypha::runNode<StopInExecutorTurn>(2, argv, "run_node_turn_stop");
    watchdog.finish();
    EXPECT_FALSE(watchdog.timedOut())
        << "runNode kept running after a stop ended its executor turn";
    EXPECT_EQ(exitCode, 0);
}

TEST_F(RunNodeTest, RunNodeReturnsZeroWhenAStopArrivesBeforeRun)
{
    char        first[] = "prog";
    std::string file = this->path.string();
    char*       argv[] = {first, file.data(), nullptr};
    // A bounded wait on the event: if runNode keeps running, the watchdog stops it and the test
    // fails.
    RunWatchdog watchdog(std::chrono::seconds(10));
    const int   exitCode = hypha::runNode<StopInSetup>(2, argv, "run_node_stop");
    watchdog.finish();
    EXPECT_FALSE(watchdog.timedOut()) << "runNode kept running after a stop request";
    EXPECT_EQ(exitCode, 0);
}

TEST_F(RunNodeTest, RunNodeReturnsTheRuntimeCodeForANonStandardThrow)
{
    char        first[] = "prog";
    std::string file = this->path.string();
    char*       argv[] = {first, file.data(), nullptr};
    EXPECT_EQ(
        (hypha::runNode<ThrowsNonStandard>(2, argv, "run_node_non_std")), hypha::kExitRuntime
    );
}

TEST_F(RunNodeTest, RunNodeReturnsTheRuntimeCodeWhenTheConstructorThrows)
{
    char        first[] = "prog";
    std::string file = this->path.string();
    char*       argv[] = {first, file.data(), nullptr};
    EXPECT_EQ((hypha::runNode<ConstructorThrows>(2, argv, "run_node_ctor")), hypha::kExitRuntime);
}

TEST_F(RunNodeTest, RunNodeReturnsTheConfigurationCodeWhenAPortTableIsMissing)
{
    // A port without its table (and so without a topic) is a refused configuration, as a
    // missing silence_ms is: a restart cannot fix it.
    char        first[] = "prog";
    std::string file = this->path.string();
    char*       argv[] = {first, file.data(), nullptr};
    EXPECT_EQ((hypha::runNode<CommitFails>(2, argv, "run_node_commit")), hypha::kExitConfiguration);
}
