// A cycle reads its large inputs in place (Arrivals<View<M>>, Latest<View<M>>) and writes
// its large outputs in place (Output<M>::loan): the bytes on the wire equal those of
// send(const M&), a view kept past its step owns a copy of its bytes, and a loan lives no
// longer than its step. Host time, a triggered cycle.
#include "hypha/cycle_node.h"
#include "hypha/hypha.h"
#include "hypha/testing.h"

#include "test_types.h"

#include <gtest/gtest.h>
#include <sensor_msgs/PointCloud2.h>
#include <sensor_msgs/point_cloud2_iterator.h>
#include <std_msgs/Float64.h>

#include <chrono>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <optional>
#include <random>
#include <stdexcept>
#include <string>
#include <vector>

using sensor_msgs::PointCloud2;
using Float64 = std_msgs::Float64;
using namespace hypha::literals;

namespace {

std::string topicFor(const std::string& name)
{
    return hypha_test::testTopic("/" + name);
}

PointCloud2 cloud(uint32_t points, uint8_t seed)
{
    PointCloud2 message;
    message.header.frame_id = "lidar_" + std::to_string(seed);
    message.header.stamp.secs = seed;
    message.height = 1;
    message.width = points;
    message.fields = {
        {.name = "x", .offset = 0, .datatype = sensor_msgs::PointField::FLOAT32, .count = 1},
        {.name = "intensity", .offset = 4, .datatype = sensor_msgs::PointField::UINT8, .count = 1},
    };
    message.point_step = 5;
    message.row_step = points * 5;
    message.is_dense = (seed % 2) == 0;
    message.data.resize(static_cast<size_t>(points) * 5);
    std::mt19937 random(seed);
    for (uint8_t& byte : message.data) {
        byte = static_cast<uint8_t>(random());
    }
    return message;
}

/// @p message without its data and `header.seq`: the head a filter loans with.
PointCloud2 headOf(const hypha::View<PointCloud2>& view)
{
    PointCloud2 head = hypha::toMessage(view);
    head.data.clear();
    head.header.seq = 0;
    return head;
}

/// Echoes each input cloud through a loan, with an Float64 sent while the loan is held.
class Echo final : public hypha::CycleNode
{
  public:
    Arrivals<hypha::View<PointCloud2>> input{*this, "cloud", perStep(3)};
    Output<PointCloud2>                output{*this, "out", perStep(3)};
    Output<Float64>                    count{*this, "count", perStep(3)};
    std::vector<PointCloud2>           seen;
    hypha::ViewPtr<PointCloud2>        kept;
    bool                               keep{false};
    size_t                             points{0};

    Echo()
    {
        this->setTrigger(this->input);
    }

    void step(const hypha::CycleTime&) override
    {
        for (const hypha::ViewPtr<PointCloud2>& sample : this->input.samples()) {
            this->seen.push_back(hypha::toMessage(*sample));
            // The vendored iterator reads a view only with #298 (aligned reads); here, the copy.
            sensor_msgs::PointCloud2ConstIterator<uint8_t> intensity(
                this->seen.back(), "intensity"
            );
            for (; intensity != intensity.end(); ++intensity) {
                ++this->points;
            }
            if (this->keep) {
                this->kept = sample;
            }
            // A capacity larger than the cloud: the send shrinks the loan to the bytes used.
            const size_t used = sample->data.size();
            auto         loan = this->output.loan(headOf(*sample), used + 64);
            ASSERT_TRUE(loan.has_value()) << loan.error().name();
            std::memcpy(loan->data().data(), sample->data.data(), used);
            this->count.send(Float64{static_cast<double>(this->seen.size())});
            this->output.send(std::move(*loan), used);
        }
    }
};

class CycleViewFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        // Host time: an earlier test of the program may have left simulated time on.
        static_cast<void>(hypha::disableSimTime());
        this->path = std::filesystem::temp_directory_path() / "hypha_cycle_view_test.toml";
        std::ofstream output(this->path);
        output << "[ports.inputs.cloud]\ntopic='" << topicFor("cloud") << "'\nsilence_ms = 10000\n"
               << "[ports.outputs.out]\ntopic='" << topicFor("out") << "'\n"
               << "[ports.outputs.count]\ntopic='" << topicFor("count") << "'\n";
        output.close();
        this->node.emplace("cycle_view_test", this->path.string());
    }

    void TearDown() override
    {
        this->node.reset();
        std::filesystem::remove(this->path);
    }

    template <typename Done>
    bool runUntil(Done done)
    {
        const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
        while (!done()) {
            if (std::chrono::steady_clock::now() >= deadline || !this->node->runFor(1_ms).ok()) {
                return false;
            }
        }
        return true;
    }

    std::optional<hypha::Node> node;
    std::filesystem::path      path;
};

}  // namespace

