// test-layer: unit (no node, no bus, no C function; #336)
// Time::fromSeconds and Duration::fromSeconds must keep every nanosecond a double holds
// (#243). A double near 1.7e9 s has a spacing of 2^-22 s (238.4 ns), so what it holds is
// that value to 238 ns; multiplying the whole value by 1e9 rounds again, in a double near
// 1.7e18 whose spacing is 256 ns, and adds up to 128 ns of error of its own.
//
// The reference is exact integer arithmetic: a finite double is m * 2^e with an integer m
// below 2^53, so its nanoseconds are m * 1e9 * 2^e. m * 1e9 is below 2^83 and fits an
// unsigned __int128; the power of two is a shift. The rounding is to nearest, ties away
// from zero.
//
// Time::seconds and Duration::seconds are the other direction (#253): they must return the
// double nearest to the exact value nanoseconds / 1e9. The reference for that is below.
#include "hypha_base/time.h"

#include <gtest/gtest.h>

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <iostream>
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

// ---------------------------------------------------------------------------------------
// seconds() (#253)
// ---------------------------------------------------------------------------------------

namespace {

__extension__ typedef unsigned __int128 Wide;

uint64_t magnitudeOf(int64_t nanos)
{
    return nanos < 0 ? 0 - static_cast<uint64_t>(nanos) : static_cast<uint64_t>(nanos);
}

/// @brief The exact distance |candidate * 1e9 - magnitude| of a finite positive double from
///        magnitude / 1e9, in units of 2^-shift ns, as an integer.
///
/// A finite double c is m * 2^-shift with an integer m below 2^53 (frexp and ldexp by 53), so
/// c * 1e9 is m * 1e9 * 2^-shift and its distance to magnitude is
/// |m * 1e9 - magnitude * 2^shift| / 2^shift. Both products are below 2^91 and fit an
/// unsigned __int128.
Wide scaledDistance(uint64_t magnitude, double candidate, int& shift)
{
    int          exponent = 0;
    const double fraction = std::frexp(candidate, &exponent);
    const auto   mantissa = static_cast<Wide>(std::ldexp(fraction, 53));
    shift = 53 - exponent;
    const Wide product = mantissa * 1000000000u;
    const Wide target = static_cast<Wide>(magnitude) << shift;
    return product > target ? product - target : target - product;
}

/// @brief |candidate - magnitude / 1e9| in seconds, as a double (its relative error is
///        about 1e-16, far below what the tests compare it to).
double distanceSeconds(uint64_t magnitude, double candidate)
{
    int        shift = 0;
    const Wide distance = scaledDistance(magnitude, candidate, shift);
    return std::ldexp(static_cast<double>(distance), -shift) / 1e9;
}

/// @brief The double nearest to the exact value nanos / 1e9, decided in integers.
///
/// The candidates are the double division's result and its two neighbours, and the exact
/// distances (scaledDistance) pick the nearest. A tie is impossible for a value that is not
/// itself a double: nanos / 1e9 = nanos / (2^9 * 5^9) has a finite binary expansion only
/// when 5^9 divides nanos, and then it is a multiple of 2^-9 below 2^34, a double.
double referenceSeconds(int64_t nanos)
{
    if (nanos == 0) {
        return 0.0;
    }
    const uint64_t magnitude = magnitudeOf(nanos);
    const double   approximate = static_cast<double>(magnitude) / 1e9;
    const double   candidates[] = {
        std::nextafter(approximate, 0.0), approximate, std::nextafter(approximate, 1e300)
    };
    // The candidates may sit on both sides of a power of two, so their units differ by a
    // factor of two: bring every distance to the finest unit.
    Wide distances[3];
    int  shifts[3];
    int  finest = 0;
    for (int i = 0; i < 3; ++i) {
        distances[i] = scaledDistance(magnitude, candidates[i], shifts[i]);
        finest = std::max(finest, shifts[i]);
    }
    double best = approximate;
    Wide   bestDistance = ~static_cast<Wide>(0);
    for (int i = 0; i < 3; ++i) {
        const Wide comparable = distances[i] << (finest - shifts[i]);
        if (comparable < bestDistance) {
            bestDistance = comparable;
            best = candidates[i];
        }
    }
    return nanos < 0 ? -best : best;
}

/// The stamps whose seconds() is checked: near the present, around the whole-second and
/// the 2^53 ns boundaries, small values, zero, and the ends of the int64 range.
std::vector<int64_t> secondsStamps()
{
    std::vector<int64_t> stamps = {
        0,
        1,
        2,
        999999999,
        1000000000,
        1000000001,
        1500000000,
        std::numeric_limits<int64_t>::max(),
        std::numeric_limits<int64_t>::max() - 1,
        std::numeric_limits<int64_t>::min() + 1
    };
    for (const int64_t edge :
         {int64_t{1} << 53, int64_t{1} << 62, int64_t{1700000000} * 1000000000}) {
        for (int64_t delta = -3; delta <= 3; ++delta) {
            stamps.push_back(edge + delta);
        }
    }
    std::mt19937_64 generator(253);
    // A few thousand stamps near the present: 1.7e18 ns up to 1.8e18 ns.
    std::uniform_int_distribution<int64_t> present(1700000000000000000LL, 1800000000000000000LL);
    for (int i = 0; i < 5000; ++i) {
        stamps.push_back(present(generator));
    }
    // Every magnitude from a nanosecond to the end of the range.
    for (int bits = 1; bits < 63; ++bits) {
        std::uniform_int_distribution<int64_t> any(
            int64_t{1} << (bits - 1), (int64_t{1} << bits) - 1
        );
        for (int i = 0; i < 40; ++i) {
            stamps.push_back(any(generator));
        }
    }
    return stamps;
}

struct SecondsTally
{
    int         checked = 0;
    int         notNearest = 0;  // not the nearest double: allowed above 2^53 ns, within a bound
    int         wrong = 0;  // outside what seconds() promises
    double      worstUlps = 0.0;
    std::string firstWrong;
};

/// seconds() promises the nearest double up to 2^53 ns. Above, it is the nearest or, only
/// when the exact value lies within 2^-52 s of the midpoint between two doubles, the
/// neighbour on the other side of that midpoint.
void recordSeconds(SecondsTally& tally, int64_t nanos, double got)
{
    const double want = referenceSeconds(nanos);
    ++tally.checked;
    if (got == want) {
        return;
    }
    ++tally.notNearest;
    bool allowed = false;
    if (magnitudeOf(nanos) > (uint64_t{1} << 53) && std::fabs(got) != std::fabs(want)
        && (std::nextafter(std::fabs(want), 1e300) == std::fabs(got)
            || std::nextafter(std::fabs(want), 0.0) == std::fabs(got))) {
        // The exact value is x; got is the neighbour of want that x is not nearest to.
        // Its distance from the midpoint is half the difference of the two distances.
        const uint64_t magnitude = magnitudeOf(nanos);
        const double   fromMidpoint = (distanceSeconds(magnitude, std::fabs(got))
                                       - distanceSeconds(magnitude, std::fabs(want)))
                                    / 2;
        allowed = std::signbit(got) == std::signbit(want) && fromMidpoint <= std::ldexp(1.0, -52);
    }
    if (allowed) {
        return;
    }
    ++tally.wrong;
    const double spacing = std::nextafter(std::fabs(want), 1e300) - std::fabs(want);
    tally.worstUlps = std::max(tally.worstUlps, std::fabs(got - want) / spacing);
    if (tally.firstWrong.empty()) {
        char text[160];
        std::snprintf(
            text,
            sizeof text,
            "seconds() of %lld ns is %.17g, the nearest double is %.17g",
            static_cast<long long>(nanos),
            got,
            want
        );
        tally.firstWrong = text;
    }
}

}  // namespace

