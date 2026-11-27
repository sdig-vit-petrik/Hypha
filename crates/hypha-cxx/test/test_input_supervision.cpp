#include "hypha/cycle_node.h"
#include "hypha/hypha.h"
#include <hypha_msgs/NodeState.h>

#include "test_types.h"

#include <gtest/gtest.h>
#include <std_msgs/UInt32.h>

#include <chrono>
#include <cstddef>
#include <filesystem>
#include <fstream>
#include <functional>
#include <stdexcept>
#include <string>
#include <thread>
#include <utility>

using UInt32 = std_msgs::UInt32;
using namespace hypha::literals;

namespace {

std::string topicFor(const std::string& name)
{
    return hypha_test::testTopic("/" + name);
}

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

/// Runs @p milliseconds of simulated time in 10 ms slices. No event is awaited.
bool advanceSimulated(hypha::Node& node, int milliseconds)
{
    for (int elapsed = 0; elapsed < milliseconds; elapsed += 10) {
        hypha_test::advance(10_ms);
        if (!node.runFor(1_ms).ok()) {
            return false;
        }
    }
    return true;
}

/// Calls @p publish once per 10 ms slice until @p done holds. The predicate only observes.
bool publishUntil(
    hypha::Node&                 node,
    const std::function<void()>& publish,
    const std::function<bool()>& done,
    std::chrono::seconds         bound
)
{
    const auto deadline = std::chrono::steady_clock::now() + bound;
    while (!done()) {
        if (std::chrono::steady_clock::now() >= deadline) {
            return false;
        }
        publish();
        hypha_test::advance(10_ms);
        if (!node.runFor(1_ms).ok()) {
            return false;
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(1));
    }
    return true;
}

/// Writes a parameter file for one test and starts the node on it.
class SupervisionFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->path = std::filesystem::temp_directory_path() / "hypha_input_supervision_test.toml";
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
    }

    void TearDown() override
    {
        this->node.reset();
        std::filesystem::remove(this->path);
    }

    void initWith(const std::string& parameters)
    {
        std::ofstream output(this->path);
        output << parameters;
        output.close();
        this->node.emplace("input_supervision_test", this->path.string());
    }

    std::optional<hypha::Node> node;
    std::filesystem::path      path;
};

/// One Latest port named by the test. It requires its silence limit.
class LatestPort final : public hypha::CycleNode
{
  public:
    LatestPort(const std::string& portName, bool alive) : input(*this, portName)
    {
        if (alive) {
            this->input.setAlive(&UInt32::data);
        }
    }

    hypha::Latest<UInt32> input;
    int                   steps{0};

    void step(const hypha::CycleTime&) override
    {
        ++this->steps;
    }
};

/// A Latest port that marks its own fault as ignored, once, from inside step().
class IgnoringPort final : public hypha::CycleNode
{
  public:
    IgnoringPort() : input(*this, "arbiter")
    {
    }

    hypha::Latest<UInt32> input;

    void step(const hypha::CycleTime&) override
    {
        const bool faultToIgnore = this->input.stale() || this->input.stagnant();
        if (faultToIgnore) {
            this->input.setIgnored(true);
        }
    }
};

/// A Latest port that records inputsOk() of its last step. When @p ignoreFaults it
/// ignores a fault from inside step(); when @p alive it has the alive check.
class RecordsInputsOk final : public hypha::CycleNode
{
  public:
    RecordsInputsOk(bool ignoreFaults, bool alive)
        : input(*this, "arbiter"), ignoreFaults_(ignoreFaults)
    {
        if (alive) {
            this->input.setAlive(&UInt32::data);
        }
    }

    hypha::Latest<UInt32> input;
    bool                  lastOk{true};
    int                   steps{0};

    void step(const hypha::CycleTime&) override
    {
        ++this->steps;
        const bool faultToIgnore =
            this->ignoreFaults_ && (this->input.stale() || this->input.stagnant());
        if (faultToIgnore) {
            this->input.setIgnored(true);
        }
        this->lastOk = this->inputsOk();
    }

  private:
    bool ignoreFaults_;
};

