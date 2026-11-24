// The Transforms port: a cycle's TF input over /tf and /tf_static. Host time.

#include "hypha/cycle_node.h"
#include "hypha/cycle_transforms.h"
#include "hypha/hypha.h"

#include "test_types.h"

#include <gtest/gtest.h>
#include <tf2_msgs/TFMessage.h>

#include <chrono>
#include <expected>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

using namespace hypha::literals;

namespace {

/// TF topics of the running test, so parallel test processes stay apart.
hypha::TransformTopics testTopics()
{
    return hypha::TransformTopics{
        .tf = hypha_test::testTopic("/tf"), .tfStatic = hypha_test::testTopic("/tf_static")
    };
}

tf2_msgs::TFMessage one(const std::string& parent, const std::string& child, double x)
{
    geometry_msgs::TransformStamped item{};
    item.header.frame_id = parent;
    item.header.stamp.secs = 2;
    item.child_frame_id = child;
    item.transform.translation.x = x;
    item.transform.rotation.w = 1.0;
    tf2_msgs::TFMessage message{};
    message.transforms.push_back(item);
    return message;
}

using Lookup = std::expected<geometry_msgs::TransformStamped, hypha::TransformError>;

/// Looks up each query in every step and keeps the answers of the last one.
class Looker final : public hypha::CycleNode
{
  public:
    explicit Looker(std::vector<std::pair<std::string, std::string>> asked)
        : tf(*this, 10_s, testTopics()), queries(std::move(asked))
    {
        this->setRate(hypha::Hz(100.0));
    }

    hypha::Transforms                                tf;
    std::vector<std::pair<std::string, std::string>> queries;
    std::vector<Lookup>                              answers;
    int                                              steps{0};

    void step(const hypha::CycleTime&) override
    {
        this->answers.clear();
        for (const auto& [target, source] : this->queries) {
            this->answers.push_back(this->tf.lookupLatest(target, source));
        }
        ++this->steps;
    }

    bool allFound() const
    {
        if (this->answers.empty()) {
            return false;
        }
        for (const Lookup& answer : this->answers) {
            if (!answer.has_value()) {
                return false;
            }
        }
        return true;
    }
};

/// Runs the node in 5 ms slices until @p done holds, or a real-time bound passes.
template <typename Done>
bool runUntil(hypha::Node& node, Done done)
{
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
    while (!done()) {
        if (std::chrono::steady_clock::now() >= deadline) {
            return false;
        }
        if (!node.runFor(5_ms).ok()) {
            return false;
        }
    }
    return true;
}

}  // namespace

TEST(TfPort, FindsAPublishedTransform)
{
    hypha::Node node("tf_port_found");
    Looker      cycle({{"world", "base"}});
    const auto  started = cycle.start(node);
    ASSERT_TRUE(started.has_value()) << started.error();
    auto publisher = hypha::Node().advertise<tf2_msgs::TFMessage>(hypha::Topic{testTopics().tf});
    ASSERT_TRUE(runUntil(node, [&] {
        static_cast<void>(publisher.publish(one("world", "base", 1.5)));
        return cycle.allFound();
    }));
    const geometry_msgs::TransformStamped& found = *cycle.answers.front();
    EXPECT_EQ(found.header.frame_id, "world");
    EXPECT_EQ(found.child_frame_id, "base");
    EXPECT_EQ(found.header.stamp.secs, 2U);
    EXPECT_EQ(found.transform.translation.x, 1.5);
    EXPECT_EQ(found.transform.rotation.w, 1.0);
}

TEST(TfPort, ReportsUnknownFrames)
{
    hypha::Node node("tf_port_unknown");
    Looker      cycle({{"world", "nowhere"}});
    ASSERT_TRUE(cycle.start(node));
    auto publisher = hypha::Node().advertise<tf2_msgs::TFMessage>(hypha::Topic{testTopics().tf});
    ASSERT_TRUE(publisher.publish(one("world", "base", 1.0)));
    ASSERT_TRUE(runUntil(node, [&] {
        return cycle.steps >= 5;
    }));
    ASSERT_EQ(cycle.answers.size(), 1U);
    ASSERT_FALSE(cycle.answers.front().has_value());
    EXPECT_EQ(cycle.answers.front().error(), hypha::TransformError::UnknownFrame);
}

TEST(TfPort, TwoStaticPublishersReachALateSubscriber)
{
    hypha::Node node("tf_port_static");
    auto        first = hypha::Node().advertise<tf2_msgs::TFMessage>(
        hypha::Topic{testTopics().tfStatic}, hypha::latched
    );
    auto second = hypha::Node().advertise<tf2_msgs::TFMessage>(
        hypha::Topic{testTopics().tfStatic}, hypha::latched
    );
    ASSERT_TRUE(first.publish(one("base", "camera", 0.1)));
    ASSERT_TRUE(second.publish(one("base", "lidar", 0.2)));

    // The cycle subscribes after both publishers sent their only sample.
    Looker     cycle({{"base", "camera"}, {"base", "lidar"}});
    const auto started = cycle.start(node);
    ASSERT_TRUE(started.has_value()) << started.error();
    ASSERT_TRUE(runUntil(node, [&] {
        return cycle.allFound();
    }));
    EXPECT_EQ(cycle.answers.at(0)->transform.translation.x, 0.1);
    EXPECT_EQ(cycle.answers.at(1)->transform.translation.x, 0.2);
    EXPECT_EQ(cycle.answers.at(0)->header.stamp.secs, 0U) << "a static chain holds at time 0";
}

TEST(TfPort, LookupOutsideAStepThrows)
{
    hypha::Node node("tf_port_outside");
    Looker      cycle({});
    EXPECT_THROW(
        static_cast<void>(cycle.tf.lookup("a", "b", hypha::Time::fromNanos(0))), std::logic_error
    );
    ASSERT_TRUE(cycle.start(node));
    ASSERT_TRUE(node.runFor(30_ms).ok());
    EXPECT_GT(cycle.steps, 0);
    EXPECT_THROW(
        static_cast<void>(cycle.tf.lookup("a", "b", hypha::Time::fromNanos(0))), std::logic_error
    );
}

namespace {

/// Looks up a static chain at a time past what a RosTime holds (2106).
class FarLooker final : public hypha::CycleNode
{
  public:
    FarLooker() : tf(*this, 10_s, testTopics())
    {
        this->setRate(hypha::Hz(100.0));
    }

    hypha::Transforms     tf;
    std::optional<Lookup> answer;
    bool                  threw{false};

    void step(const hypha::CycleTime&) override
    {
        try {
            this->answer = this->tf.lookup("base", "camera", hypha::Time::fromSeconds(5.0e9));
        } catch (const std::out_of_range&) {
            this->threw = true;
        }
    }
};

}  // namespace

TEST(TfPort, AStampARosTimeCannotHoldThrows)
{
    hypha::Node node("tf_port_far");
    auto        fixed = hypha::Node().advertise<tf2_msgs::TFMessage>(
        hypha::Topic{testTopics().tfStatic}, hypha::latched
    );
    ASSERT_TRUE(fixed.publish(one("base", "camera", 0.1)));
    FarLooker  cycle;
    const auto started = cycle.start(node);
    ASSERT_TRUE(started.has_value()) << started.error();
    ASSERT_TRUE(runUntil(node, [&] {
        return cycle.threw || (cycle.answer.has_value() && cycle.answer->has_value());
    }));
    EXPECT_TRUE(cycle.threw) << "found with stamp "
                             << (cycle.answer ? (*cycle.answer)->header.stamp.secs : 0U);
}
