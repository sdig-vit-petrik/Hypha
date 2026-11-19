// visitArrivalsInOrder(): a stable chronological merge of several Arrivals batches.

#include "hypha/cycle_node.h"
#include "hypha/testing.h"
#include <hypha_msgs/std_msgs/Float64.h>
#include <hypha_msgs/std_msgs/Int64.h>

#include <gtest/gtest.h>

#include <cstdint>
#include <optional>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace {

std_msgs::Float64 floatAt(double value)
{
    std_msgs::Float64 message;
    message.data = value;
    return message;
}

std_msgs::Int64 intAt(int64_t value)
{
    std_msgs::Int64 message;
    message.data = value;
    return message;
}

/// The stamp of a test message: its value in whole units.
struct StampOf
{
    int64_t operator()(const std_msgs::Float64& message) const
    {
        return static_cast<int64_t>(message.data);
    }

    int64_t operator()(const std_msgs::Int64& message) const
    {
        return message.data;
    }
};

using Visited = std::vector<std::pair<size_t, int64_t>>;

Visited merge(
    const hypha::Arrivals<std_msgs::Float64>& first, const hypha::Arrivals<std_msgs::Int64>& second
)
{
    Visited visited;
    hypha::visitArrivalsInOrder(
        StampOf{},
        [&visited](auto port, const auto& sample) {
            visited.emplace_back(port, StampOf{}(sample));
        },
        first,
        second
    );
    return visited;
}

class ArrivalsMerge : public ::testing::Test
{
  protected:
    hypha::Cycle                       cycle{"arrivals_merge_test"};
    hypha::Arrivals<std_msgs::Float64> first{cycle, "first"};
    hypha::Arrivals<std_msgs::Int64>   second{cycle, "second"};
    hypha::testing::StepHarness        harness{cycle};

    void
    stage(std::vector<std_msgs::Float64> firstSamples, std::vector<std_msgs::Int64> secondSamples)
    {
        this->harness.set(this->first, std::move(firstSamples));
        this->harness.set(this->second, std::move(secondSamples));
    }
};

TEST_F(ArrivalsMerge, SortedBatchesMergeByStampThenPortThenBatchOrder)
{
    this->stage(
        {floatAt(1.0), floatAt(2.0), floatAt(2.0), floatAt(5.0)}, {intAt(2), intAt(3), intAt(5)}
    );

    EXPECT_EQ(
        merge(this->first, this->second),
        (Visited{{0, 1}, {0, 2}, {0, 2}, {1, 2}, {1, 3}, {0, 5}, {1, 5}})
    );
}

TEST_F(ArrivalsMerge, ABatchOutOfStampOrderIsSorted)
{
    this->stage({floatAt(3.0), floatAt(1.0), floatAt(1.0)}, {intAt(2), intAt(1)});

    EXPECT_EQ(merge(this->first, this->second), (Visited{{0, 1}, {0, 1}, {1, 1}, {1, 2}, {0, 3}}));
}

TEST_F(ArrivalsMerge, EqualStampsInAnUnsortedBatchKeepTheirBatchOrder)
{
    this->stage({floatAt(2.0), floatAt(1.0), floatAt(2.0)}, {});
    std::vector<size_t> order;
    hypha::visitArrivalsInOrder(
        StampOf{},
        [this, &order](auto port, const auto& sample) {
            if constexpr (decltype(port)::value == 0) {
                order.push_back(static_cast<size_t>(&sample - this->first.samples().data()));
            }
        },
        this->first,
        this->second
    );

    EXPECT_EQ(order, (std::vector<size_t>{1, 0, 2}));
}

TEST_F(ArrivalsMerge, EmptyBatchesVisitNothing)
{
    this->stage({}, {});

    EXPECT_TRUE(merge(this->first, this->second).empty());
}

TEST_F(ArrivalsMerge, TheVisitorGetsThePortAsACompileTimeIndex)
{
    this->stage({floatAt(1.0)}, {intAt(2)});
    std::vector<std::string> kinds;
    hypha::visitArrivalsInOrder(
        StampOf{},
        [&kinds](auto port, const auto& sample) {
            if constexpr (decltype(port)::value == 0) {
                kinds.push_back("float " + std::to_string(sample.data));
            } else {
                kinds.push_back("int " + std::to_string(sample.data));
            }
        },
        this->first,
        this->second
    );

    EXPECT_EQ(kinds, (std::vector<std::string>{"float 1.000000", "int 2"}));
}

TEST_F(ArrivalsMerge, AnOptionalTimeStampOrdersAMissingStampFirst)
{
    this->stage({floatAt(2.0), floatAt(-1.0)}, {intAt(1)});
    const auto stampOf = [](const auto& message) -> std::optional<hypha::Time> {
        if (message.data < 0) {
            return std::nullopt;
        }
        return hypha::Time::fromNanos(static_cast<int64_t>(message.data));
    };
    Visited visited;
    hypha::visitArrivalsInOrder(
        stampOf,
        [&visited](size_t port, const auto& sample) {
            visited.emplace_back(port, static_cast<int64_t>(sample.data));
        },
        this->first,
        this->second
    );

    EXPECT_EQ(visited, (Visited{{0, -1}, {1, 1}, {0, 2}}));
}

TEST_F(ArrivalsMerge, AnExceptionFromTheVisitorStopsTheMerge)
{
    this->stage({floatAt(1.0), floatAt(2.0)}, {intAt(3)});
    size_t     calls = 0;
    const auto failing = [&calls](size_t, const auto&) {
        calls += 1;
        throw std::runtime_error("stop");
    };

    EXPECT_THROW(
        hypha::visitArrivalsInOrder(StampOf{}, failing, this->first, this->second),
        std::runtime_error
    );
    EXPECT_EQ(calls, 1U);
}

}  // namespace
