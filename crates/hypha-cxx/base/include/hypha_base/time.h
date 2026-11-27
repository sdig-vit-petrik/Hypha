#pragma once

#include "hypha_base/short_namespace.h"

#include <chrono>
#include <cmath>
#include <cstdint>
#include <limits>
#include <optional>
#include <stdexcept>

namespace hypha {

namespace detail {

/// @brief The int64 sum. An overflow clamps to the limit on the side it went.
constexpr int64_t saturatingAdd(int64_t lhs, int64_t rhs)
{
    int64_t sum = 0;
    if (!__builtin_add_overflow(lhs, rhs, &sum)) {
        return sum;
    }
    return rhs > 0 ? std::numeric_limits<int64_t>::max() : std::numeric_limits<int64_t>::min();
}

/// @brief The int64 difference. An overflow clamps to the limit on the side it went.
constexpr int64_t saturatingSub(int64_t lhs, int64_t rhs)
{
    int64_t difference = 0;
    if (!__builtin_sub_overflow(lhs, rhs, &difference)) {
        return difference;
    }
    return rhs < 0 ? std::numeric_limits<int64_t>::max() : std::numeric_limits<int64_t>::min();
}

/// @brief The int64 product. An overflow clamps to the limit of its sign.
constexpr int64_t saturatingMul(int64_t lhs, int64_t rhs)
{
    int64_t product = 0;
    if (!__builtin_mul_overflow(lhs, rhs, &product)) {
        return product;
    }
    const bool productIsPositive = (lhs < 0) == (rhs < 0);
    return productIsPositive ? std::numeric_limits<int64_t>::max()
                             : std::numeric_limits<int64_t>::min();
}

/// @brief The int64 nearest `value` toward zero, clamped to the int64 range.
///        Callers refuse NaN and infinities first.
constexpr int64_t clampedNanos(double value)
{
    constexpr double kUpperBound = 9223372036854775808.0;
    if (value >= kUpperBound) {
        return std::numeric_limits<int64_t>::max();
    }
    if (value <= -kUpperBound) {
        return std::numeric_limits<int64_t>::min();
    }
    return static_cast<int64_t>(value);
}

/// @brief Whole nanoseconds of finite `seconds`, rounded to nearest (ties away from
///        zero), clamped to the int64 range. Callers refuse NaN and infinities first.
///
/// Whole seconds and the fraction convert apart, as ros::Time::fromSec does. Multiplying
/// the whole value by 1e9 first rounds a second time: a double near 1.7e9 s holds 2^-22 s,
/// but its product with 1e9 sits near 1.7e18, where a double holds only multiples of 256 ns,
/// so the product is up to 128 ns away from what the double holds (#243). The split is
/// exact: seconds - trunc(seconds) is exact in a double, and a fraction below one second
/// times 1e9 is below 1e9, where a double holds well under a nanosecond.
constexpr int64_t nanosFromSeconds(double seconds)
{
    // The int64 range ends at 9223372036.854775807 s; one second past it is out for certain.
    constexpr double kOutOfRange = 9223372037.0;
    if (seconds >= kOutOfRange) {
        return std::numeric_limits<int64_t>::max();
    }
    if (seconds <= -kOutOfRange) {
        return std::numeric_limits<int64_t>::min();
    }
    const auto   whole = static_cast<int64_t>(seconds);
    const double scaled = (seconds - static_cast<double>(whole)) * 1e9;
    auto         fraction = static_cast<int64_t>(scaled);
    const double remainder = scaled - static_cast<double>(fraction);
    if (remainder >= 0.5) {
        fraction += 1;
    } else if (remainder <= -0.5) {
        fraction -= 1;
    }
    return saturatingAdd(saturatingMul(whole, 1000000000), fraction);
}

/// @brief `nanos / 1e9` as a double: the nearest one up to 2^53 ns, and above that nearly.
///
/// `double(nanos) / 1e9` rounds twice above 2^53 ns (104 days): the conversion of the
/// nanoseconds already drops low bits, to a multiple of 256 ns at a stamp since the epoch (#253).
/// Up to 2^53 ns both operands are exact and the single division is the nearest double. Above,
/// the whole seconds and the remainder (`/` and `%` truncate toward zero, so negatives mirror)
/// are each exact in a double, the division of the remainder rounds far below the spacing of
/// the result, and the sum rounds once. The result is the nearest double except where the exact
/// value lies within about 1e-16 s of a midpoint between two doubles, where it may be the
/// other neighbour.
constexpr double secondsFromNanos(int64_t nanos)
{
    constexpr int64_t kExactNanos = int64_t{1} << 53;
    if (nanos >= -kExactNanos && nanos <= kExactNanos) {
        return static_cast<double>(nanos) / 1e9;
    }
    return static_cast<double>(nanos / 1000000000) + static_cast<double>(nanos % 1000000000) / 1e9;
}

}  // namespace detail

/// @brief A signed time span with nanosecond resolution.
///
/// Arithmetic saturates at the int64 limits, as hypha_time does in Rust.
class Duration
{
  public:
    constexpr Duration() = default;

