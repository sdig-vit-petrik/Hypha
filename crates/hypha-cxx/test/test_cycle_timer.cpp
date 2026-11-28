// CycleTimer: a second rate inside a step, polled with the step's time. The unit tests
// need no node; the StepHarness tests step a CycleNode at chosen times; the bus tests
// start one, under simulated time (two runs, two ways of moving the clock, one tick
// sequence) and on host time.

#include "hypha/cycle_node.h"
#include "hypha/cycle_timer.h"
#include "hypha/hypha.h"
#include "hypha/testing.h"

#include "test_types.h"

#include <gtest/gtest.h>
#include <std_msgs/Float64.h>

#include <chrono>
#include <cstdint>
#include <optional>
#include <random>
#include <stdexcept>
#include <string>
#include <tuple>
#include <vector>

using hypha::CycleTimer;
using hypha::Duration;
using hypha::Time;
using namespace hypha::literals;

namespace {

hypha::CycleTime at(int64_t millis)
{
    return hypha::CycleTime{.now = Time::fromNanos(millis * 1'000'000)};
}

int64_t millisOf(Time time)
{
    return time.nanos() / 1'000'000;
}

using Millis = std::vector<int64_t>;

}  // namespace

TEST(CycleTimer, TheFirstPollAnchorsTheGridAndGivesNoTick)
{
    CycleTimer timer{10_ms};
    EXPECT_TRUE(timer.running());
    EXPECT_FALSE(timer.next().has_value()) << "no grid before the first poll";

    EXPECT_FALSE(timer.poll(at(1000)));
    EXPECT_EQ(timer.next(), Time::fromNanos(1010'000'000));
    EXPECT_FALSE(timer.poll(at(1009)));
    const CycleTimer::Tick tick = timer.poll(at(1010));
    EXPECT_EQ(tick.periods, 1U) << "a step exactly at the instant ticks";
    EXPECT_EQ(millisOf(tick.expected), 1010);
}

TEST(CycleTimer, ATenHertzTimerInAHundredHertzCycleTicksOnEveryTenthStep)
{
    CycleTimer timer{hypha::Hz(10.0)};
    EXPECT_EQ(timer.period(), 100_ms);
    Millis ticked;
    Millis expected;
    for (int64_t t = 0; t <= 1000; t += 10) {
        const CycleTimer::Tick tick = timer.poll(at(t));
        if (tick) {
            ASSERT_EQ(tick.periods, 1U);
            ASSERT_EQ(tick.missed(), 0U);
            ticked.push_back(t);
            expected.push_back(millisOf(tick.expected));
        }
    }
    EXPECT_EQ(ticked, (Millis{100, 200, 300, 400, 500, 600, 700, 800, 900, 1000}));
    EXPECT_EQ(expected, ticked);
    EXPECT_EQ(timer.missedTotal(), 0U);
}

TEST(CycleTimer, ALongStepCountsItsMissedPeriodsAndTheGridDoesNotDrift)
{
    CycleTimer timer{100_ms};
    static_cast<void>(timer.poll(at(0)));
    EXPECT_EQ(timer.poll(at(100)).periods, 1U);

    // A step that took 350 ms: the next poll is at 460 ms.
    const CycleTimer::Tick late = timer.poll(at(460));
    EXPECT_EQ(late.periods, 3U) << "200, 300 and 400";
    EXPECT_EQ(late.missed(), 2U);
    EXPECT_EQ(millisOf(late.expected), 400) << "the newest instant passed";
    EXPECT_EQ(timer.missedTotal(), 2U);

    EXPECT_FALSE(timer.poll(at(490)));
    const CycleTimer::Tick onGrid = timer.poll(at(500));
    EXPECT_EQ(onGrid.periods, 1U) << "still on multiples of 100";
    EXPECT_EQ(onGrid.missed(), 0U);
    EXPECT_EQ(timer.missedTotal(), 2U);
}

