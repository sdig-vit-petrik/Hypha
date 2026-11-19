#pragma once

/// @file
/// ros::Time, ros::Duration and ros::Rate over hypha time. ros::Time::now() is the
/// scheduled time of the step that runs plugin code (set by mrs::PluginHost), so a
/// plugin sees the same time on every replay; outside a step it is hypha::now().

#include <hypha_base/clock.h>
#include <hypha_base/time.h>
#include <hypha_msgs/ros_time.h>

#include <cmath>
#include <cstdint>
#include <optional>

namespace mrs {

/// The scheduled time of the running step, or empty outside a step.
std::optional<hypha::Time> currentStepTime();
void                       setCurrentStepTime(std::optional<hypha::Time> time);

}  // namespace mrs

namespace ros {

namespace detail {
inline void normalize(int64_t total, int32_t& sec, int32_t& nsec)
{
    int64_t s = total / 1000000000;
    int64_t n = total % 1000000000;
    if (n < 0) {
        n += 1000000000;
        s -= 1;
    }
    sec = static_cast<int32_t>(s);
    nsec = static_cast<int32_t>(n);
}
}  // namespace detail

class Duration
{
  public:
    int32_t sec{0};
    int32_t nsec{0};

    Duration() = default;

    Duration(int32_t s, int32_t n)
    {
        detail::normalize(int64_t{s} * 1000000000 + n, this->sec, this->nsec);
    }

    explicit Duration(double seconds)
    {
        detail::normalize(std::llround(seconds * 1e9), this->sec, this->nsec);
    }

    Duration(const hypha_msgs::RosDuration& d) : Duration(d.secs, d.nsecs)  // NOLINT: implicit
    {
    }

    Duration(hypha::Duration d)  // NOLINT: implicit, as ROS converts its own types
    {
        detail::normalize(d.nanos(), this->sec, this->nsec);
    }

    operator hypha_msgs::RosDuration() const  // NOLINT
    {
        return hypha_msgs::RosDuration{this->sec, this->nsec};
    }

    static Duration fromNSec(int64_t n)
    {
        Duration d;
        detail::normalize(n, d.sec, d.nsec);
        return d;
    }

    double toSec() const
    {
        return static_cast<double>(this->toNSec()) * 1e-9;
    }

    int64_t toNSec() const
    {
        return int64_t{this->sec} * 1000000000 + this->nsec;
    }

    bool isZero() const
    {
        return this->sec == 0 && this->nsec == 0;
    }

    hypha::Duration toHypha() const
    {
        return hypha::Duration::fromNanos(this->toNSec());
    }

    /// Blocking sleeps have no place in a step: a call does not compile.
    bool sleep() const = delete;

    friend Duration operator+(Duration a, Duration b)
    {
        return fromNSec(a.toNSec() + b.toNSec());
    }

    friend Duration operator-(Duration a, Duration b)
    {
        return fromNSec(a.toNSec() - b.toNSec());
    }

    friend Duration operator-(Duration a)
    {
        return fromNSec(-a.toNSec());
    }

    friend Duration operator*(Duration a, double k)
    {
        return Duration(a.toSec() * k);
    }

    friend Duration operator*(double k, Duration a)
    {
        return Duration(a.toSec() * k);
    }

    friend Duration operator/(Duration a, double k)
    {
        return Duration(a.toSec() / k);
    }

    Duration& operator+=(Duration o)
    {
        return *this = *this + o;
    }

    Duration& operator-=(Duration o)
    {
        return *this = *this - o;
    }

    friend auto operator<=>(Duration a, Duration b)
    {
        return a.toNSec() <=> b.toNSec();
    }

    friend bool operator==(Duration a, Duration b)
    {
        return a.toNSec() == b.toNSec();
    }
};

using WallDuration = Duration;

class Time
{
  public:
    uint32_t sec{0};
    uint32_t nsec{0};

    Time() = default;

    Time(uint32_t s, uint32_t n) : sec(s + n / 1000000000), nsec(n % 1000000000)
    {
    }

    explicit Time(double seconds)
    {
        *this = fromNSec(std::llround(seconds * 1e9));
    }

    Time(const hypha_msgs::RosTime& t) : Time(t.secs, t.nsecs)  // NOLINT: implicit
    {
    }

    operator hypha_msgs::RosTime() const  // NOLINT: header.stamp = ros::Time::now()
    {
        return hypha_msgs::RosTime{this->sec, this->nsec};
    }

    static Time fromNSec(int64_t n)
    {
        Time t;
        if (n <= 0) {
            return t;
        }
        t.sec = static_cast<uint32_t>(n / 1000000000);
        t.nsec = static_cast<uint32_t>(n % 1000000000);
        return t;
    }

    static Time fromHypha(hypha::Time time)
    {
        return fromNSec(time.nanos());
    }

    /// The scheduled time of the running step, else framework time.
    static Time now()
    {
        const std::optional<hypha::Time> step = mrs::currentStepTime();
        return fromHypha(step.has_value() ? *step : hypha::now());
    }

    /// Framework time is always valid for the port: there is nothing to wait for.
    static bool waitForValid()
    {
        return true;
    }

    double toSec() const
    {
        return static_cast<double>(this->toNSec()) * 1e-9;
    }

    int64_t toNSec() const
    {
        return int64_t{this->sec} * 1000000000 + this->nsec;
    }

    bool isZero() const
    {
        return this->sec == 0 && this->nsec == 0;
    }

    hypha::Time toHypha() const
    {
        return hypha::Time::fromNanos(this->toNSec());
    }

    friend Duration operator-(const Time& a, const Time& b)
    {
        return Duration::fromNSec(a.toNSec() - b.toNSec());
    }

    friend Time operator+(const Time& a, Duration d)
    {
        return fromNSec(a.toNSec() + d.toNSec());
    }

    friend Time operator-(const Time& a, Duration d)
    {
        return fromNSec(a.toNSec() - d.toNSec());
    }

    Time& operator+=(Duration d)
    {
        return *this = *this + d;
    }

    Time& operator-=(Duration d)
    {
        return *this = *this - d;
    }

    friend auto operator<=>(const Time& a, const Time& b)
    {
        return a.toNSec() <=> b.toNSec();
    }

    friend bool operator==(const Time& a, const Time& b)
    {
        return a.toNSec() == b.toNSec();
    }

    static const Time UNINITIALIZED;
};

inline const Time Time::UNINITIALIZED{};

using WallTime = Time;

class Rate
{
  public:
    explicit Rate(double hz) : period_(1.0 / hz)
    {
    }

    explicit Rate(Duration period) : period_(period)
    {
    }

    Duration expectedCycleTime() const
    {
        return this->period_;
    }

    bool sleep() = delete;

  private:
    Duration period_;
};

}  // namespace ros