/// The state of the node's `~/state`, as hypha::nodeState() reads it now.
uint8_t currentNodeState()
{
    const std::expected<hypha_msgs::NodeState, hypha::Status> state = hypha::nodeState();
    EXPECT_TRUE(state.has_value());
    return state.has_value() ? state->state : hypha_msgs::NodeState::WAITING;
}

int countOf(const std::string& output, const std::string& text)
{
    int         count = 0;
    std::size_t position = output.find(text);
    while (position != std::string::npos) {
        ++count;
        position = output.find(text, position + text.size());
    }
    return count;
}

/// Two Latest ports that read one topic, each with its own silence limit.
class SharedSourcePorts final : public hypha::CycleNode
{
  public:
    explicit SharedSourcePorts(hypha::Topic topic)
        : fast(*this, "fast", std::move(topic)), slow(*this, "slow", hypha::sharing(fast))
    {
    }

    hypha::Latest<UInt32> fast;
    hypha::Latest<UInt32> slow;

    void step(const hypha::CycleTime&) override
    {
    }
};

/// Throws from step() after the port has captured its step image.
class ThrowingStep final : public hypha::CycleNode
{
  public:
    ThrowingStep() : input(*this, "arbiter")
    {
    }

    hypha::Latest<UInt32> input;
    int                   steps{0};

    void step(const hypha::CycleTime&) override
    {
        ++this->steps;
        throw std::runtime_error("step failed");
    }
};

/// An unsupervised port that calls setIgnored() in step() and records the throw.
class PlainIgnoring final : public hypha::CycleNode
{
  public:
    PlainIgnoring() : input(*this, "arbiter", unsupervised)
    {
    }

    hypha::Latest<UInt32> input;
    int                   steps{0};
    bool                  threw{false};

    void step(const hypha::CycleTime&) override
    {
        ++this->steps;
        try {
            this->input.setIgnored(true);
        } catch (const std::logic_error&) {
            this->threw = true;
        }
    }
};

class ArrivalsPort final : public hypha::CycleNode
{
  public:
    explicit ArrivalsPort(bool alive = false) : input(*this, "arbiter")
    {
        if (alive) {
            this->input.setAlive(&UInt32::data);
        }
    }

    hypha::Arrivals<UInt32> input;

    void step(const hypha::CycleTime&) override
    {
    }
};

TEST_F(SupervisionFixture, AMissingSilenceLimitFailsStartAndNamesTheCyclePortAndKey)
{
    this->initWith("[ports.inputs.arbiter]\ntopic='" + topicFor("arbiter") + "'\n");
    hypha::Node& nodeRef = *this->node;
    LatestPort   port("arbiter", false);
    port.setRate(hypha::Hz(100.0));
    const auto started = port.start(nodeRef);
    ASSERT_FALSE(started.has_value());
    EXPECT_NE(started.error().find("cycle 'input_supervision_test'"), std::string::npos)
        << started.error();
    EXPECT_NE(started.error().find("input 'arbiter'"), std::string::npos) << started.error();
    EXPECT_NE(started.error().find("missing key 'silence_ms'"), std::string::npos)
        << started.error();
}

TEST_F(SupervisionFixture, ASilentPublisherMakesThePortSilentAndANewSampleClearsIt)
{
    const std::string topic = topicFor("arbiter");
    this->initWith("[ports.inputs.arbiter]\ntopic='" + topic + "'\nsilence_ms = 100\n");
    hypha::Node& nodeRef = *this->node;
    auto         publisher = hypha::Node().advertise<UInt32>(hypha::Topic{topic});
    ASSERT_TRUE(publisher.enabled());
    LatestPort port("arbiter", false);
    port.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(port.start(nodeRef).has_value());

    ASSERT_TRUE(publisher.publish(UInt32{1}));
    ASSERT_TRUE(runUntil(
        nodeRef,
        [&port]() {
            return port.input.received() > 0U;
        },
        std::chrono::seconds(10)
    ));
    ASSERT_TRUE(runUntil(
        nodeRef,
        [&port]() {
            return port.input.stale();
        },
        std::chrono::seconds(10)
    ));
    EXPECT_TRUE((port.input.stale() || port.input.stagnant()));

    ASSERT_TRUE(publisher.publish(UInt32{2}));
    ASSERT_TRUE(runUntil(
        nodeRef,
        [&port]() {
            return !port.input.stale();
        },
        std::chrono::seconds(10)
    ));
    EXPECT_FALSE((port.input.stale() || port.input.stagnant()));
}