TEST(CycleTimer, AnEarlyStepFindsNoneAndTheNextOneBoth)
{
    CycleTimer timer{10_ms};
    static_cast<void>(timer.poll(at(0)));
    EXPECT_EQ(timer.poll(at(10)).periods, 1U);
    EXPECT_FALSE(timer.poll(at(19)));
    EXPECT_EQ(timer.poll(at(30)).periods, 2U);
}

TEST(CycleTimer, APeriodThatTheStepDoesNotDivideTicksInTheFirstStepAtOrAfterEachInstant)
{
    CycleTimer timer{100_ms};
    static_cast<void>(timer.poll(at(0)));
    Millis steps;
    for (int64_t t = 30; t <= 600; t += 30) {
        if (timer.poll(at(t))) {
            steps.push_back(t);
        }
    }
    EXPECT_EQ(steps, (Millis{120, 210, 300, 420, 510, 600}));
}

TEST(CycleTimer, ElapsedIsTheTimeSinceThePreviousTickRan)
{
    CycleTimer timer{100_ms};
    static_cast<void>(timer.poll(at(0)));
    const CycleTimer::Tick first = timer.poll(at(120));
    ASSERT_TRUE(first);
    EXPECT_FALSE(first.elapsed.has_value()) << "no tick before the first one";
    EXPECT_FALSE(timer.poll(at(120))) << "a second poll at the same time finds nothing";
    EXPECT_EQ(timer.poll(at(210)).elapsed, 90_ms);
    EXPECT_EQ(timer.poll(at(440)).elapsed, 230_ms);
}

TEST(CycleTimer, StopEndsTheTicksAndStartAnchorsTheGridAgain)
{
    CycleTimer timer{10_ms};
    static_cast<void>(timer.poll(at(0)));
    EXPECT_TRUE(timer.poll(at(10)));

    timer.stop();
    EXPECT_FALSE(timer.running());
    EXPECT_FALSE(timer.next().has_value());
    for (int64_t t = 20; t <= 50; t += 10) {
        EXPECT_FALSE(timer.poll(at(t)));
    }
    EXPECT_EQ(timer.missedTotal(), 0U) << "a stopped timer misses nothing";

    timer.start(at(53));
    EXPECT_TRUE(timer.running());
    EXPECT_EQ(timer.next(), Time::fromNanos(63'000'000));
    EXPECT_FALSE(timer.poll(at(53))) << "no tick at the start itself";
    const CycleTimer::Tick tick = timer.poll(at(63));
    EXPECT_EQ(tick.periods, 1U) << "the grid is anchored at the start";
    EXPECT_FALSE(tick.elapsed.has_value()) << "the first tick after a start has none";
    EXPECT_EQ(timer.poll(at(73)).periods, 1U);
}

TEST(CycleTimer, StartWithoutATimeAnchorsAtTheNextPoll)
{
    CycleTimer timer{10_ms};
    timer.stop();  // a timer that activate() starts (ROS autostart = false)
    for (int64_t t = 0; t <= 100; t += 10) {
        ASSERT_FALSE(timer.poll(at(t)));
    }
    timer.start();
    EXPECT_FALSE(timer.poll(at(105)));
    EXPECT_TRUE(timer.poll(at(115)));
}

TEST(CycleTimer, StartOfARunningTimerChangesNothing)
{
    CycleTimer timer{10_ms};
    static_cast<void>(timer.poll(at(0)));
    timer.start(at(7));  // as ROS start() of a started timer: no effect
    timer.start();
    EXPECT_EQ(timer.next(), Time::fromNanos(10'000'000));
    EXPECT_TRUE(timer.poll(at(10)));
}

