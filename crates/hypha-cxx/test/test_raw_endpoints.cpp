// Raw endpoints on the Rust core: bytes, identity, transport age and schema lookup.

#include "test_types.h"

#include <gtest/gtest.h>

#include <chrono>
#include <cstdint>
#include <optional>
#include <string>
#include <thread>
#include <vector>

using hypha_test::testTopic;

namespace {

std::vector<uint8_t> bytesOf(const std::string& text)
{
    return std::vector<uint8_t>(text.begin(), text.end());
}

/// Named collector: a subscribe callback outlives the statement that makes it.
struct Collector
{
    std::vector<std::string>* seen;

    void operator()(const hypha::RawSample& sample) const
    {
        this->seen->emplace_back(sample.bytes.begin(), sample.bytes.end());
    }
};

/// Records the transport age the library reported for each delivery.
struct AgeCollector
{
    std::vector<int64_t>* ages;

    void operator()(const hypha::RawSample& sample) const
    {
        this->ages->push_back(sample.transportAge.nanos());
    }
};

struct SelfRemovingCollector
{
    std::optional<hypha::RawSubscriber>* subscriber;
    size_t*                              deliveries;

    void operator()(const hypha::RawSample&) const
    {
        *this->deliveries += 1;
        this->subscriber->reset();
    }
};

hypha::RawPublisher bytesPublisher(const std::string& topic)
{
    return hypha::rawPublisher(
               topic,
               {.dataType = "t/Bytes", .md5 = "00000000000000000000000000000000", .definition = ""}
    )
        .value();
}

class RawEndpoints : public ::testing::Test
{
  protected:
    hypha::Node node{"test_raw_endpoints"};
};

}  // namespace

// A raw subscriber that declares a policy opens a topic before its publisher does, and
// the raw publisher, which declares none, adopts it.
TEST_F(RawEndpoints, ARawSampleCarriesItsTransportAge)
{
    constexpr int64_t QUEUED_NANOS = 30'000'000;
    constexpr int64_t IMPLAUSIBLE_AGE_NANOS = 10'000'000'000;
    const std::string topic = testTopic("/raw/transport_age");

    std::vector<int64_t> ages;
    auto                 subscriber =
        hypha::subscribeRawOfType(topic, "t/Bytes", AgeCollector{&ages}, 0, hypha::defaultQos)
            .value();
    auto publisher = bytesPublisher(topic);

    ASSERT_EQ(publisher.publish(bytesOf("aged")), hypha::Status::Ok);
    std::this_thread::sleep_for(std::chrono::nanoseconds(QUEUED_NANOS));
    ASSERT_TRUE(hypha_test::runUntil(this->node, [&] {
        return !ages.empty();
    }));
    ASSERT_EQ(ages.size(), 1U);
    EXPECT_GE(ages[0], QUEUED_NANOS) << "the age covers the time in the queue";
    EXPECT_LT(ages[0], IMPLAUSIBLE_AGE_NANOS) << "the age is a delay, not a raw clock value";
}

TEST_F(RawEndpoints, TheDefinitionIsResolvable)
{
    const std::string          topic = testTopic("/raw/schema");
    hypha::RawPublisher        publisher = hypha::rawPublisher(
                                               topic,
                                               {.dataType = "std_msgs/Bool",
                                                .md5 = "8b94c1b53db61fb6aed406028ad6332a",
                                                .definition = "bool data\n"}
    )
                                               .value();
    std::optional<std::string> found;
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
    while (!found.has_value() && std::chrono::steady_clock::now() < deadline) {
        found = hypha::lookupDefinition(topic, "std_msgs/Bool", "8b94c1b53db61fb6aed406028ad6332a");
        if (!found.has_value()) {
            std::this_thread::sleep_for(std::chrono::milliseconds(25));
        }
    }
    EXPECT_EQ(found.value_or(""), "bool data\n");
}

TEST_F(RawEndpoints, EveryByteValueAndAnEmptyPayloadSurvive)
{
    const std::string        topic = testTopic("/raw/bytes");
    std::vector<std::string> seen;
    auto                     subscriber =
        hypha::subscribeRawOfType(topic, "t/Bytes", Collector{&seen}, 0, hypha::defaultQos).value();
    auto publisher = bytesPublisher(topic);

    std::vector<uint8_t> payload(256);
    for (size_t index = 0; index < payload.size(); ++index) {
        payload[index] = static_cast<uint8_t>(index);
    }
    ASSERT_EQ(publisher.publish(payload), hypha::Status::Ok);
    ASSERT_EQ(publisher.publish({}), hypha::Status::Ok);
    ASSERT_TRUE(hypha_test::runUntil(this->node, [&] {
        return seen.size() >= 2U;
    }));
    EXPECT_EQ(seen[0], std::string(payload.begin(), payload.end()));
    EXPECT_TRUE(seen[1].empty()) << "an empty payload arrives as an empty sample";
}

TEST_F(RawEndpoints, ACallbackMayDestroyItsOwnSubscriber)
{
    const std::string                   topic = testTopic("/raw/self_remove");
    size_t                              deliveries = 0;
    std::optional<hypha::RawSubscriber> subscriber;
    subscriber.emplace(
        hypha::subscribeRawOfType(
            topic, "t/Bytes", SelfRemovingCollector{&subscriber, &deliveries}, 2, hypha::defaultQos
        )
            .value()
    );
    auto publisher = bytesPublisher(topic);

    ASSERT_EQ(publisher.publish(bytesOf("first")), hypha::Status::Ok);
    ASSERT_EQ(publisher.publish(bytesOf("second")), hypha::Status::Ok);
    (void)this->node.runFor(hypha::Duration::fromMillis(1));

    EXPECT_EQ(deliveries, 1U);
    EXPECT_FALSE(subscriber.has_value());
}