TEST(SecondsPrecision, ReferenceMatchesKnownValues)
{
    EXPECT_EQ(referenceSeconds(0), 0.0);
    EXPECT_EQ(referenceSeconds(1500000000), 1.5);
    EXPECT_EQ(referenceSeconds(-500000000), -0.5);
    EXPECT_EQ(referenceSeconds(1), 1e-9);
    EXPECT_EQ(referenceSeconds(1700000000000000000LL), 1.7e9);
    // 1.7e9 s plus 2^-22 s is 1700000000000000238.4 ns: the double is that to a ns or two.
    EXPECT_EQ(referenceSeconds(1700000000000000238LL), 1.7e9 + std::ldexp(1.0, -22));
    EXPECT_EQ(referenceSeconds(std::numeric_limits<int64_t>::max()), 9223372036.854775807);
    EXPECT_EQ(referenceSeconds(std::numeric_limits<int64_t>::min()), -9223372036.854775808);
}

TEST(SecondsPrecision, TimeIsTheNearestDoubleUpTo2e53AndWithinTheBoundAbove)
{
    SecondsTally tally;
    for (const int64_t nanos : secondsStamps()) {
        recordSeconds(tally, nanos, Time::fromNanos(nanos).seconds());
    }
    EXPECT_EQ(tally.wrong, 0) << tally.wrong << " of " << tally.checked
                              << " stamps are off, by up to " << tally.worstUlps
                              << " spacings; first: " << tally.firstWrong;
    std::cout << "[  INFO   ] " << tally.notNearest << " of " << tally.checked
              << " stamps are not the nearest double (all within the bound)\n";
}

TEST(SecondsPrecision, DurationIsTheNearestDoubleUpTo2e53AndWithinTheBoundAbove)
{
    SecondsTally tally;
    for (const int64_t nanos : secondsStamps()) {
        recordSeconds(tally, nanos, Duration::fromNanos(nanos).seconds());
        if (nanos != std::numeric_limits<int64_t>::min()) {
            recordSeconds(tally, -nanos, Duration::fromNanos(-nanos).seconds());
        }
    }
    recordSeconds(
        tally,
        std::numeric_limits<int64_t>::min(),
        Duration::fromNanos(std::numeric_limits<int64_t>::min()).seconds()
    );
    EXPECT_EQ(tally.wrong, 0) << tally.wrong << " of " << tally.checked
                              << " spans are off, by up to " << tally.worstUlps
                              << " spacings; first: " << tally.firstWrong;
    std::cout << "[  INFO   ] " << tally.notNearest << " of " << tally.checked
              << " stamps are not the nearest double (all within the bound)\n";
}