TEST(CycleTimer, TimeGoingBackAnchorsTheGridAtTheNewTime)
{
    CycleTimer timer{10_ms};
    static_cast<void>(timer.poll(at(1000)));
    EXPECT_TRUE(timer.poll(at(1010)));

    // A backward step of the host wall clock: no tick, no burst, no freeze.
    EXPECT_FALSE(timer.poll(at(500)));
    EXPECT_EQ(timer.next(), Time::fromNanos(510'000'000));
    const CycleTimer::Tick tick = timer.poll(at(510));
    EXPECT_EQ(tick.periods, 1U);
    EXPECT_FALSE(tick.elapsed.has_value()) << "no negative elapsed time";
    EXPECT_EQ(timer.missedTotal(), 0U);
}

TEST(CycleTimer, APeriodOfNothingIsRefused)
{
    EXPECT_THROW(CycleTimer(Duration::fromNanos(0)), std::invalid_argument);
    EXPECT_THROW(CycleTimer(Duration::fromNanos(-1)), std::invalid_argument);
    EXPECT_THROW(CycleTimer(hypha::Hz(0.0)), std::invalid_argument);
    EXPECT_THROW(CycleTimer(hypha::Hz(-5.0)), std::invalid_argument);
}

// ---- The MRS port's Ticker, step by step ----

namespace {

/// The Ticker of the playground's MRS tracker port, as it is written there: a grid anchored
/// at the first poll, the count of instants in (previous poll, now], none on the first call.
class MrsTicker
{
  public:
    explicit MrsTicker(Duration period) : period(period)
    {
    }

    uint32_t poll(Time now)
    {
        if (!this->due) {
            this->due = now + this->period;
            return 0;
        }
        uint32_t ticks = 0;
        while (now >= *this->due) {
            this->due = *this->due + this->period;
            ++ticks;
        }
        return ticks;
    }

  private:
    Duration            period;
    std::optional<Time> due;
};

}  // namespace

TEST(CycleTimer, CountsThePeriodsOfTheMrsTickerAtEveryStep)
{
    // A jittered 100 Hz step with long stalls and early steps, from an odd start: the
    // timer's periods equal the Ticker's count at every one of 100000 steps.
    for (const Duration period : {10_ms, 100_ms, Duration::fromNanos(3'333'333)}) {
        std::mt19937_64                        random{42};
        std::uniform_int_distribution<int64_t> jitter{0, 20'000'000};
        std::uniform_int_distribution<int>     stall{0, 999};
        MrsTicker                              ticker{period};
        CycleTimer                             timer{period};
        int64_t                                nanos = 1'234'567'891;
        uint64_t                               periods = 0;
        for (int index = 0; index < 100'000; ++index) {
            nanos += jitter(random) + (stall(random) == 0 ? 750'000'000 : 0);
            const hypha::CycleTime time{.now = Time::fromNanos(nanos)};
            const uint32_t         expected = ticker.poll(time.now);
            const CycleTimer::Tick tick = timer.poll(time);
            ASSERT_EQ(tick.periods, expected) << "step " << index << " at " << nanos;
            periods += tick.periods;
        }
        EXPECT_GT(periods, 0U);
    }
}

// ---- StepHarness: the timer in a step, before the inputs are read ----

namespace {

/// A 100 Hz control loop with a 10 Hz status timer, and a gains timer that its mode starts
/// and stops (MRS Se3Controller's activate() and deactivate()).
class Controller final : public hypha::CycleNode
{
  public:
    using Record = std::tuple<int64_t, uint64_t, std::optional<int64_t>>;

    hypha::Latest<std_msgs::Float64> reference{*this, "reference"};
    hypha::Output<std_msgs::Float64> command{*this, "command"};
    hypha::Latest<std_msgs::Float64> active{*this, "active"};

    hypha::CycleTimer   status{hypha::Hz(10.0)};
    hypha::CycleTimer   gains{hypha::Hz(10.0)};
    std::vector<Record> statusTicks;  // (step, periods, elapsed) in ms
    std::vector<Record> gainsTicks;
    double              integrated{0.0};

