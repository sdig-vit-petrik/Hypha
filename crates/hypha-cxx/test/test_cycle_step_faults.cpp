// What a running cycle does when its rate, its step or its input data is bad,
// and how a queued port reads a fast source.

#include "hypha/cycle_node.h"
#include "hypha/hypha.h"
#include <hypha_msgs/NodeState.h>

#include "test_types.h"

#include <gtest/gtest.h>
#include <std_msgs/Bool.h>
#include <std_msgs/Float64.h>

#include <chrono>
#include <cmath>
#include <cstdint>
#include <expected>
#include <filesystem>
#include <fstream>
#include <limits>
#include <new>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

using Float64 = std_msgs::Float64;
using namespace hypha::literals;

namespace {

std::string faultTopic(const std::string& name)
{
    return hypha_test::testTopic("/" + name);
}

class CycleStepFaultFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->path = std::filesystem::temp_directory_path() / "hypha_cycle_step_faults.toml";
        std::ofstream output(this->path);
        output << "[ports.outputs.command]\ntopic='" << faultTopic("fault_output") << "'\n";
        output.close();
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("cycle_step_faults", this->path.string());
    }

    void TearDown() override
    {
        this->node.reset();
        std::filesystem::remove(this->path);
    }

    std::optional<hypha::Node> node;
    std::filesystem::path      path;
};

struct NoopStep
{
    void operator()(const hypha::CycleTime&) const
    {
    }
};

struct ThrowingStep
{
    int* calls;

    void operator()(const hypha::CycleTime&) const
    {
        ++*this->calls;
        throw std::runtime_error("simulated uncaught step failure");
    }
};

struct AllocatingStep
{
    void operator()(const hypha::CycleTime&) const
    {
        throw std::bad_alloc();
    }
};

struct FlagStep
{
    int*                           calls;
    bool*                          lastFlag;
    hypha::Latest<std_msgs::Bool>* flag;

    void operator()(const hypha::CycleTime&) const
    {
        ++*this->calls;
        if (this->flag->usable()) {
            *this->lastFlag = (*this->flag)->data;
        }
    }
};

struct ImuStep
{
    int*                      calls;
    hypha::Arrivals<Float64>* samples;
    std::vector<double>*      received;

    void operator()(const hypha::CycleTime&) const
    {
        ++*this->calls;
        for (const Float64& sample : this->samples->samples()) {
            this->received->push_back(sample.data);
        }
    }
};

}  // namespace

TEST_F(CycleStepFaultFixture, UnrepresentableCycleRatesRefuseStart)
{
    hypha::Node& nodeRef = *this->node;

    hypha::Cycle tooSlow("cycle_rate_too_slow");
    tooSlow.setRate(hypha::Hz(1.0e-300));
    const auto slowStart = tooSlow.start(nodeRef, NoopStep{});
    ASSERT_FALSE(slowStart.has_value());
    EXPECT_NE(slowStart.error().find("InvalidArgument"), std::string::npos);

    const double boundaryRate = std::ldexp(1.0e9, -63);
    hypha::Cycle outsideInt64("cycle_rate_int64_boundary");
    outsideInt64.setRate(hypha::Hz(std::nextafter(boundaryRate, 0.0)));
    EXPECT_FALSE(outsideInt64.start(nodeRef, NoopStep{}).has_value());

    hypha::Cycle insideInt64("cycle_rate_inside_int64");
    insideInt64.setRate(
        hypha::Hz(std::nextafter(boundaryRate, std::numeric_limits<double>::infinity()))
    );
    EXPECT_TRUE(insideInt64.start(nodeRef, NoopStep{}).has_value());

    hypha::Cycle infinite("cycle_rate_infinite");
    infinite.setRate(hypha::Hz(std::numeric_limits<double>::infinity()));
    EXPECT_FALSE(infinite.start(nodeRef, NoopStep{}).has_value());

    hypha::Cycle subNanosecond("cycle_rate_too_fast");
    subNanosecond.setRate(hypha::Hz(1.0e12));
    EXPECT_FALSE(subNanosecond.start(nodeRef, NoopStep{}).has_value());

    hypha::Cycle notSet("cycle_rate_not_set");
    EXPECT_FALSE(notSet.start(nodeRef, NoopStep{}).has_value());
}

