// Time::fromSeconds and Duration::fromSeconds must keep every nanosecond a double holds
// (#243). A double near 1.7e9 s has a spacing of 2^-22 s (238.4 ns), so what it holds is
// that value to 238 ns; multiplying the whole value by 1e9 rounds again, in a double near
// 1.7e18 whose spacing is 256 ns, and adds up to 128 ns of error of its own.
//
// The reference is exact integer arithmetic: a finite double is m * 2^e with an integer m
// below 2^53, so its nanoseconds are m * 1e9 * 2^e. m * 1e9 is below 2^83 and fits an
// unsigned __int128; the power of two is a shift. The rounding is to nearest, ties away
// from zero.
#include "hypha_base/time.h"

#include <gtest/gtest.h>

#include <cmath>
#include <cstdint>
#include <limits>
#include <random>
#include <stdexcept>
#include <string>
#include <vector>

using hypha::Duration;
using hypha::Time;

namespace {

/// @brief The exact nanoseconds of `seconds` (|seconds| >= 1e-3, below 9.2e9), rounded to
///        nearest, ties away from zero.
int64_t referenceNanos(double seconds)
{
    int          exponent = 0;
    const double fraction = std::frexp(std::fabs(seconds), &exponent);
    // fraction is in [0.5, 1): ldexp by 53 makes it the integer m, and the value is m * 2^shift.
    const auto              mantissa = static_cast<unsigned __int128>(std::ldexp(fraction, 53));
    const int               shift = exponent - 53;
    const unsigned __int128 scaled = mantissa * 1000000000u;
    unsigned __int128       nanos = 0;
    if (shift >= 0) {
        nanos = scaled << shift;
    } else {
        const int               drop = -shift;
        const unsigned __int128 one = 1;
        nanos = scaled >> drop;
        const unsigned __int128 remainder = scaled & ((one << drop) - 1);
        if (remainder * 2 >= (one << drop)) {
            nanos += 1;
        }
    }
    const auto magnitude = static_cast<int64_t>(nanos);
    return seconds < 0 ? -magnitude : magnitude;
}

struct Tally
{
    int         checked = 0;
    int         wrong = 0;
    int64_t     worstError = 0;
    std::string firstWrong;
};

void record(Tally& tally, double seconds, int64_t got)
{
    const int64_t want = referenceNanos(seconds);
    ++tally.checked;
    if (got == want) {
        return;
    }
    ++tally.wrong;
    const int64_t error = got > want ? got - want : want - got;
    if (error > tally.worstError) {
        tally.worstError = error;
    }
    if (tally.firstWrong.empty()) {
        tally.firstWrong = "fromSeconds(" + std::to_string(seconds) + ") = " + std::to_string(got)
                         + ", the double holds " + std::to_string(want);
    }
}

/// The stamps: 1.7e9 + k * 2^-22 over the whole fraction range (every fraction such a
/// double can hold), then a few thousand pseudo-random stamps near the present.
std::vector<double> nearThePresent()
{
    std::vector<double> stamps;
    const double        step = std::ldexp(1.0, -22);
    for (int k = 0; k < (1 << 22); k += 997) {
        stamps.push_back(1.7e9 + k * step);
    }
    std::mt19937_64                        generator(243);
    std::uniform_real_distribution<double> present(1.70e9, 1.80e9);
    for (int i = 0; i < 5000; ++i) {
        stamps.push_back(present(generator));
    }
    return stamps;
}

}  // namespace

TEST(FromSecondsPrecision, ReferenceMatchesKnownValues)
{
    EXPECT_EQ(referenceNanos(1.5), 1500000000);
    EXPECT_EQ(referenceNanos(-0.5), -500000000);
    EXPECT_EQ(referenceNanos(1.7e9), 1700000000000000000LL);
    // 2^-22 s is 238.418579101562 ns: it rounds to 238.
    EXPECT_EQ(referenceNanos(1.7e9 + std::ldexp(1.0, -22)), 1700000000000000238LL);
    // 2^-10 s is 976562.5 ns exactly: a tie, away from zero.
    EXPECT_EQ(referenceNanos(1.7e9 + std::ldexp(1.0, -10)), 1700000000000976563LL);
    EXPECT_EQ(referenceNanos(-(1.7e9 + std::ldexp(1.0, -10))), -1700000000000976563LL);
}

TEST(FromSecondsPrecision, TimeKeepsWhatTheDoubleHoldsNearThePresent)
{
    Tally tally;
    for (const double seconds : nearThePresent()) {
        record(tally, seconds, Time::fromSeconds(seconds).nanos());
    }
    EXPECT_EQ(tally.wrong, 0) << tally.wrong << " of " << tally.checked
                              << " stamps are off, by up to " << tally.worstError
                              << " ns; first: " << tally.firstWrong;
}