TEST_F(SupervisionFixture, ARepeatedPayloadIsStagnantAndAChangedCounterClearsIt)
{
    const std::string topic = topicFor("arbiter");
    this->initWith(
        "[ports.inputs.arbiter]\ntopic='" + topic + "'\nsilence_ms = 1000\nstagnation_ms = 100\n"
    );
    hypha::Node& nodeRef = *this->node;
    auto         publisher = hypha::Node().advertise<UInt32>(hypha::Topic{topic});
    ASSERT_TRUE(publisher.enabled());
    LatestPort port("arbiter", true);
    port.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(port.start(nodeRef).has_value());

    ASSERT_TRUE(publishUntil(
        nodeRef,
        [&publisher]() {
            EXPECT_TRUE(publisher.publish(UInt32{5}));
        },
        [&port]() {
            return port.input.stagnant();
        },
        std::chrono::seconds(10)
    ));
    EXPECT_FALSE(port.input.stale());
    // bindingHealth() carries the supervision report of the last finished step.
    const std::expected<hypha::BindingHealth, hypha::Status> health = port.input.bindingHealth();
    ASSERT_TRUE(health.has_value());
    EXPECT_TRUE(health->stagnant);
    EXPECT_FALSE(health->stale);
    EXPECT_FALSE(health->ignored);

    ASSERT_TRUE(publishUntil(
        nodeRef,
        [&publisher]() {
            EXPECT_TRUE(publisher.publish(UInt32{6}));
        },
        [&port]() {
            return !port.input.stagnant();
        },
        std::chrono::seconds(10)
    ));
}

TEST_F(SupervisionFixture, AnIgnoredFaultStaysVisibleButIsNotCounted)
{
    const std::string topic = topicFor("arbiter");
    this->initWith("[ports.inputs.arbiter]\ntopic='" + topic + "'\nsilence_ms = 100\n");
    hypha::Node& nodeRef = *this->node;
    auto         publisher = hypha::Node().advertise<UInt32>(hypha::Topic{topic});
    ASSERT_TRUE(publisher.enabled());
    IgnoringPort port;
    port.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(port.start(nodeRef).has_value());

    ASSERT_TRUE(publisher.publish(UInt32{1}));
    ASSERT_TRUE(runUntil(
        nodeRef,
        [&port]() {
            return port.input.ignored();
        },
        std::chrono::seconds(10)
    ));
    // An ignored fault is hidden: the port reads as neither stale nor stagnant.
    EXPECT_FALSE(port.input.stale());
    EXPECT_FALSE(port.input.stagnant());
}

TEST_F(SupervisionFixture, TwoPortsOfOneCycleShareOneSourceWithTheirOwnLimits)
{
    const std::string topic = topicFor("shared");
    this->initWith(
        "[ports.inputs.fast]\nsilence_ms = 100\n[ports.inputs.slow]\nsilence_ms = "
        "5000\n"
    );
    hypha::Node& nodeRef = *this->node;
    auto         publisher = hypha::Node().advertise<UInt32>(hypha::Topic{topic});
    ASSERT_TRUE(publisher.enabled());
    SharedSourcePorts ports(hypha::Topic{topic});
    ports.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(ports.start(nodeRef).has_value());

    ASSERT_TRUE(publisher.publish(UInt32{1}));
    ASSERT_TRUE(runUntil(
        nodeRef,
        [&ports]() {
            return ports.fast.received() > 0U && ports.slow.received() > 0U;
        },
        std::chrono::seconds(10)
    ));

    // 1 s: past the short limit of 100 ms, far below the long limit of 5000 ms.
    ::testing::internal::CaptureStderr();
    ASSERT_TRUE(advanceSimulated(nodeRef, 1000));
    EXPECT_TRUE(ports.fast.stale());
    EXPECT_FALSE(ports.slow.stale());

    // 5.6 s: past the long limit too.
    ASSERT_TRUE(advanceSimulated(nodeRef, 4600));
    EXPECT_TRUE(ports.slow.stale());
    const std::string output = ::testing::internal::GetCapturedStderr();
    // Each port logs its own change under its own name.
    EXPECT_EQ(countOf(output, "input 'fast' (" + topic + ") is stale: "), 1) << output;
    EXPECT_EQ(countOf(output, "input 'slow' (" + topic + ") is stale: "), 1) << output;
}

