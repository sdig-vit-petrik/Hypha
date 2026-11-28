// clear() in a step (#213): an input forgets its value and pending samples, a Transforms
// port its dynamic transforms, when the step returns. Host time, then a StepHarness.

#include "hypha/cycle_node.h"
#include "hypha/cycle_transforms.h"
#include "hypha/hypha.h"
#include "hypha/testing.h"

#include "test_types.h"

#include <geometry_msgs/TransformStamped.h>
#include <gtest/gtest.h>
#include <std_msgs/Float64.h>
#include <tf2_msgs/TFMessage.h>

#include <chrono>
#include <cstddef>
#include <cstdint>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

using Float64 = std_msgs::Float64;
using namespace hypha::literals;

namespace {

std::string topicFor(const std::string& name)
{
    return hypha_test::testTopic("/" + name);
}

hypha::TransformTopics clearTopics()
{
    return hypha::TransformTopics{
        .tf = topicFor("clear_tf"), .tfStatic = topicFor("clear_tf_static")
    };
}

geometry_msgs::TransformStamped
transform(const std::string& parent, const std::string& child, double x)
{
    geometry_msgs::TransformStamped item{};
    item.header.frame_id = parent;
    item.header.stamp.secs = 2;
    item.child_frame_id = child;
    item.transform.translation.x = x;
    item.transform.rotation.w = 1.0;
    return item;
}

tf2_msgs::TFMessage message(const geometry_msgs::TransformStamped& item)
{
    tf2_msgs::TFMessage out{};
    out.transforms.push_back(item);
    return out;
}

/// What one step saw.
struct Seen
{
    bool                  has{false};
    std::optional<double> value;
    hypha::InputVerdict   latest{hypha::InputVerdict::NeverArrived};
    size_t                marks{0};
    hypha::InputVerdict   marksVerdict{hypha::InputVerdict::NeverArrived};
    bool                  dynamicFound{false};
    bool                  staticFound{false};
};

/// A node whose step clears every input and its transforms when asked to.
class Resetter final : public hypha::CycleNode
{
  public:
    Resetter()
        : speed(*this, hypha::Topic{topicFor("clear_speed")}, hypha::unsupervised)
        , marks(*this, hypha::Topic{topicFor("clear_marks")}, hypha::unsupervised)
        , tf(*this, 10_s, clearTopics())
    {
        this->setRate(hypha::Hz(200.0));
    }

    hypha::Latest<Float64>   speed;
    hypha::Arrivals<Float64> marks;
    hypha::Transforms        tf;
    bool                     clearNext{false};
    std::optional<size_t>    clearedAt;
    bool                     readAfterClear{false};
    std::vector<Seen>        seen;

    void step(const hypha::CycleTime&) override
    {
        Seen now{};
        now.has = this->speed.has();
        if (this->speed.usable()) {
            now.value = this->speed->data;
        }
        now.latest = this->speed.verdict();
        now.marks = this->marks.samples().size();
        now.marksVerdict = this->marks.verdict();
        now.dynamicFound = this->tf.lookupLatest("world", "base").has_value();
        now.staticFound = this->tf.lookupLatest("base", "lidar").has_value();
        this->seen.push_back(now);
        if (this->clearNext) {
            this->clearNext = false;
            this->speed.clear();
            this->marks.clear();
            this->tf.clear();
            // The rest of the step still reads what it captured.
            this->readAfterClear =
                this->speed.has() && this->tf.lookupLatest("world", "base").has_value();
            this->clearedAt = this->seen.size() - 1;
        }
    }
};

template <typename Done>
bool runUntil(hypha::Node& node, Done done)
{
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
    while (!done()) {
        if (std::chrono::steady_clock::now() >= deadline || !node.runFor(5_ms).ok()) {
            return false;
        }
    }
    return true;
}

}  // namespace