TEST_F(CycleStepFaultFixture, AnExceptionInTheStepFailsTheRunWithPanic)
{
    hypha::Node&           nodeRef = *this->node;
    hypha::Cycle           cycle("cycle_step_throw");
    hypha::Latest<Float64> input{
        cycle, hypha::Topic{faultTopic("throw_input")}, hypha::unsupervised
    };
    cycle.setRate(hypha::Hz(10.0));
    int calls = 0;
    ASSERT_TRUE(cycle.start(nodeRef, ThrowingStep{&calls}).has_value());

    ::testing::internal::CaptureStderr();
    hypha_test::advance(100_ms);
    const hypha::RunResult failed = nodeRef.runFor(1_ms);
    ::testing::internal::GetCapturedStderr();

    EXPECT_FALSE(failed.ok());
    EXPECT_EQ(failed.reason(), hypha::StopReason::RuntimeFailed);
    EXPECT_EQ(failed.status(), hypha::Status::Panic);
    EXPECT_EQ(calls, 1);
    const auto health = cycle.health();
    ASSERT_TRUE(health.has_value());
    EXPECT_EQ(health->callback_failures, 1U);
}

TEST_F(CycleStepFaultFixture, AnAllocationFailureInTheStepReportsResourceLimit)
{
    hypha::Node& nodeRef = *this->node;
    hypha::Cycle cycle("cycle_step_bad_alloc");
    cycle.setRate(hypha::Hz(10.0));
    ASSERT_TRUE(cycle.start(nodeRef, AllocatingStep{}).has_value());

    hypha_test::advance(100_ms);
    const hypha::RunResult run = nodeRef.runFor(1_ms);
    EXPECT_FALSE(run.ok());
    EXPECT_EQ(run.reason(), hypha::StopReason::RuntimeFailed);
    EXPECT_EQ(run.status(), hypha::Status::ResourceLimit);
}

/// A raw publisher of Float64 frames on @p topic, which sends any bytes.
hypha::RawPublisher float64Raw(const std::string& topic)
{
    return hypha::rawPublisher(
               topic,
               {.dataType = hypha::codec::dataType<Float64>(),
                .md5 = hypha::codec::md5Sum<Float64>(),
                .definition = hypha::codec::definition<Float64>()}
    )
        .value();
}

/// The ROS 1 bytes of one Float64.
std::vector<uint8_t> float64Bytes(double value)
{
    return hypha::codec::encode(Float64{value});
}

/// Records what a Latest port shows in each step.
struct LatestSeen
{
    int                   steps{0};
    std::optional<double> value;
    bool                  fresh{false};
    hypha::InputVerdict   verdict{hypha::InputVerdict::NeverArrived};
};

struct RecordLatest
{
    LatestSeen*             seen;
    hypha::Latest<Float64>* input;

    void operator()(const hypha::CycleTime&) const
    {
        ++this->seen->steps;
        this->seen->fresh = this->input->fresh();
        this->seen->verdict = this->input->verdict();
        this->seen->value.reset();
        if (this->input->has()) {
            this->seen->value = (**this->input).data;
        }
    }
};

struct RecordArrivals
{
    std::vector<double>*      seen;
    hypha::Arrivals<Float64>* input;

    void operator()(const hypha::CycleTime&) const
    {
        for (const Float64& sample : this->input->samples()) {
            this->seen->push_back(sample.data);
        }
    }
};

/// One undecodable-input row of the node state, or 0 when the node has none.
uint64_t undecodableOfNode()
{
    const std::expected<hypha_msgs::NodeState, hypha::Status> state = hypha::nodeState();
    EXPECT_TRUE(state.has_value());
    const bool hasRow = state.has_value() && !state->inputs.empty();
    return hasRow ? state->inputs.front().undecodable : 0U;
}