TEST_F(SupervisionFixture, SupervisionSettersAfterStartThrow)
{
    this->initWith(
        "[ports.inputs.arbiter]\ntopic='" + topicFor("arbiter") + "'\nsilence_ms = 100\n"
    );
    hypha::Node& nodeRef = *this->node;
    LatestPort   port("arbiter", false);
    port.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(port.start(nodeRef).has_value());
    EXPECT_THROW(port.input.setAlive(&UInt32::data), std::logic_error);
    EXPECT_THROW(port.input.setIgnored(true), std::logic_error);
}

TEST_F(SupervisionFixture, AnArrivalsPortGoesStagnantOnARepeatedPayload)
{
    const std::string topic = topicFor("arbiter");
    this->initWith(
        "[ports.inputs.arbiter]\ntopic='" + topic + "'\nsilence_ms = 1000\nstagnation_ms = 100\n"
    );
    hypha::Node& nodeRef = *this->node;
    auto         publisher = hypha::Node().advertise<UInt32>(hypha::Topic{topic});
    ASSERT_TRUE(publisher.enabled());
    ArrivalsPort port(true);
    port.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(port.start(nodeRef).has_value());

    ASSERT_TRUE(publishUntil(
        nodeRef,
        [&publisher]() {
            EXPECT_TRUE(publisher.publish(UInt32{5}));
        },
        [&port]() {
            return port.input.stagnant();
        },
        std::chrono::seconds(10)
    ));
    EXPECT_FALSE(port.input.stale());
}

TEST_F(SupervisionFixture, ASilentArrivalsPortIsStaleAndAFreshSampleEndsIt)
{
    const std::string topic = topicFor("arbiter");
    this->initWith("[ports.inputs.arbiter]\ntopic='" + topic + "'\nsilence_ms = 100\n");
    hypha::Node& nodeRef = *this->node;
    auto         publisher = hypha::Node().advertise<UInt32>(hypha::Topic{topic});
    ASSERT_TRUE(publisher.enabled());
    ArrivalsPort port;
    port.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(port.start(nodeRef).has_value());

    ASSERT_TRUE(publisher.publish(UInt32{1}));
    ASSERT_TRUE(runUntil(
        nodeRef,
        [&port]() {
            return port.input.received() > 0U;
        },
        std::chrono::seconds(10)
    ));
    ASSERT_TRUE(runUntil(
        nodeRef,
        [&port]() {
            return port.input.stale();
        },
        std::chrono::seconds(10)
    ));
    // The step reads the silence from the verdict, not only from the fault.
    EXPECT_EQ(port.input.verdict(), hypha::InputVerdict::Stale);
    EXPECT_TRUE(port.input.samples().empty());

    ASSERT_TRUE(publisher.publish(UInt32{2}));
    ASSERT_TRUE(runUntil(
        nodeRef,
        [&port]() {
            return !port.input.stale();
        },
        std::chrono::seconds(10)
    ));
    EXPECT_NE(port.input.verdict(), hypha::InputVerdict::Stale);
}

TEST_F(SupervisionFixture, SetIgnoredAfterAStepThatThrowsThrowsLogicError)
{
    this->initWith(
        "[ports.inputs.arbiter]\ntopic='" + topicFor("arbiter") + "'\nsilence_ms = 100\n"
    );
    hypha::Node& nodeRef = *this->node;
    ThrowingStep port;
    port.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(port.start(nodeRef).has_value());
    for (int tick = 0; tick < 100 && port.steps == 0; ++tick) {
        hypha_test::advance(10_ms);
        static_cast<void>(nodeRef.runFor(1_ms));
    }
    ASSERT_GE(port.steps, 1);
    EXPECT_THROW(port.input.setIgnored(true), std::logic_error);
    EXPECT_FALSE(port.input.ignored());
}

