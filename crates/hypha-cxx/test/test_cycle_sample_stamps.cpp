// A cycle input shows where its samples came from: the send time, the publisher and the
// publisher's send sequence of each frame, read from the frame header. One started cycle
// on the bus, in host time.

#include "hypha/cycle_node.h"
#include "hypha/hypha.h"
#include <hypha_msgs/std_msgs/Float64.h>
#include <hypha_msgs/std_msgs/UInt32.h>

#include "test_types.h"

#include <gtest/gtest.h>

#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

using Float64 = std_msgs::Float64;
using UInt32 = std_msgs::UInt32;

namespace {

/// The stamp of one sample as a step read it.
struct Stamp
{
    double   value{0.0};
    int64_t  sentNanos{0};
    uint64_t sequence{0};
    uint64_t publisher{0};
};

class Watcher final : public hypha::CycleNode
{
  public:
    Latest<Float64>    level{*this, "level"};
    Arrivals<UInt32>   marks{*this, "marks"};
    bool               emptyBeforeAValue{true};
    std::vector<Stamp> levels;
    std::vector<Stamp> markStamps;
    bool               pastTheEndThrows{false};

    void step(const hypha::CycleTime&) override
    {
        if (!this->level.has()) {
            const bool allEmpty = !this->level.sentAt().has_value()
                               && !this->level.sequence().has_value()
                               && !this->level.publisherId().has_value();
            this->emptyBeforeAValue = this->emptyBeforeAValue && allEmpty;
        }
        if (this->level.fresh()) {
            this->levels.push_back(
                Stamp{
                    this->level.lastKnown().data,
                    this->level.sentAt().value().nanos(),
                    this->level.sequence().value(),
                    this->level.publisherId().value(),
                }
            );
        }
        const size_t count = this->marks.samples().size();
        for (size_t index = 0; index < count; ++index) {
            this->markStamps.push_back(
                Stamp{
                    static_cast<double>(this->marks.samples()[index].data),
                    this->marks.sentAt(index).nanos(),
                    this->marks.sequence(index),
                    this->marks.publisherId(index),
                }
            );
        }
        try {
            static_cast<void>(this->marks.sentAt(count));
        } catch (const std::out_of_range&) {
            this->pastTheEndThrows = true;
        }
    }
};

const std::string kParameters = "[ports.inputs.level]\n"
                                "topic = '/stamp_level'\n"
                                "silence_ms = 5000\n"
                                "[ports.inputs.marks]\n"
                                "topic = '/stamp_marks'\n"
                                "silence_ms = 5000\n";

}  // namespace

TEST(CycleSampleStamps, AStepReadsTheSendTimeThePublisherAndItsSequenceOfEachSample)
{
    const std::filesystem::path path =
        std::filesystem::temp_directory_path() / "hypha_cycle_sample_stamps.toml";
    {
        std::ofstream output(path);
        output << kParameters;
    }
    {
        hypha::Node               node("cycle_sample_stamps", path.string());
        hypha::Publisher<Float64> levels =
            hypha::Node().advertise<Float64>(hypha::Topic{"/stamp_level"});
        hypha::Publisher<UInt32> marks =
            hypha::Node().advertise<UInt32>(hypha::Topic{"/stamp_marks"});
        // Five frames nobody reads: the publisher's sequence runs ahead of the count of
        // samples the input has taken, so the two cannot be mistaken for each other.
        for (int unread = 0; unread < 5; ++unread) {
            ASSERT_TRUE(levels.publish(Float64{-1.0}));
        }
        Watcher watcher;
        watcher.setRate(hypha::Hz{100.0});
        ASSERT_TRUE(watcher.start(node).has_value());

        // One level at a time, so each becomes the held value of some step.
        for (double value = 1.0; value <= 3.0; value += 1.0) {
            ASSERT_TRUE(levels.publish(Float64{value}));
            const size_t wanted = static_cast<size_t>(value);
            const bool   seen = hypha_test::runUntil(node, [&watcher, wanted]() {
                return watcher.levels.size() >= wanted;
            });
            ASSERT_TRUE(seen) << "level " << value;
        }
        for (uint32_t mark = 1; mark <= 4; ++mark) {
            ASSERT_TRUE(marks.publish(UInt32{mark}));
        }
        const bool allMarks = hypha_test::runUntil(node, [&watcher]() {
            return watcher.markStamps.size() >= 4U;
        });
        ASSERT_TRUE(allMarks);

        EXPECT_TRUE(watcher.emptyBeforeAValue);
        EXPECT_TRUE(watcher.pastTheEndThrows);

        ASSERT_EQ(watcher.levels.size(), 3U);
        EXPECT_GE(watcher.levels.front().sequence, 5U) << "the publisher's own sequence";
        EXPECT_NE(watcher.levels.front().publisher, 0U);
        for (size_t index = 0; index < watcher.levels.size(); ++index) {
            const Stamp& stamp = watcher.levels[index];
            EXPECT_DOUBLE_EQ(stamp.value, static_cast<double>(index + 1));
            EXPECT_GT(stamp.sentNanos, 0);
            EXPECT_EQ(stamp.publisher, watcher.levels.front().publisher);
            // One publisher: each frame follows the one before by one.
            EXPECT_EQ(stamp.sequence, watcher.levels.front().sequence + index);
            if (index > 0) {
                EXPECT_GT(stamp.sentNanos, watcher.levels[index - 1].sentNanos);
            }
        }

        ASSERT_EQ(watcher.markStamps.size(), 4U);
        for (size_t index = 0; index < watcher.markStamps.size(); ++index) {
            const Stamp& stamp = watcher.markStamps[index];
            EXPECT_DOUBLE_EQ(stamp.value, static_cast<double>(index + 1));
            EXPECT_EQ(stamp.publisher, watcher.markStamps.front().publisher);
            EXPECT_EQ(stamp.sequence, watcher.markStamps.front().sequence + index);
            if (index > 0) {
                EXPECT_GE(stamp.sentNanos, watcher.markStamps[index - 1].sentNanos);
            }
        }
        // Two publishers are two senders.
        EXPECT_NE(watcher.levels.front().publisher, watcher.markStamps.front().publisher);
    }
    std::error_code ignored;
    std::filesystem::remove(path, ignored);
}