TEST(SecondsPrecision, ConstantExpressionsAndZero)
{
    constexpr double stamp = Time::fromNanos(1700000000250000000LL).seconds();
    constexpr double span = Duration::fromNanos(-1700000000250000000LL).seconds();
    constexpr double zero = Duration::fromNanos(0).seconds();
    EXPECT_EQ(stamp, 1700000000.25);
    EXPECT_EQ(span, -1700000000.25);
    EXPECT_EQ(zero, 0.0);
    EXPECT_EQ(Time::fromNanos(-1).seconds(), -1e-9);
}

/// A double holds a time to the nanosecond when its spacing is at most 2^-30 s (0.93 ns):
/// below 2^23 s = 8388608 s (97 days), that is |nanos| < 8388608e9. There the double
/// nearest nanos / 1e9 is less than half a nanosecond from the time, so fromSeconds
/// returns the time itself.
TEST(SecondsPrecision, RoundTripIsExactWhereADoubleHoldsTheNanosecond)
{
    constexpr int64_t                      kLimit = int64_t{8388608} * 1000000000;
    std::mt19937_64                        generator(253);
    std::uniform_int_distribution<int64_t> any(-kLimit + 1, kLimit - 1);
    std::vector<int64_t>                   stamps = {
        0, 1, -1, kLimit - 1, -kLimit + 1, 999999999, 1000000000, -999999999
    };
    for (int i = 0; i < 20000; ++i) {
        stamps.push_back(any(generator));
    }
    int         wrong = 0;
    int64_t     worst = 0;
    std::string first;
    for (const int64_t nanos : stamps) {
        for (const int64_t got :
             {Time::fromSeconds(Time::fromNanos(nanos).seconds()).nanos(),
              Duration::fromSeconds(Duration::fromNanos(nanos).seconds()).nanos()}) {
            if (got != nanos) {
                ++wrong;
                worst = std::max(worst, got > nanos ? got - nanos : nanos - got);
                if (first.empty()) {
                    first = std::to_string(nanos) + " ns came back as " + std::to_string(got);
                }
            }
        }
    }
    EXPECT_EQ(wrong, 0) << wrong << " round trips are off, by up to " << worst
                        << " ns; first: " << first;
}

/// At an epoch stamp the double is coarser than a nanosecond; the round trip is then within
/// half its spacing, plus the half nanosecond fromSeconds rounds by (a result that is the
/// other neighbour of a near-midpoint value is 2^-52 s, 2e-7 ns, farther: the integer bound
/// below absorbs it).
TEST(SecondsPrecision, RoundTripNearThePresentIsWithinTheSpacingOfTheDouble)
{
    int         wrong = 0;
    std::string first;
    for (const int64_t nanos : secondsStamps()) {
        const double seconds = Time::fromNanos(nanos).seconds();
        if (nanos < 1000000000LL || std::fabs(seconds) > 9.0e9) {
            continue;  // below a second the double holds more than a ns; near the end it clamps
        }
        const double  spacingNanos = (std::nextafter(seconds, 1e300) - seconds) * 1e9;
        const int64_t got = Time::fromSeconds(seconds).nanos();
        const double  error = static_cast<double>(got > nanos ? got - nanos : nanos - got);
        if (error > std::floor(spacingNanos / 2 + 0.5)) {
            ++wrong;
            if (first.empty()) {
                first = std::to_string(nanos) + " ns came back as " + std::to_string(got);
            }
        }
    }
    EXPECT_EQ(wrong, 0) << wrong
                        << " round trips are farther than half a spacing; first: " << first;
}

/// ros::Time::toSec() is sec + 1e-9 * nsec, which rounds the product and then the sum. It
/// is not always the nearest double, but within two spacings of it. Information: how many
/// stamps differ.
TEST(SecondsPrecision, RosFormulaIsWithinTwoSpacingsOfTheNearestDouble)
{
    int checked = 0;
    int differ = 0;
    for (const int64_t nanos : secondsStamps()) {
        if (nanos < 0) {
            continue;
        }
        const double ros = static_cast<double>(nanos / 1000000000)
                         + 1e-9 * static_cast<double>(nanos % 1000000000);
        const double ours = Time::fromNanos(nanos).seconds();
        ++checked;
        if (ros != ours) {
            ++differ;
            EXPECT_LE(std::fabs(ros - ours), 2.0 * (std::nextafter(ours, 1e300) - ours)) << nanos;
        }
    }
    std::cout << "[  INFO   ] ros::Time::toSec formula differs from seconds() for " << differ
              << " of " << checked << " stamps\n";
}