    static constexpr Duration fromNanos(int64_t nanos)
    {
        return Duration(nanos);
    }

    static constexpr Duration fromMicros(int64_t micros)
    {
        return Duration(detail::saturatingMul(micros, 1000));
    }

    static constexpr Duration fromMillis(int64_t millis)
    {
        return Duration(detail::saturatingMul(millis, 1000000));
    }

    /// @brief Construct from seconds, rounded to the nearest nanosecond (ties away from
    ///        zero) without losing what the double holds. Out-of-range input clamps.
    /// @throws std::invalid_argument for NaN or infinite seconds; in a constant
    ///         expression that is a compile error. tryFromSeconds() does not throw.
    static constexpr Duration fromSeconds(double seconds)
    {
        const bool secondsAreFinite = std::isfinite(seconds);
        if (!secondsAreFinite) {
            throw std::invalid_argument("hypha::Duration::fromSeconds: seconds are not finite");
        }
        return Duration(detail::nanosFromSeconds(seconds));
    }

    /// @brief fromSeconds() for a path that does not throw: empty for NaN or infinite
    ///        seconds. Finite seconds convert and clamp as fromSeconds() does.
    static constexpr std::optional<Duration> tryFromSeconds(double seconds)
    {
        const bool secondsAreFinite = std::isfinite(seconds);
        if (!secondsAreFinite) {
            return std::nullopt;
        }
        return Duration(detail::nanosFromSeconds(seconds));
    }

    constexpr int64_t nanos() const
    {
        return this->nanoseconds;
    }

    /// @brief The span in seconds: the double nearest to the exact value.
    constexpr double seconds() const
    {
        return detail::secondsFromNanos(this->nanoseconds);
    }

    constexpr std::chrono::nanoseconds chrono() const
    {
        return std::chrono::nanoseconds(this->nanoseconds);
    }

    constexpr bool isZero() const
    {
        return this->nanoseconds == 0;
    }

    constexpr Duration operator+(Duration other) const
    {
        return Duration(detail::saturatingAdd(this->nanoseconds, other.nanoseconds));
    }

    constexpr Duration operator-(Duration other) const
    {
        return Duration(detail::saturatingSub(this->nanoseconds, other.nanoseconds));
    }

    constexpr Duration operator-() const
    {
        if (this->nanoseconds == std::numeric_limits<int64_t>::min()) {
            return Duration(std::numeric_limits<int64_t>::max());
        }
        return Duration(-this->nanoseconds);
    }

    /// @brief Scale by `factor`. An out-of-range product clamps.
    /// @throws std::invalid_argument for a NaN or infinite factor.
    constexpr Duration operator*(double factor) const
    {
        const bool factorIsFinite = std::isfinite(factor);
        if (!factorIsFinite) {
            throw std::invalid_argument("hypha::Duration::operator*: factor is not finite");
        }
        const double scaled = static_cast<double>(this->nanoseconds) * factor;
        return Duration(detail::clampedNanos(scaled));
    }

