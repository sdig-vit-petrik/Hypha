#pragma once

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

    /// @brief Construct from seconds; the fractional part is truncated toward zero.
    ///        Out-of-range input clamps.
    /// @throws std::invalid_argument for NaN or infinite seconds; in a constant
    ///         expression that is a compile error. tryFromSeconds() does not throw.
    static constexpr Duration fromSeconds(double seconds)
    {
        const bool secondsAreFinite = std::isfinite(seconds);
        if (!secondsAreFinite) {
            throw std::invalid_argument("hypha::Duration::fromSeconds: seconds are not finite");
        }
        return Duration(detail::clampedNanos(seconds * 1e9));
    }

    /// @brief fromSeconds() for a path that does not throw: empty for NaN or infinite
    ///        seconds. Finite seconds convert and clamp as fromSeconds() does.
    static constexpr std::optional<Duration> tryFromSeconds(double seconds)
    {
        const bool secondsAreFinite = std::isfinite(seconds);
        if (!secondsAreFinite) {
            return std::nullopt;
        }
        return Duration(detail::clampedNanos(seconds * 1e9));
    }

    constexpr int64_t nanos() const
    {
        return this->nanoseconds;
    }

    constexpr double seconds() const
    {
        return static_cast<double>(this->nanoseconds) / 1e9;
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

    /// @brief Construct from seconds; the fractional part is truncated toward zero.
    ///        Out-of-range seconds clamp.
    /// @throws std::invalid_argument for NaN or infinite seconds; in a constant
    ///         expression that is a compile error.
    static constexpr Time fromSeconds(double seconds)
    {
        const bool secondsAreFinite = std::isfinite(seconds);
        if (!secondsAreFinite) {
            throw std::invalid_argument("hypha::Time::fromSeconds: seconds are not finite");
        }
        return Time(detail::clampedNanos(seconds * 1e9));
    }

    constexpr int64_t nanos() const
    {
        return this->nanoseconds;
    }

    constexpr double seconds() const
    {
        return static_cast<double>(this->nanoseconds) / 1e9;
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
