#pragma once

#include "hypha/hypha.h"

#include <gtest/gtest.h>

#include <chrono>
#include <cstdint>
#include <expected>
#include <optional>
#include <string>

namespace hypha_test {

struct Reading
{
    int         value{0};
    std::string label;
};

struct Command
{
    double target{0.0};
};

struct Header
{
    uint32_t    seq{0};
    hypha::Time stamp;
    std::string frame_id;
};

struct StampedReading
{
    Header      header;
    int         value{0};
    std::string label;
};

struct ComputeTrajectory
{
    struct Request
    {
        double      max_velocity{0.0};
        std::string goal;
    };

    struct Response
    {
        bool        success{false};
        std::string message;
    };
};

/// @brief Advance simulated time by @p d. The test enabled simulated time in
///        its SetUp, so the advance must succeed.
inline void advance(hypha::Duration d)
{
    EXPECT_TRUE(hypha::advanceSimTime(d));
}

/// @brief Run @p node in short bounded runs until @p done() holds, then return true.
/// A run of one millisecond does not prove that a subscriber or a cycle step has run.
/// wait ends with false after @p limit of host time, or when a run fails.
template <typename Done>
bool runUntil(
    hypha::Node& node, Done&& done, std::chrono::milliseconds limit = std::chrono::seconds(20)
)
{
    const auto deadline = std::chrono::steady_clock::now() + limit;
    while (!done()) {
        if (std::chrono::steady_clock::now() >= deadline) {
            return false;
        }
        if (!node.runFor(hypha::Duration::fromMillis(1)).ok()) {
            return false;
        }
    }
    return true;
}

/// @brief Run @p node until a dispatch sweep has ended at simulated time @p nanos.
///
/// A sweep runs its timers before it ends, so when this returns true every timer has
/// observed @p nanos. Use it after advancing or rewinding simulated time, when the
/// effect of the change is a timer's own decision and not a callback's.
inline bool runUntilSweepAt(
    hypha::Node& node, int64_t nanos, std::chrono::milliseconds limit = std::chrono::seconds(20)
)
{
    return runUntil(
        node,
        [&] {
            const std::optional<uint64_t> sweepEnd = hypha::metric("node.last_sweep_end_nanos");
            return sweepEnd.has_value() && static_cast<int64_t>(*sweepEnd) == nanos;
        },
        limit
    );
}

/// @brief A composed cycle whose step is a method, so a test runs several on one node.
///        A node has one CycleNode, which takes the node's name; this one has its own.
class StepCycle : public hypha::Cycle
{
  public:
    using hypha::Cycle::Cycle;

    virtual void step(const hypha::CycleTime& time) = 0;

    std::expected<void, std::string> start(hypha::Node& node)
    {
        return hypha::Cycle::start(node, CallStep{this});
    }

  private:
    struct CallStep
    {
        StepCycle* cycle;

        void operator()(const hypha::CycleTime& time) const
        {
            this->cycle->step(time);
        }
    };
};

/// @brief The text of the std::runtime_error that @p call throws, or an empty text when
///        it throws none. An endpoint that the library refuses names its status there.
template <typename Call>
std::string refusalOf(Call&& call)
{
    try {
        call();
    } catch (const std::runtime_error& error) {
        return error.what();
    }
    return {};
}

/// @brief A topic name unique to the running test.
/// The transport is a real shared-memory segment that outlives one test.
inline std::string testTopic(const std::string& name)
{
    const auto* running = ::testing::UnitTest::GetInstance()->current_test_info();
    if (running == nullptr) {
        return name;
    }
    return std::string("/") + running->test_suite_name() + "/" + running->name() + name;
}

}  // namespace hypha_test

namespace ros::message_traits {

template <>
struct DataType<hypha_test::Reading>
{
    static const char* value()
    {
        return "hypha_test/Reading";
    }

    static const char* value(const hypha_test::Reading&)
    {
        return value();
    }
};

template <>
struct MD5Sum<hypha_test::Reading>
{
    static const char* value()
    {
        return "581e4b1bda6a172aa19e1038a38e01eb";
    }

    static const char* value(const hypha_test::Reading&)
    {
        return value();
    }
};

template <>
struct Definition<hypha_test::Reading>
{
    static const char* value()
    {
        return "int32 value\nstring label\n";
    }

    static const char* value(const hypha_test::Reading&)
    {
        return value();
    }
};

template <>
struct DataType<hypha_test::Command>
{
    static const char* value()
    {
        return "hypha_test/Command";
    }

    static const char* value(const hypha_test::Command&)
    {
        return value();
    }
};

template <>
struct MD5Sum<hypha_test::Command>
{
    static const char* value()
    {
        return "c2f7134c29a3ad4daf56996f7fc77409";
    }