    Controller()
    {
        this->gains.stop();  // started by activate()
    }

    void step(const hypha::CycleTime& time) override
    {
        if (const hypha::CycleTimer::Tick tick = this->status.poll(time)) {
            this->statusTicks.emplace_back(record(time, tick));
            this->integrated += static_cast<double>(tick.periods);
        }
        if (const hypha::CycleTimer::Tick tick = this->gains.poll(time)) {
            this->gainsTicks.emplace_back(record(time, tick));
        }
        const bool wantActive = this->active.usable() && this->active->data > 0.0;
        if (wantActive && !this->gains.running()) {
            this->gains.start(time);  // activate()
        } else if (!wantActive && this->gains.running()) {
            this->gains.stop();  // deactivate()
        }
        if (this->reference.usable()) {
            this->command.send({.data = this->reference->data + this->integrated});
        }
    }

  private:
    static Record record(const hypha::CycleTime& time, const hypha::CycleTimer::Tick& tick)
    {
        std::optional<int64_t> elapsed;
        if (tick.elapsed) {
            elapsed = tick.elapsed->nanos() / 1'000'000;
        }
        return {millisOf(time.now), tick.periods, elapsed};
    }
};

struct ControllerRun
{
    std::vector<Controller::Record> status;
    std::vector<Controller::Record> gains;
    std::vector<double>             sent;
};

/// A 100 Hz step from 0 to 1 s with one step 350 ms late, activated from 300 ms to 700 ms.
ControllerRun runController()
{
    Controller                  controller;
    hypha::testing::StepHarness harness{controller};
    harness.set(controller.reference, std_msgs::Float64{.data = 100.0});
    ControllerRun run;
    int64_t       t = 0;
    while (t <= 1000) {
        harness.set(
            controller.active, std_msgs::Float64{.data = (t >= 300 && t < 700) ? 1.0 : 0.0}
        );
        harness.step(controller, at(t));
        for (const std_msgs::Float64& sent : harness.sent(controller.command)) {
            run.sent.push_back(sent.data);
        }
        t += (t == 410) ? 350 : 10;  // the step at 410 ms took 350 ms
    }
    run.status = controller.statusTicks;
    run.gains = controller.gainsTicks;
    return run;
}

}  // namespace

TEST(CycleTimerStepHarness, TheTimersTickOnTheirGridCountTheLongStepAndStartAndStop)
{
    const ControllerRun run = runController();
    using R = Controller::Record;
    EXPECT_EQ(
        run.status,
        (std::vector<R>{
            {100, 1, std::nullopt},
            {200, 1, 100},
            {300, 1, 100},
            {400, 1, 100},
            {760, 3, 360},  // 500, 600, 700: two periods missed by the long step
            {800, 1, 40},
            {900, 1, 100},
            {1000, 1, 100},
        })
    );
    // Started in the step at 300 ms, stopped in the step at 760 ms (the first at or after
    // 700): it ticks at 400 and catches 500 ... 700 in the long step's successor.
    EXPECT_EQ(run.gains, (std::vector<R>{{400, 1, std::nullopt}, {760, 3, 360}}));
    ASSERT_FALSE(run.sent.empty());
    EXPECT_DOUBLE_EQ(run.sent.back(), 110.0) << "the step at 1000 ms sends after its tick";
}

TEST(CycleTimerStepHarness, TwoRunsAreEqual)
{
    const ControllerRun first = runController();
    const ControllerRun second = runController();
    EXPECT_EQ(first.status, second.status);
    EXPECT_EQ(first.gains, second.gains);
    EXPECT_EQ(first.sent, second.sent);
}

// ---- On the bus ----

namespace {

/// A 30 Hz rate cycle with a 10 Hz timer: periods that do not divide.
class Divider final : public hypha::CycleNode
{
  public:
    hypha::CycleTimer                        timer{hypha::Hz(10.0)};
    std::vector<std::pair<int64_t, int64_t>> ticks;  // (step time, newest instant)
    uint64_t                                 periods{0};
    std::optional<Time>                      first;
    std::optional<Time>                      last;