TEST(FromSecondsPrecision, DurationKeepsWhatTheDoubleHoldsNearThePresent)
{
    Tally tally;
    for (const double seconds : nearThePresent()) {
        record(tally, seconds, Duration::fromSeconds(seconds).nanos());
        record(tally, -seconds, Duration::fromSeconds(-seconds).nanos());
    }
    EXPECT_EQ(tally.wrong, 0) << tally.wrong << " of " << tally.checked
                              << " spans are off, by up to " << tally.worstError
                              << " ns; first: " << tally.firstWrong;
}

TEST(FromSecondsPrecision, TryFromSecondsAgreesWithFromSeconds)
{
    Tally tally;
    for (const double seconds : nearThePresent()) {
        record(tally, seconds, Duration::tryFromSeconds(seconds)->nanos());
    }
    EXPECT_EQ(tally.wrong, 0) << tally.wrong << " of " << tally.checked
                              << " spans are off, by up to " << tally.worstError
                              << " ns; first: " << tally.firstWrong;
}

TEST(FromSecondsPrecision, NegativeValuesAndValuesJustBelowAWholeSecond)
{
    EXPECT_EQ(Time::fromSeconds(-0.5).nanos(), -500000000);
    EXPECT_EQ(Duration::fromSeconds(-0.5).nanos(), -500000000);
    EXPECT_EQ(Time::fromSeconds(-1.5).nanos(), -1500000000);
    EXPECT_EQ(Time::fromSeconds(-1.7e9).nanos(), -1700000000000000000LL);
    EXPECT_EQ(Time::fromSeconds(0.0).nanos(), 0);
    EXPECT_EQ(Time::fromSeconds(-0.0).nanos(), 0);

    Tally tally;
    for (const double whole : {1.0, 2.0, 1024.0, 1.0e6, 1.7e9, 4.0e9}) {
        const double below = std::nextafter(whole, 0.0);
        record(tally, below, Time::fromSeconds(below).nanos());
        record(tally, -below, Time::fromSeconds(-below).nanos());
        record(tally, below, Duration::fromSeconds(below).nanos());
        record(tally, -below, Duration::fromSeconds(-below).nanos());
        // The fraction of a value below a whole second must never reach the next second.
        EXPECT_LE(Time::fromSeconds(below).nanos(), static_cast<int64_t>(whole) * 1000000000LL);
        EXPECT_GE(Time::fromSeconds(-below).nanos(), -static_cast<int64_t>(whole) * 1000000000LL);
    }
    EXPECT_EQ(tally.wrong, 0) << tally.wrong << " of " << tally.checked
                              << " values are off, by up to " << tally.worstError
                              << " ns; first: " << tally.firstWrong;
}

TEST(FromSecondsPrecision, FractionsBelowANanosecondRoundToNearest)
{
    EXPECT_EQ(Time::fromSeconds(1.4e-9).nanos(), 1);
    EXPECT_EQ(Time::fromSeconds(1.6e-9).nanos(), 2);
    EXPECT_EQ(Time::fromSeconds(-1.6e-9).nanos(), -2);
    EXPECT_EQ(Duration::fromSeconds(2.6e-9).nanos(), 3);
    EXPECT_EQ(Time::fromSeconds(0.7).nanos(), 700000000);
    EXPECT_EQ(Time::fromSeconds(0.3).nanos(), 300000000);
}

TEST(FromSecondsPrecision, KeepsItsRefusalsAndClamps)
{
    const double nan = std::numeric_limits<double>::quiet_NaN();
    const double inf = std::numeric_limits<double>::infinity();
    EXPECT_THROW(static_cast<void>(Time::fromSeconds(nan)), std::invalid_argument);
    EXPECT_THROW(static_cast<void>(Time::fromSeconds(-inf)), std::invalid_argument);
    EXPECT_THROW(static_cast<void>(Duration::fromSeconds(inf)), std::invalid_argument);
    EXPECT_FALSE(Duration::tryFromSeconds(nan).has_value());
    EXPECT_EQ(Time::fromSeconds(9.3e9).nanos(), std::numeric_limits<int64_t>::max());
    EXPECT_EQ(Time::fromSeconds(-9.3e9).nanos(), std::numeric_limits<int64_t>::min());
    // Just past the int64 range, inside the last whole second the range holds.
    EXPECT_EQ(Time::fromSeconds(9223372036.9).nanos(), std::numeric_limits<int64_t>::max());
    EXPECT_EQ(Time::fromSeconds(-9223372036.9).nanos(), std::numeric_limits<int64_t>::min());
    EXPECT_EQ(Time::fromSeconds(9223372036.5).nanos(), 9223372036500000000LL);
}

TEST(FromSecondsPrecision, StaysUsableInConstantExpressions)
{
    constexpr Time stamp = Time::fromSeconds(1.7e9 + 0.25);
    static_assert(stamp.nanos() == 1700000000250000000LL);
    constexpr Duration span = Duration::fromSeconds(-0.5);
    static_assert(span.nanos() == -500000000);
    EXPECT_EQ(stamp.nanos(), 1700000000250000000LL);
}