TEST_F(SupervisionFixture, SetIgnoredOnAnUnsupervisedPortInsideStepThrowsLogicError)
{
    this->initWith("[ports.inputs.arbiter]\ntopic='" + topicFor("arbiter") + "'\n");
    hypha::Node&  nodeRef = *this->node;
    PlainIgnoring port;
    port.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(port.start(nodeRef).has_value());
    ASSERT_TRUE(runUntil(
        nodeRef,
        [&port]() {
            return port.steps > 0;
        },
        std::chrono::seconds(10)
    ));
    EXPECT_TRUE(port.threw);
    EXPECT_FALSE(port.input.ignored());
}

TEST_F(SupervisionFixture, NodeStateAndInputsOkAgreeForAStagnantInput)
{
    const std::string topic = topicFor("arbiter");
    this->initWith(
        "[ports.inputs.arbiter]\ntopic='" + topic + "'\nsilence_ms = 1000\nstagnation_ms = 100\n"
    );
    hypha::Node&    nodeRef = *this->node;
    auto            publisher = hypha::Node().advertise<UInt32>(hypha::Topic{topic});
    RecordsInputsOk port(false, true);
    port.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(port.start(nodeRef).has_value());

    ASSERT_TRUE(publishUntil(
        nodeRef,
        [&publisher]() {
            EXPECT_TRUE(publisher.publish(UInt32{5}));
        },
        [&port]() {
            return port.input.stagnant();
        },
        std::chrono::seconds(10)
    ));
    ASSERT_TRUE(publisher.publish(UInt32{5}));
    ASSERT_TRUE(advanceSimulated(nodeRef, 10));

    EXPECT_EQ(port.input.verdict(), hypha::InputVerdict::Usable);
    EXPECT_FALSE(port.lastOk) << "a stagnant input is not ok in the step";
    EXPECT_FALSE(port.inputsOk()) << "nor between steps";
    EXPECT_EQ(currentNodeState(), hypha_msgs::NodeState::DEGRADED);
    const std::expected<hypha_msgs::NodeState, hypha::Status> state = hypha::nodeState();
    ASSERT_TRUE(state.has_value());
    ASSERT_EQ(state->inputs.size(), 1U);
    EXPECT_TRUE(state->inputs[0].stagnant) << "the row says why the node is degraded";
    EXPECT_FALSE(state->inputs[0].ignored);
}

TEST_F(SupervisionFixture, NodeStateAndInputsOkAgreeForAnIgnoredStaleInput)
{
    const std::string topic = topicFor("arbiter");
    this->initWith("[ports.inputs.arbiter]\ntopic='" + topic + "'\nsilence_ms = 100\n");
    hypha::Node&    nodeRef = *this->node;
    auto            publisher = hypha::Node().advertise<UInt32>(hypha::Topic{topic});
    RecordsInputsOk port(true, false);
    port.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(port.start(nodeRef).has_value());

    ASSERT_TRUE(publisher.publish(UInt32{1}));
    ASSERT_TRUE(runUntil(
        nodeRef,
        [&port]() {
            return port.input.ignored();
        },
        std::chrono::seconds(10)
    ));
    ASSERT_TRUE(advanceSimulated(nodeRef, 20));

    EXPECT_EQ(port.input.verdict(), hypha::InputVerdict::Stale);
    EXPECT_TRUE(port.lastOk) << "an ignored stale input is ok in the step";
    EXPECT_TRUE(port.inputsOk());
    EXPECT_EQ(currentNodeState(), hypha_msgs::NodeState::INPUTS_USABLE);
    const std::expected<hypha_msgs::NodeState, hypha::Status> state = hypha::nodeState();
    ASSERT_TRUE(state.has_value());
    ASSERT_EQ(state->inputs.size(), 1U);
    EXPECT_EQ(state->inputs[0].verdict, hypha_msgs::RequiredInput::STALE) << "the row keeps it";
    EXPECT_TRUE(state->inputs[0].ignored);
    EXPECT_GT(state->inputs[0].age_nanos, 100'000'000U) << "a stale input reports its age";
    // bindingHealth() reports the fault as ~/state does: present, and ignored.
    const std::expected<hypha::BindingHealth, hypha::Status> health = port.input.bindingHealth();
    ASSERT_TRUE(health.has_value());
    EXPECT_TRUE(health->stale) << "an ignored fault stays in the report";
    EXPECT_FALSE(health->stagnant);
    EXPECT_TRUE(health->ignored);
}