    void setup(hypha::Node&) override
    {
        this->setRate(hypha::Hz(30.0));
    }

    void step(const hypha::CycleTime& time) override
    {
        if (const hypha::CycleTimer::Tick tick = this->timer.poll(time)) {
            this->ticks.emplace_back(time.now.nanos(), tick.expected.nanos());
            this->periods += tick.periods;
        }
        if (!this->first) {
            this->first = time.now;
        }
        this->last = time.now;
    }
};

class CycleTimerBus : public ::testing::Test
{
  protected:
    void TearDown() override
    {
        static_cast<void>(hypha::disableSimTime());
    }
};

/// One run under simulated time from 1 s: the clock moves by @p step until @p until.
std::vector<std::pair<int64_t, int64_t>> simulatedRun(Duration step, Duration until)
{
    EXPECT_TRUE(hypha::enableSimTime(Time::fromNanos(1'000'000'000)));
    std::vector<std::pair<int64_t, int64_t>> ticks;
    {
        hypha::Node node("cycle_timer_sim_" + std::to_string(step.nanos()));
        Divider     divider;
        divider.setup(node);
        EXPECT_TRUE(divider.start(node).has_value());
        Duration moved{};
        while (moved < until) {
            hypha_test::advance(step);
            moved += step;
            EXPECT_TRUE(node.runFor(1_ms).ok());
        }
        for (int turn = 0; turn < 20; ++turn) {
            EXPECT_TRUE(node.runFor(1_ms).ok());
        }
        ticks = divider.ticks;
    }
    EXPECT_TRUE(hypha::disableSimTime());
    return ticks;
}

}  // namespace

TEST_F(CycleTimerBus, TwoSimulatedRunsGiveTheSameTicksHoweverTheClockMoves)
{
    const auto   fine = simulatedRun(10_ms, 3_s);
    const auto   coarse = simulatedRun(370_ms, 3_s);  // 11 cycle periods in one jump
    const size_t common = std::min(fine.size(), coarse.size());
    ASSERT_GE(common, 25U);
    for (size_t index = 0; index < common; ++index) {
        ASSERT_EQ(fine[index], coarse[index]) << "tick " << index;
    }
    // A replayed step runs each deadline, so every tick is one period after the previous
    // and runs in the first step at or after its instant.
    for (size_t index = 1; index < common; ++index) {
        EXPECT_EQ(fine[index].second - fine[index - 1].second, 100'000'000);
        EXPECT_GE(fine[index].first, fine[index].second);
        EXPECT_LT(fine[index].first - fine[index].second, 33'333'334);
    }
}

TEST_F(CycleTimerBus, OnHostTimeEveryPeriodOfTheGridIsCounted)
{
    hypha::Node node("cycle_timer_host");
    Divider     divider;
    divider.setup(node);
    ASSERT_TRUE(divider.start(node).has_value());
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(30);
    while (divider.ticks.size() < 5 && std::chrono::steady_clock::now() < deadline) {
        ASSERT_TRUE(node.runFor(50_ms).ok());
    }
    ASSERT_GE(divider.ticks.size(), 5U);
    ASSERT_TRUE(divider.first.has_value());
    for (const auto& [stepAt, instant] : divider.ticks) {
        EXPECT_GE(stepAt, instant) << "a tick runs at or after its instant";
        EXPECT_EQ((instant - divider.first->nanos()) % 100'000'000, 0)
            << "on the grid of the first step";
    }
    // Every instant of the grid up to the last step is in a tick's count.
    const int64_t span = divider.last->nanos() - divider.first->nanos();
    EXPECT_EQ(static_cast<int64_t>(divider.periods), span / 100'000'000);
}