TEST_F(CycleViewFixture, AViewInAndALoanOutSendTheBytesOfTheTypedPath)
{
    Echo       echo;
    const auto started = echo.start(*this->node);
    ASSERT_TRUE(started.has_value()) << started.error();
    std::vector<std::vector<uint8_t>> wire;
    auto                              raw = hypha::subscribeRawOfType(
                                                topicFor("out"),
                                                hypha::RawType{
                                                    .dataType = hypha::codec::dataType<PointCloud2>(),
                                                    .md5 = hypha::codec::md5Sum<PointCloud2>()
                                                },
                                                [&wire](const hypha::RawSample& sample) {
                       wire.emplace_back(sample.bytes.begin(), sample.bytes.end());
                                                },
                                                0,
                                                hypha::defaultQos
    )
                                                .value();
    std::vector<double>               counts;
    auto                              countSub = hypha::Node().subscribe<Float64>(
        hypha::Topic{topicFor("count")}, [&counts](const hypha::MessagePtr<Float64>& message) {
            counts.push_back(message->data);
        }
    );
    auto publisher = hypha::Node().advertise<PointCloud2>(hypha::Topic{topicFor("cloud")});
    // From an empty cloud to one read in place (above 64 KiB).
    const std::vector<uint32_t> sizes{0, 1, 100, 20000, 30000};
    std::vector<PointCloud2>    sent;
    for (size_t index = 0; index < sizes.size(); ++index) {
        sent.push_back(cloud(sizes[index], static_cast<uint8_t>(index + 1)));
        ASSERT_TRUE(publisher.publish(sent.back()));
        ASSERT_TRUE(this->runUntil([&] {
            return wire.size() > index;
        }));
    }
    ASSERT_EQ(echo.seen.size(), sizes.size());
    for (size_t index = 0; index < sizes.size(); ++index) {
        PointCloud2 expected = sent[index];
        expected.header.seq = static_cast<uint32_t>(index);
        EXPECT_EQ(echo.seen[index], expected) << "the view is the message that was sent";
        EXPECT_EQ(wire[index], hypha::codec::encode(expected))
            << "the loan sends the bytes send(const M&) sends, header.seq included";
    }
    EXPECT_EQ(echo.points, 0U + 1 + 100 + 20000 + 30000) << "the iterator reads the view";
    EXPECT_EQ(counts.size(), sizes.size()) << "a send while a loan is held goes out";
}

TEST_F(CycleViewFixture, AViewKeptPastItsStepOwnsACopyOfItsBytes)
{
    Echo echo;
    echo.keep = true;
    ASSERT_TRUE(echo.start(*this->node).has_value());
    auto       publisher = hypha::Node().advertise<PointCloud2>(hypha::Topic{topicFor("cloud")});
    const auto first = cloud(30000, 3);
    ASSERT_TRUE(publisher.publish(first));
    ASSERT_TRUE(this->runUntil([&] {
        return echo.seen.size() == 1;
    }));
    echo.keep = false;
    for (uint8_t seed = 4; seed < 8; ++seed) {
        ASSERT_TRUE(publisher.publish(cloud(30000, seed)));
        ASSERT_TRUE(this->runUntil([&] {
            return echo.seen.size() == seed - 2U;
        }));
    }
    ASSERT_TRUE(echo.kept);
    PointCloud2 kept = hypha::toMessage(*echo.kept);
    kept.header.seq = 0;
    EXPECT_EQ(kept, first) << "the slot went back; the kept view reads its copy";
}

TEST_F(CycleViewFixture, AHarnessFeedsMessagesToAViewPortAndKeepsTheLoanedSend)
{
    Echo                        echo;
    hypha::testing::StepHarness harness(echo);
    const PointCloud2           input = cloud(10, 9);
    harness.deliver(echo.input, std::vector<PointCloud2>{input}, hypha::Time::fromNanos(0));
    harness.step(echo);
    ASSERT_EQ(echo.seen.size(), 1U);
    EXPECT_EQ(echo.seen.front(), input);
    ASSERT_EQ(harness.sent(echo.output).size(), 1U);
    EXPECT_EQ(harness.sent(echo.output).front(), input) << "a loan sends what send() would";
}

namespace {

/// Holds a loan past its step, and drops one unsent.
class Hoarder final : public hypha::CycleNode
{
  public:
    Output<PointCloud2>                     output{*this, "out"};
    std::optional<hypha::Loan<PointCloud2>> held;
    int                                     steps{0};
    bool                                    refusedLate{false};

    void step(const hypha::CycleTime&) override
    {
        this->steps += 1;
        if (this->steps == 1) {
            {
                auto dropped = this->output.loan(PointCloud2{}, 8);
                ASSERT_TRUE(dropped.has_value()) << dropped.error().name();
            }
            auto kept = this->output.loan(PointCloud2{}, 8);
            ASSERT_TRUE(kept.has_value()) << "a dropped loan gave its count back";
            this->held.emplace(std::move(*kept));
            return;
        }
        if (this->held.has_value()) {
            try {
                this->output.send(std::move(*this->held));
            } catch (const std::logic_error&) {
                this->refusedLate = true;
            }
            this->held.reset();
        }
    }
};

}  // namespace

TEST_F(CycleViewFixture, ALoanLivesNoLongerThanItsStep)
{
    Hoarder hoarder;
    hoarder.setRate(hypha::Hz(200.0));
    ASSERT_TRUE(hoarder.start(*this->node).has_value());
    std::vector<int> received;
    auto             subscriber = hypha::Node().subscribe<PointCloud2>(
        hypha::Topic{topicFor("out")}, [&received](const hypha::MessagePtr<PointCloud2>&) {
            received.push_back(1);
        }
    );
    ASSERT_TRUE(this->runUntil([&] {
        return hoarder.steps >= 3;
    }));
    EXPECT_TRUE(hoarder.refusedLate) << "a loan of an earlier step is refused";
    EXPECT_TRUE(received.empty()) << "neither the dropped nor the kept loan was sent";
}