TEST_F(CycleStepFaultFixture, AnUndecodableSampleDoesNotFailTheRunAndIsCountedInTheNodeState)
{
    hypha::Node&      nodeRef = *this->node;
    const std::string inputTopic = faultTopic("fault_input");
    auto              raw = float64Raw(inputTopic);
    ASSERT_TRUE(raw.isValid());
    hypha::Cycle           cycle("decode_failure");
    hypha::Latest<Float64> input{cycle, hypha::Topic{inputTopic}, hypha::unsupervised};
    cycle.setRate(hypha::Hz(10.0));
    LatestSeen seen;
    ASSERT_TRUE(cycle.start(nodeRef, RecordLatest{&seen, &input}).has_value());

    ASSERT_EQ(raw.publish(float64Bytes(1.5)), hypha::Status::Ok);
    hypha_test::advance(100_ms);
    ASSERT_TRUE(nodeRef.runFor(1_ms).ok());
    ASSERT_EQ(seen.value, std::optional<double>(1.5));

    ::testing::internal::CaptureStderr();
    ASSERT_EQ(raw.publish(std::vector<uint8_t>{0x01}), hypha::Status::Ok);
    hypha_test::advance(100_ms);
    const hypha::RunResult run = nodeRef.runFor(1_ms);
    const std::string      output = ::testing::internal::GetCapturedStderr();

    EXPECT_TRUE(run.ok()) << "an undecodable sample does not fail the run";
    EXPECT_EQ(seen.steps, 2) << "the step still runs";
    EXPECT_EQ(seen.value, std::optional<double>(1.5)) << "the port keeps its value";
    EXPECT_FALSE(seen.fresh) << "the bad sample is not a new value";
    EXPECT_EQ(seen.verdict, hypha::InputVerdict::Usable);
    EXPECT_EQ(undecodableOfNode(), 1U);
    EXPECT_NE(
        output.find(
            "(" + inputTopic + "): a sample did not decode as std_msgs/Float64; dropped (1 so far)"
        ),
        std::string::npos
    ) << output;
}

TEST_F(CycleStepFaultFixture, AGoodAndABadSampleOfOneDrainLeaveTheGoodValue)
{
    hypha::Node&           nodeRef = *this->node;
    const std::string      inputTopic = faultTopic("good_then_bad");
    auto                   raw = float64Raw(inputTopic);
    hypha::Cycle           cycle("good_then_bad");
    hypha::Latest<Float64> input{cycle, hypha::Topic{inputTopic}, hypha::unsupervised};
    cycle.setRate(hypha::Hz(10.0));
    LatestSeen seen;
    ASSERT_TRUE(cycle.start(nodeRef, RecordLatest{&seen, &input}).has_value());

    // Under simulated time the drain keeps every frame: the capture selects the bad one.
    ::testing::internal::CaptureStderr();
    ASSERT_EQ(raw.publish(float64Bytes(2.5)), hypha::Status::Ok);
    ASSERT_EQ(raw.publish(std::vector<uint8_t>{0x01}), hypha::Status::Ok);
    hypha_test::advance(100_ms);
    const hypha::RunResult run = nodeRef.runFor(1_ms);
    ::testing::internal::GetCapturedStderr();

    EXPECT_TRUE(run.ok()) << "the bad sample fails nothing";
    EXPECT_EQ(seen.steps, 1);
    EXPECT_EQ(seen.value, std::optional<double>(2.5)) << "the good sample of the drain is held";
    EXPECT_TRUE(seen.fresh);
    EXPECT_EQ(seen.verdict, hypha::InputVerdict::Usable);
}