TEST_F(SupervisionFixture, ANodeWithItsOwnMainLogsTheChangeAndReadingTheStateLogsNothing)
{
    const std::string topic = topicFor("arbiter");
    this->initWith("[ports.inputs.arbiter]\ntopic='" + topic + "'\nsilence_ms = 100\n");
    hypha::Node& nodeRef = *this->node;
    auto         publisher = hypha::Node().advertise<UInt32>(hypha::Topic{topic});
    LatestPort   port("arbiter", false);
    port.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(port.start(nodeRef).has_value());

    ::testing::internal::CaptureStderr();
    ASSERT_TRUE(publisher.publish(UInt32{1}));
    ASSERT_TRUE(runUntil(
        nodeRef,
        [&port]() {
            return port.input.stale();
        },
        std::chrono::seconds(10)
    ));
    ASSERT_TRUE(advanceSimulated(nodeRef, 50));
    const std::string run = ::testing::internal::GetCapturedStderr();
    const std::string subject = "input 'arbiter' (" + topic + ")";
    EXPECT_EQ(countOf(run, subject + " is usable"), 1) << run;
    EXPECT_EQ(countOf(run, subject + " is stale: newest sample is "), 1) << run;
    EXPECT_EQ(countOf(run, ", silence_ms = 100"), 1) << run;

    ::testing::internal::CaptureStderr();
    for (int call = 0; call < 3; ++call) {
        EXPECT_EQ(currentNodeState(), hypha_msgs::NodeState::DEGRADED);
    }
    const std::string reads = ::testing::internal::GetCapturedStderr();
    EXPECT_EQ(countOf(reads, "hypha:"), 0) << "reading the node state logs nothing: " << reads;
}

struct NoStep
{
    void operator()(const hypha::CycleTime&) const
    {
    }
};

/// A Latest port named by the test with the tags the test gives.
template <typename... Tags>
class TaggedPort final : public hypha::CycleNode
{
  public:
    explicit TaggedPort(Tags... tags) : input(*this, "arbiter", tags...)
    {
    }

    hypha::Latest<UInt32> input;

    void step(const hypha::CycleTime&) override
    {
    }
};

/// The console line that reports the stale verdict of the input on @p topic, or empty.
std::string staleLineOf(const std::string& output, const std::string& topic)
{
    const std::string subject = "(" + topic + ") is stale: ";
    const std::size_t found = output.find(subject);
    if (found == std::string::npos) {
        return {};
    }
    const std::size_t begin = output.rfind('\n', found);
    const std::size_t end = output.find('\n', found);
    return output.substr(
        begin == std::string::npos ? 0 : begin + 1,
        end == std::string::npos ? std::string::npos : end - begin
    );
}

/// Starts @p port, sends one sample on @p topic and lets the input go stale. Returns the
/// console line of that verdict.
template <typename Port>
std::string staleLineOfStartedPort(hypha::Node& node, Port& port, const std::string& topic)
{
    auto publisher = hypha::Node().advertise<UInt32>(hypha::Topic{topic});
    port.setRate(hypha::Hz(100.0));
    EXPECT_TRUE(port.start(node).has_value());
    ::testing::internal::CaptureStderr();
    EXPECT_TRUE(publisher.publish(UInt32{1}));
    EXPECT_TRUE(runUntil(
        node,
        [&port]() {
            return port.input.stale();
        },
        std::chrono::seconds(10)
    ));
    EXPECT_TRUE(advanceSimulated(node, 50));
    return staleLineOf(::testing::internal::GetCapturedStderr(), topic);
}

TEST_F(SupervisionFixture, AnAdvisoryInputLogsItsStaleVerdictAtInfo)
{
    const std::string topic = topicFor("arbiter");
    this->initWith("[ports.inputs.arbiter]\ntopic='" + topic + "'\nsilence_ms = 100\n");
    TaggedPort        port(hypha::advisory);
    const std::string line = staleLineOfStartedPort(*this->node, port, topic);
    EXPECT_NE(line.find("[INFO]"), std::string::npos) << line;
    EXPECT_NE(line.find("(advisory)"), std::string::npos) << line;
}