    static const char* value(const hypha_test::Command&)
    {
        return value();
    }
};

template <>
struct Definition<hypha_test::Command>
{
    static const char* value()
    {
        return "float64 target\n";
    }

    static const char* value(const hypha_test::Command&)
    {
        return value();
    }
};

template <>
struct DataType<hypha_test::StampedReading>
{
    static const char* value()
    {
        return "hypha_test/StampedReading";
    }

    static const char* value(const hypha_test::StampedReading&)
    {
        return value();
    }
};

template <>
struct MD5Sum<hypha_test::StampedReading>
{
    static const char* value()
    {
        return "751e804437632a5ebfdb598a79ae8acb";
    }

    static const char* value(const hypha_test::StampedReading&)
    {
        return value();
    }
};

template <>
struct Definition<hypha_test::StampedReading>
{
    static const char* value()
    {
        return "Header header\nint32 value\nstring label\n"
               "================================================================================\n"
               "MSG: std_msgs/Header\nuint32 seq\ntime stamp\nstring frame_id\n";
    }

    static const char* value(const hypha_test::StampedReading&)
    {
        return value();
    }
};

template <>
struct DataType<hypha_test::ComputeTrajectory::Request>
{
    static const char* value()
    {
        return "hypha_test/ComputeTrajectoryRequest";
    }

    static const char* value(const hypha_test::ComputeTrajectory::Request&)
    {
        return value();
    }
};

template <>
struct MD5Sum<hypha_test::ComputeTrajectory::Request>
{
    static const char* value()
    {
        return "11112222333344445555666677778888";
    }

    static const char* value(const hypha_test::ComputeTrajectory::Request&)
    {
        return value();
    }
};

template <>
struct Definition<hypha_test::ComputeTrajectory::Request>
{
    static const char* value()
    {
        return "float64 max_velocity\nstring goal\n";
    }

    static const char* value(const hypha_test::ComputeTrajectory::Request&)
    {
        return value();
    }
};

template <>
struct DataType<hypha_test::ComputeTrajectory::Response>
{
    static const char* value()
    {
        return "hypha_test/ComputeTrajectoryResponse";
    }

    static const char* value(const hypha_test::ComputeTrajectory::Response&)
    {
        return value();
    }
};

template <>
struct MD5Sum<hypha_test::ComputeTrajectory::Response>
{
    static const char* value()
    {
        return "99998888777766665555444433332222";
    }

    static const char* value(const hypha_test::ComputeTrajectory::Response&)
    {
        return value();
    }
};

template <>
struct Definition<hypha_test::ComputeTrajectory::Response>
{
    static const char* value()
    {
        return "bool success\nstring message\n";
    }

    static const char* value(const hypha_test::ComputeTrajectory::Response&)
    {
        return value();
    }
};

}  // namespace ros::message_traits

namespace ros::serialization {

template <>
struct Serializer<hypha_test::Reading>
{
    template <typename Stream, typename T>
    inline static void allInOne(Stream& stream, T m)
    {
        stream.next(m.value);
        stream.next(m.label);
    }

    ROS_DECLARE_ALLINONE_SERIALIZER
};

template <>
struct Serializer<hypha_test::Command>
{
    template <typename Stream, typename T>
    inline static void allInOne(Stream& stream, T m)
    {
        stream.next(m.target);
    }

    ROS_DECLARE_ALLINONE_SERIALIZER
};

template <>
struct Serializer<hypha_test::Header>
{
    template <typename Stream, typename T>
    inline static void allInOne(Stream& stream, T m)
    {
        stream.next(m.seq);
        uint32_t sec = static_cast<uint32_t>(m.stamp.nanos() / 1000000000ULL);
        uint32_t nsec = static_cast<uint32_t>(m.stamp.nanos() % 1000000000ULL);
        stream.next(sec);
        stream.next(nsec);
        stream.next(m.frame_id);
    }

    ROS_DECLARE_ALLINONE_SERIALIZER
};

template <>
struct Serializer<hypha_test::StampedReading>
{
    template <typename Stream, typename T>
    inline static void allInOne(Stream& stream, T m)
    {
        stream.next(m.header);
        stream.next(m.value);
        stream.next(m.label);
    }

    ROS_DECLARE_ALLINONE_SERIALIZER
};

template <>
struct Serializer<hypha_test::ComputeTrajectory::Request>
{
    template <typename Stream, typename T>
    inline static void allInOne(Stream& stream, T m)
    {
        stream.next(m.max_velocity);
        stream.next(m.goal);
    }

    ROS_DECLARE_ALLINONE_SERIALIZER
};

template <>
struct Serializer<hypha_test::ComputeTrajectory::Response>
{
    template <typename Stream, typename T>
    inline static void allInOne(Stream& stream, T m)
    {
        stream.next(m.success);
        stream.next(m.message);
    }

    ROS_DECLARE_ALLINONE_SERIALIZER
};

}  // namespace ros::serialization