TEST_F(CycleStepFaultFixture, AFirstSampleThatDoesNotDecodeLeavesTheInputNeverArrived)
{
    hypha::Node&           nodeRef = *this->node;
    const std::string      inputTopic = faultTopic("first_bad");
    auto                   raw = float64Raw(inputTopic);
    hypha::Cycle           cycle("first_bad");
    hypha::Latest<Float64> input{cycle, hypha::Topic{inputTopic}, hypha::unsupervised};
    cycle.setRate(hypha::Hz(10.0));
    LatestSeen seen;
    ASSERT_TRUE(cycle.start(nodeRef, RecordLatest{&seen, &input}).has_value());

    ::testing::internal::CaptureStderr();
    ASSERT_EQ(raw.publish(std::vector<uint8_t>{0x01}), hypha::Status::Ok);
    hypha_test::advance(100_ms);
    ASSERT_TRUE(nodeRef.runFor(1_ms).ok());
    ::testing::internal::GetCapturedStderr();

    EXPECT_EQ(seen.steps, 1);
    EXPECT_FALSE(seen.value.has_value());
    EXPECT_EQ(seen.verdict, hypha::InputVerdict::NeverArrived);
    EXPECT_FALSE(cycle.inputsOk());
}

TEST_F(CycleStepFaultFixture, ArrivalsDeliversTheGoodSamplesAroundABadOneInOrder)
{
    hypha::Node&             nodeRef = *this->node;
    const std::string        inputTopic = faultTopic("arrivals_bad");
    auto                     raw = float64Raw(inputTopic);
    hypha::Cycle             cycle("arrivals_bad");
    hypha::Arrivals<Float64> input{cycle, hypha::Topic{inputTopic}, hypha::unsupervised};
    cycle.setRate(hypha::Hz(10.0));
    std::vector<double> seen;
    ASSERT_TRUE(cycle.start(nodeRef, RecordArrivals{&seen, &input}).has_value());

    ::testing::internal::CaptureStderr();
    ASSERT_EQ(raw.publish(float64Bytes(1.0)), hypha::Status::Ok);
    ASSERT_EQ(raw.publish(std::vector<uint8_t>{0x01}), hypha::Status::Ok);
    ASSERT_EQ(raw.publish(float64Bytes(2.0)), hypha::Status::Ok);
    hypha_test::advance(100_ms);
    const hypha::RunResult run = nodeRef.runFor(1_ms);
    ::testing::internal::GetCapturedStderr();

    EXPECT_TRUE(run.ok());
    EXPECT_EQ(seen, (std::vector<double>{1.0, 2.0}));
    EXPECT_EQ(undecodableOfNode(), 1U);
}

/// Reads its input with *port, unchecked, and sends what it read.
struct ReadUnchecked
{
    hypha::Latest<Float64>* input;
    hypha::Output<Float64>* command;

    void operator()(const hypha::CycleTime&) const
    {
        this->command->send(Float64{(**this->input).data});
    }
};

TEST_F(CycleStepFaultFixture, AStepThatReadsAnInputThatIsNotUsableFailsTheRunAndSendsNothing)
{
    hypha::Node& nodeRef = *this->node;
    auto         outputSubscriber =
        hypha::Node().subscribe<Float64>(hypha::Topic{faultTopic("fault_output")});
    hypha::Cycle           cycle("unchecked_read");
    hypha::Latest<Float64> input{
        cycle, hypha::Topic{faultTopic("unchecked_input")}, hypha::unsupervised
    };
    hypha::Output<Float64> command{cycle, "command"};
    cycle.setRate(hypha::Hz(10.0));
    ASSERT_TRUE(cycle.start(nodeRef, ReadUnchecked{&input, &command}).has_value());

    ::testing::internal::CaptureStderr();
    hypha_test::advance(100_ms);
    const hypha::RunResult run = nodeRef.runFor(1_ms);
    const std::string      output = ::testing::internal::GetCapturedStderr();

    EXPECT_FALSE(run.ok());
    EXPECT_EQ(run.reason(), hypha::StopReason::RuntimeFailed);
    EXPECT_NE(output.find("step threw: hypha: input 'source' ("), std::string::npos) << output;
    EXPECT_NE(output.find("never arrived"), std::string::npos) << output;
    EXPECT_FALSE(outputSubscriber.latestSample().has_value());
}