TEST_F(SupervisionFixture, ARequiredInputLogsItsStaleVerdictAtWarn)
{
    const std::string topic = topicFor("arbiter");
    this->initWith("[ports.inputs.arbiter]\ntopic='" + topic + "'\nsilence_ms = 100\n");
    LatestPort        port("arbiter", false);
    const std::string line = staleLineOfStartedPort(*this->node, port, topic);
    EXPECT_NE(line.find("[WARN]"), std::string::npos) << line;
    EXPECT_EQ(line.find("(advisory)"), std::string::npos) << line;
}

TEST_F(SupervisionFixture, AnUnsupervisedInputStartsWithoutSilenceMsAndHasNoLimit)
{
    const std::string topic = topicFor("arbiter");
    this->initWith("[ports.inputs.arbiter]\ntopic='" + topic + "'\n");
    auto       publisher = hypha::Node().advertise<UInt32>(hypha::Topic{topic});
    TaggedPort port(hypha::unsupervised);
    port.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(port.start(*this->node).has_value());
    ASSERT_TRUE(publisher.publish(UInt32{1}));
    ASSERT_TRUE(runUntil(
        *this->node,
        [&port]() {
            return port.input.usable();
        },
        std::chrono::seconds(10)
    ));
    ASSERT_TRUE(advanceSimulated(*this->node, 300));
    EXPECT_TRUE(port.input.usable()) << "an unsupervised input never goes stale";
    EXPECT_FALSE(port.input.stale());
    EXPECT_EQ(port.input->data, 1U) << "and stays readable";
}

TEST_F(SupervisionFixture, SilenceMsOnAnUnsupervisedPortIsRefused)
{
    this->initWith(
        "[ports.inputs.arbiter]\ntopic='" + topicFor("arbiter") + "'\nsilence_ms = 100\n"
    );
    TaggedPort port(hypha::unsupervised);
    port.setRate(hypha::Hz(100.0));
    const auto started = port.start(*this->node);
    ASSERT_FALSE(started.has_value());
    EXPECT_NE(
        started.error().find("key 'silence_ms' on a port tagged unsupervised"), std::string::npos
    ) << started.error();
}

TEST_F(SupervisionFixture, UnsupervisedAndAdvisoryTogetherAreRefused)
{
    this->initWith("[ports.inputs.arbiter]\ntopic='" + topicFor("arbiter") + "'\n");
    TaggedPort port(hypha::unsupervised, hypha::advisory);
    port.setRate(hypha::Hz(100.0));
    const auto started = port.start(*this->node);
    ASSERT_FALSE(started.has_value());
    EXPECT_NE(
        started.error().find("the tags unsupervised and advisory exclude each other"),
        std::string::npos
    ) << started.error();
}

TEST_F(SupervisionFixture, AnAdvisoryInputStartsWithoutSilenceMs)
{
    this->initWith("[ports.inputs.arbiter]\ntopic='" + topicFor("arbiter") + "'\n");
    TaggedPort port(hypha::advisory);
    port.setRate(hypha::Hz(100.0));
    EXPECT_TRUE(port.start(*this->node).has_value());
}

TEST_F(SupervisionFixture, ATablelessRequiredInputIsRefusedWithoutTheTag)
{
    this->initWith("");
    hypha::Cycle          cycle("tableless_cycle");
    hypha::Latest<UInt32> input{cycle, hypha::Topic{topicFor("tableless")}};
    cycle.setRate(hypha::Hz(100.0));
    const auto started = cycle.start(*this->node, NoStep{});
    ASSERT_FALSE(started.has_value());
    EXPECT_NE(
        started.error().find("a required input without a table needs the tag hypha::unsupervised"),
        std::string::npos
    ) << started.error();
}

/// The text and verdict of the InputNotUsable that reading @p port throws; empty when
/// it throws none.
std::pair<std::string, hypha::InputVerdict> readFailure(const hypha::Latest<UInt32>& port)
{
    try {
        static_cast<void>(*port);
    } catch (const hypha::InputNotUsable& error) {
        return {error.what(), error.verdict()};
    }
    return {"", hypha::InputVerdict::Usable};
}

