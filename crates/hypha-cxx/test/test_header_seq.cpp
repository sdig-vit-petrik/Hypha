#include "hypha/cycle_node.h"

#include "test_types.h"

#include <gtest/gtest.h>

#include <filesystem>
#include <fstream>
#include <optional>
#include <string>
#include <vector>

using hypha_test::StampedReading;
using hypha_test::testTopic;
using namespace hypha::literals;

namespace hypha_test {

/// A header that is not the first field: the seq lands after `count` and `name`.
struct LateHeader
{
    uint32_t    count{0};
    std::string name;
    Header      header;
};

}  // namespace hypha_test

namespace ros::serialization {

template <>
struct Serializer<hypha_test::LateHeader>
{
    template <typename Stream, typename T>
    inline static void allInOne(Stream& stream, T m)
    {
        stream.next(m.count);
        stream.next(m.name);
        stream.next(m.header);
    }

    ROS_DECLARE_ALLINONE_SERIALIZER
};

}  // namespace ros::serialization

namespace {

class HeaderSeqFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("header_seq_test");
    }

    std::optional<hypha::Node> node;
};

/// Sends one StampedReading per step, with a seq the output must replace.
class StampedSender final : public hypha::CycleNode
{
  public:
    hypha::Output<hypha_test::StampedReading> output{*this, "out"};
    hypha_test::StampedReading                message{};

    void step(const hypha::CycleTime&) override
    {
        this->message.header.seq = 999;
        this->message.value += 1;
        this->output.send(this->message);
    }
};

std::vector<uint32_t>* collectedSeqs = nullptr;

void collectSeq(const hypha::MessagePtr<StampedReading>& message)
{
    collectedSeqs->push_back(message->header.seq);
}

}  // namespace

TEST_F(HeaderSeqFixture, AutomaticallyAssignsMonotonicSequenceNumbers)
{
    const std::string topic = testTopic("/stamped_readings");
    auto              pub = hypha::Node().advertise<StampedReading>(hypha::Topic{topic});
    ASSERT_TRUE(pub.enabled());

    std::vector<uint32_t> receivedSeqs;
    auto                  sub = hypha::Node().subscribe<StampedReading>(
        hypha::Topic{topic}, [&](const hypha::MessagePtr<StampedReading>& msg) {
            receivedSeqs.push_back(msg->header.seq);
        }
    );
    ASSERT_TRUE(sub.enabled());

    StampedReading m1;
    m1.header.seq = 999;
    m1.value = 10;
    ASSERT_TRUE(pub.publish(m1));
    EXPECT_EQ(m1.header.seq, 999U);

    StampedReading m2;
    m2.value = 20;
    ASSERT_TRUE(pub.publish(m2));
    EXPECT_EQ(m2.header.seq, 0U);

    StampedReading m3;
    m3.value = 30;
    ASSERT_TRUE(pub.publish(m3));
    EXPECT_EQ(m3.header.seq, 0U);

    (void)node->runFor(hypha::Duration::fromMillis(1));

    ASSERT_EQ(receivedSeqs.size(), 3U);
    EXPECT_EQ(receivedSeqs[0], 0U);
    EXPECT_EQ(receivedSeqs[1], 1U);
    EXPECT_EQ(receivedSeqs[2], 2U);
}

TEST_F(HeaderSeqFixture, CopiesOfOnePublisherShareOneSequence)
{
    const std::string topic = testTopic("/stamped_copies");
    auto              first = hypha::Node().advertise<StampedReading>(hypha::Topic{topic});
    auto              second = first;

    std::vector<uint32_t> receivedSeqs;
    collectedSeqs = &receivedSeqs;
    auto sub = hypha::Node().subscribe<StampedReading>(hypha::Topic{topic}, collectSeq);

    ASSERT_TRUE(first.publish(StampedReading{}));
    ASSERT_TRUE(second.publish(StampedReading{}));
    ASSERT_TRUE(first.publish(StampedReading{}));
    ASSERT_TRUE(hypha_test::runUntil(*node, [&] {
        return receivedSeqs.size() >= 3U;
    }));
    EXPECT_EQ(receivedSeqs, (std::vector<uint32_t>{0U, 1U, 2U}));
    collectedSeqs = nullptr;
}

TEST(HeaderSeqOutput, AnOutputStampsAMonotonicSequence)
{
    const std::string     topic = testTopic("/stamped_output");
    std::filesystem::path path =
        std::filesystem::temp_directory_path() / "hypha_header_seq_output.toml";
    {
        std::ofstream file(path);
        file << "[ports.outputs.out]\ntopic='" << topic << "'\n";
    }
    ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
    {
        hypha::Node           node("header_seq_output", path.string());
        std::vector<uint32_t> receivedSeqs;
        collectedSeqs = &receivedSeqs;
        auto sub = hypha::Node().subscribe<StampedReading>(hypha::Topic{topic}, collectSeq);

        StampedSender sender;
        sender.setRate(hypha::Hz(100.0));
        ASSERT_TRUE(sender.start(node).has_value());
        for (size_t step = 1; step <= 3; ++step) {
            hypha_test::advance(10_ms);
            ASSERT_TRUE(hypha_test::runUntil(node, [&] {
                return receivedSeqs.size() >= step;
            }));
        }
        EXPECT_EQ(receivedSeqs, (std::vector<uint32_t>{0U, 1U, 2U}));
        EXPECT_EQ(sender.message.header.seq, 999U) << "send() does not change the message";
        collectedSeqs = nullptr;
    }
    std::filesystem::remove(path);
}

TEST(HeaderSeqEncoding, TheSeqIsWrittenWhereverTheHeaderSits)
{
    hypha_test::LateHeader message;
    message.count = 7;
    message.name = "abc";
    message.header.seq = 5;
    std::vector<uint8_t> bytes(hypha::codec::encodedLength(message));
    ASSERT_TRUE(hypha::codec::detail::encodeWithSeq(message, bytes.data(), bytes.size(), 42U));

    hypha_test::LateHeader decoded;
    ASSERT_TRUE(hypha::codec::decode(bytes, decoded));
    EXPECT_EQ(decoded.count, 7U);
    EXPECT_EQ(decoded.name, "abc");
    EXPECT_EQ(decoded.header.seq, 42U);
    EXPECT_EQ(message.header.seq, 5U);
}