TEST_F(CycleStepFaultFixture, ANonCanonicalRosTrueReachesTheStepAsTrue)
{
    hypha::Node&      nodeRef = *this->node;
    const std::string inputTopic = faultTopic("flag");
    auto              raw = hypha::rawPublisher(
                                inputTopic,
                                {.dataType = hypha::codec::dataType<std_msgs::Bool>(),
                                 .md5 = hypha::codec::md5Sum<std_msgs::Bool>(),
                                 .definition = hypha::codec::definition<std_msgs::Bool>()}
    )
                                .value();
    ASSERT_TRUE(raw.isValid());

    hypha::Cycle                  cycle("ros_bool");
    hypha::Latest<std_msgs::Bool> flag{cycle, hypha::Topic{inputTopic}, hypha::unsupervised};
    cycle.setRate(hypha::Hz(10.0));
    int  calls = 0;
    bool lastFlag = false;
    ASSERT_TRUE(cycle.start(nodeRef, FlagStep{&calls, &lastFlag, &flag}).has_value());

    // A ROS 1 publisher may send any nonzero byte for true; 0x20 was seen live.
    ASSERT_EQ(raw.publish(std::vector<uint8_t>{0x20}), hypha::Status::Ok);
    hypha_test::advance(100_ms);
    const hypha::RunResult initial = nodeRef.runFor(1_ms);
    ASSERT_TRUE(initial.ok()) << initial.status().asInt();

    hypha_test::advance(100_ms);
    const hypha::RunResult decoded = nodeRef.runFor(1_ms);
    ASSERT_TRUE(decoded.ok()) << decoded.status().asInt();
    EXPECT_EQ(calls, 2);
    EXPECT_TRUE(lastFlag) << "a nonzero wire byte is a ROS true";
}

TEST_F(CycleStepFaultFixture, OneKilohertzSamplesReachAHundredHertzArrivalsPortInOrder)
{
    hypha::Node&      nodeRef = *this->node;
    const std::string topic = faultTopic("imu_fast");
    auto              publisher = hypha::Node().advertise<Float64>(hypha::Topic{topic});
    ASSERT_TRUE(publisher.enabled());

    hypha::Cycle             cycle("cycle_imu_queued");
    hypha::Arrivals<Float64> samples{
        cycle, hypha::Topic{topic}, hypha::perStep(16), hypha::unsupervised
    };
    cycle.setRate(hypha::Hz(100.0));
    int                 calls = 0;
    std::vector<double> received;
    const auto          started = cycle.start(nodeRef, ImuStep{&calls, &samples, &received});
    ASSERT_TRUE(started.has_value()) << started.error();

    for (int batch = 0; batch < 2; ++batch) {
        const int first = batch * 10;
        for (int index = first; index < first + 10; ++index) {
            ASSERT_TRUE(publisher.publish(Float64{static_cast<double>(index)}));
            const bool lastOfBatch = index == first + 9;
            if (!lastOfBatch) {
                hypha_test::advance(1_ms);
                const hypha::RunResult drain = nodeRef.runFor(1_ms);
                ASSERT_TRUE(drain.ok()) << drain.status().asInt();
            }
        }
        const hypha::RunResult drain = nodeRef.runFor(1_ms);
        ASSERT_TRUE(drain.ok()) << drain.status().asInt();
        EXPECT_EQ(calls, batch);

        hypha_test::advance(1_ms);
        const hypha::RunResult run = nodeRef.runFor(1_ms);
        ASSERT_TRUE(run.ok()) << run.status().asInt();
        EXPECT_EQ(calls, batch + 1);
    }

    ASSERT_EQ(received.size(), 20U);
    for (size_t index = 0; index < received.size(); ++index) {
        EXPECT_DOUBLE_EQ(received[index], static_cast<double>(index));
    }
}