TEST_F(SupervisionFixture, ReadingAStaleLatestThrowsInputNotUsableNamingPortTopicAgeAndLimit)
{
    const std::string topic = topicFor("arbiter");
    this->initWith("[ports.inputs.arbiter]\ntopic='" + topic + "'\nsilence_ms = 100\n");
    auto       publisher = hypha::Node().advertise<UInt32>(hypha::Topic{topic});
    TaggedPort port;
    port.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(port.start(*this->node).has_value());

    using Failure = std::pair<std::string, hypha::InputVerdict>;
    const Failure     neverFailure = readFailure(port.input);
    const std::string never = neverFailure.first;
    EXPECT_EQ(neverFailure.second, hypha::InputVerdict::NeverArrived);
    EXPECT_NE(
        never.find("hypha: input 'arbiter' (" + topic + ") never arrived: no sample for "),
        std::string::npos
    ) << never;
    EXPECT_THROW(static_cast<void>(port.input.lastKnown()), std::logic_error);

    ASSERT_TRUE(publisher.publish(UInt32{7}));
    ASSERT_TRUE(runUntil(
        *this->node,
        [&port]() {
            return port.input.usable();
        },
        std::chrono::seconds(10)
    ));
    EXPECT_EQ(port.input->data, 7U) << "a usable value reads";
    ASSERT_TRUE(runUntil(
        *this->node,
        [&port]() {
            return port.input.stale();
        },
        std::chrono::seconds(10)
    ));
    const Failure     staleFailure = readFailure(port.input);
    const std::string stale = staleFailure.first;
    EXPECT_EQ(staleFailure.second, hypha::InputVerdict::Stale);
    const std::string expectedStart =
        "hypha: input 'arbiter' (" + topic + ") is stale: its newest sample is ";
    EXPECT_EQ(stale.rfind(expectedStart, 0), 0U) << stale;
    EXPECT_NE(
        stale.find(" ms old, silence_ms = 100; read lastKnown() to use the old value"),
        std::string::npos
    ) << stale;
    EXPECT_EQ(port.input.lastKnown().data, 7U) << "lastKnown() reads the old value";
    const UInt32 fallback{0};
    EXPECT_EQ(port.input.valueOr(fallback).data, 0U) << "valueOr needs a usable value";
}

TEST_F(SupervisionFixture, AnIgnoredStaleInputStillThrows)
{
    const std::string topic = topicFor("arbiter");
    this->initWith("[ports.inputs.arbiter]\ntopic='" + topic + "'\nsilence_ms = 100\n");
    auto            publisher = hypha::Node().advertise<UInt32>(hypha::Topic{topic});
    RecordsInputsOk port(true, false);
    port.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(port.start(*this->node).has_value());
    ASSERT_TRUE(publisher.publish(UInt32{1}));
    ASSERT_TRUE(runUntil(
        *this->node,
        [&port]() {
            return port.input.ignored();
        },
        std::chrono::seconds(10)
    ));
    EXPECT_THROW(static_cast<void>(*port.input), hypha::InputNotUsable);
}

TEST_F(SupervisionFixture, AStagnantUsableInputIsReadable)
{
    const std::string topic = topicFor("arbiter");
    this->initWith(
        "[ports.inputs.arbiter]\ntopic='" + topic + "'\nsilence_ms = 1000\nstagnation_ms = 100\n"
    );
    auto       publisher = hypha::Node().advertise<UInt32>(hypha::Topic{topic});
    LatestPort port("arbiter", true);
    port.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(port.start(*this->node).has_value());
    ASSERT_TRUE(publishUntil(
        *this->node,
        [&publisher]() {
            EXPECT_TRUE(publisher.publish(UInt32{5}));
        },
        [&port]() {
            return port.input.stagnant();
        },
        std::chrono::seconds(10)
    ));
    EXPECT_EQ(port.input->data, 5U);
}

TEST_F(SupervisionFixture, UnsupervisedAndAdvisoryTogetherAreRefusedOnATablelessPort)
{
    this->initWith("");
    hypha::Cycle          cycle("tableless_tags_cycle");
    hypha::Latest<UInt32> input{
        cycle, hypha::Topic{topicFor("tableless_tags")}, hypha::unsupervised, hypha::advisory
    };
    cycle.setRate(hypha::Hz(100.0));
    const auto started = cycle.start(*this->node, NoStep{});
    ASSERT_FALSE(started.has_value());
    EXPECT_NE(
        started.error().find("the tags unsupervised and advisory exclude each other"),
        std::string::npos
    ) << started.error();
}

}  // namespace