    constexpr Duration& operator+=(Duration other)
    {
        this->nanoseconds = detail::saturatingAdd(this->nanoseconds, other.nanoseconds);
        return *this;
    }

    constexpr Duration& operator-=(Duration other)
    {
        this->nanoseconds = detail::saturatingSub(this->nanoseconds, other.nanoseconds);
        return *this;
    }

    constexpr bool operator==(const Duration&) const = default;
    constexpr auto operator<=>(const Duration&) const = default;

  private:
    explicit constexpr Duration(int64_t nanos) : nanoseconds(nanos)
    {
    }

    int64_t nanoseconds{0};
};

/// @brief A point in time with nanosecond resolution. Values from different clock sources
///        are not comparable.
///
/// Arithmetic saturates at the int64 limits, as hypha_time does in Rust.
class Time
{
  public:
    constexpr Time() = default;

    static constexpr Time fromNanos(int64_t nanos)
    {
        return Time(nanos);
    }

    /// @brief Construct from seconds, rounded to the nearest nanosecond (ties away from
    ///        zero) without losing what the double holds. Out-of-range seconds clamp.
    /// @throws std::invalid_argument for NaN or infinite seconds; in a constant
    ///         expression that is a compile error.
    static constexpr Time fromSeconds(double seconds)
    {
        const bool secondsAreFinite = std::isfinite(seconds);
        if (!secondsAreFinite) {
            throw std::invalid_argument("hypha::Time::fromSeconds: seconds are not finite");
        }
        return Time(detail::nanosFromSeconds(seconds));
    }

    constexpr int64_t nanos() const
    {
        return this->nanoseconds;
    }

    /// @brief The time in seconds since the epoch: the double nearest to the exact value.
    constexpr double seconds() const
    {
        return detail::secondsFromNanos(this->nanoseconds);
    }

    /// @brief Sentinel meaning "time is not yet known". A default-constructed Time is zero.
    constexpr bool isZero() const
    {
        return this->nanoseconds == 0;
    }

    constexpr Time operator+(Duration d) const
    {
        return Time(detail::saturatingAdd(this->nanoseconds, d.nanos()));
    }

    constexpr Time operator-(Duration d) const
    {
        return Time(detail::saturatingSub(this->nanoseconds, d.nanos()));
    }

    constexpr Duration operator-(Time other) const
    {
        return Duration::fromNanos(detail::saturatingSub(this->nanoseconds, other.nanoseconds));
    }

    constexpr Time& operator+=(Duration d)
    {
        this->nanoseconds = detail::saturatingAdd(this->nanoseconds, d.nanos());
        return *this;
    }

    constexpr bool operator==(const Time&) const = default;
    constexpr auto operator<=>(const Time&) const = default;

  private:
    explicit constexpr Time(int64_t nanos) : nanoseconds(nanos)
    {
    }

    int64_t nanoseconds{0};
};

inline constexpr Duration operator*(double factor, Duration d)
{
    return d * factor;
}

namespace literals {
constexpr Duration operator""_ns(unsigned long long v)
{
    return Duration::fromNanos(static_cast<int64_t>(v));
}

constexpr Duration operator""_us(unsigned long long v)
{
    return Duration::fromMicros(static_cast<int64_t>(v));
}

constexpr Duration operator""_ms(unsigned long long v)
{
    return Duration::fromMillis(static_cast<int64_t>(v));
}

constexpr Duration operator""_s(unsigned long long v)
{
    return Duration::fromSeconds(static_cast<double>(v));
}

constexpr Duration operator""_s(long double v)
{
    return Duration::fromSeconds(static_cast<double>(v));
}
}  // namespace literals

}  // namespace hypha