TEST(CycleClear, AClearedNodeSeesOnlyLaterSamplesAndKeepsItsStaticTransforms)
{
    hypha::Node node("cycle_clear_host");
    Resetter    cycle;
    EXPECT_THROW(cycle.speed.clear(), std::logic_error) << "clear() is for a step";
    EXPECT_THROW(cycle.tf.clear(), std::logic_error);
    const auto started = cycle.start(node);
    ASSERT_TRUE(started.has_value()) << started.error();

    hypha::Node sender;
    auto        speed = sender.advertise<Float64>(hypha::Topic{topicFor("clear_speed")});
    auto        marks = sender.advertise<Float64>(hypha::Topic{topicFor("clear_marks")});
    auto        tf = sender.advertise<tf2_msgs::TFMessage>(hypha::Topic{clearTopics().tf});
    auto        tfStatic =
        sender.advertise<tf2_msgs::TFMessage>(hypha::Topic{clearTopics().tfStatic}, hypha::latched);
    ASSERT_TRUE(tfStatic.publish(message(transform("base", "lidar", 0.5))));
    bool sawMarks = false;
    ASSERT_TRUE(runUntil(node, [&] {
        static_cast<void>(speed.publish(Float64{1.0}));
        static_cast<void>(marks.publish(Float64{1.0}));
        static_cast<void>(tf.publish(message(transform("world", "base", 1.0))));
        const bool ready = !cycle.seen.empty() && cycle.seen.back().value == 1.0
                        && cycle.seen.back().dynamicFound && cycle.seen.back().staticFound;
        sawMarks = sawMarks || (!cycle.seen.empty() && cycle.seen.back().marks > 0);
        return ready && sawMarks;
    }));

    // Nothing is sent any more; once a step has seen no new mark, everything sent arrived.
    ASSERT_TRUE(runUntil(node, [&] {
        return cycle.seen.back().marks == 0;
    }));
    cycle.clearNext = true;
    const size_t before = cycle.seen.size();
    ASSERT_TRUE(runUntil(node, [&] {
        return cycle.seen.size() >= before + 4;
    }));
    ASSERT_TRUE(cycle.clearedAt.has_value());
    EXPECT_TRUE(cycle.readAfterClear);
    for (size_t index = *cycle.clearedAt + 1; index < cycle.seen.size(); ++index) {
        const Seen& after = cycle.seen.at(index);
        EXPECT_FALSE(after.has) << index;
        EXPECT_EQ(after.latest, hypha::InputVerdict::NeverArrived) << index;
        EXPECT_EQ(after.marks, 0U) << index;
        EXPECT_EQ(after.marksVerdict, hypha::InputVerdict::NeverArrived) << index;
        EXPECT_FALSE(after.dynamicFound) << "the dynamic transform is forgotten, " << index;
        EXPECT_TRUE(after.staticFound) << "the static one stays, " << index;
    }
    const auto health = cycle.marks.bindingHealth();
    ASSERT_TRUE(health.has_value());
    EXPECT_EQ(health->cleared_total, 0U) << "a rate step takes every sample it admits";

    // A later sample and a later transform are new.
    ASSERT_TRUE(runUntil(node, [&] {
        static_cast<void>(speed.publish(Float64{2.0}));
        static_cast<void>(tf.publish(message(transform("world", "base", 2.0))));
        return cycle.seen.back().value == 2.0 && cycle.seen.back().dynamicFound;
    }));
    EXPECT_EQ(cycle.seen.back().latest, hypha::InputVerdict::Usable);
}

TEST(CycleClear, AHarnessStepClearsItsInputsAndDynamicTransforms)
{
    Resetter                    cycle;
    hypha::testing::StepHarness harness{cycle};
    harness.set(cycle.speed, Float64{1.0});
    harness.set(cycle.marks, std::vector<Float64>{Float64{1.0}, Float64{2.0}});
    harness.deliver(cycle.tf, {transform("world", "base", 1.0)});
    harness.deliverStatic(cycle.tf, {transform("base", "lidar", 0.5)});
    harness.step(cycle);
    EXPECT_FALSE(harness.cleared(cycle.speed));
    ASSERT_TRUE(cycle.seen.back().dynamicFound);

    cycle.clearNext = true;
    harness.step(cycle);
    EXPECT_TRUE(cycle.readAfterClear) << "the clearing step still reads its capture";
    EXPECT_TRUE(harness.cleared(cycle.speed));
    EXPECT_TRUE(harness.cleared(cycle.marks));
    EXPECT_TRUE(harness.cleared(cycle.tf));

    harness.step(cycle);
    EXPECT_FALSE(harness.cleared(cycle.speed)) << "cleared() tells of the last step";
    const Seen& after = cycle.seen.back();
    EXPECT_FALSE(after.has);
    EXPECT_EQ(after.latest, hypha::InputVerdict::NeverArrived);
    EXPECT_EQ(after.marksVerdict, hypha::InputVerdict::NeverArrived);
    EXPECT_FALSE(after.dynamicFound);
    EXPECT_TRUE(after.staticFound);

    harness.set(cycle.speed, Float64{2.0});
    harness.deliver(cycle.tf, {transform("world", "base", 2.0)});
    harness.step(cycle);
    EXPECT_EQ(cycle.seen.back().value, 2.0);
    EXPECT_TRUE(cycle.seen.back().dynamicFound);
}
