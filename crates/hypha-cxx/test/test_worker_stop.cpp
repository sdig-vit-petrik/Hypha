// What a worker thread of an ApplicationNode has to end at the node's stop (#263): a sleep
// that returns early and says so, and a rate on a grid. The worker sleeps for ten seconds;
// every test here must be over within a few.

#include "hypha/hypha.h"
#include "hypha/worker.h"

#include <gtest/gtest.h>

#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <stdexcept>
#include <stop_token>
#include <string>
#include <thread>
#include <utility>

namespace {

using Clock = std::chrono::steady_clock;
using std::chrono::milliseconds;
using std::chrono::seconds;

/// The time from @p start until now, in milliseconds.
int64_t elapsedMillis(Clock::time_point start)
{
    return std::chrono::duration_cast<milliseconds>(Clock::now() - start).count();
}

/// The longest a stop may take to reach a worker, on a loaded machine.
constexpr int64_t kStopBoundMillis = 1500;

/// Runs @p runner as a process would, on the monotonic clock, with no parameter file.
int runWithoutArguments(int (*runner)(int, char**, std::string))
{
    EXPECT_TRUE(hypha::disableSimTime());
    char  program[] = "prog";
    char* argv[] = {program, nullptr};
    return runner(1, argv, "worker_stop_app");
}

/// Joins its thread at destruction, before the application's other members go.
class Worker
{
  public:
    Worker() = default;
    Worker(const Worker&) = delete;
    Worker& operator=(const Worker&) = delete;

    ~Worker()
    {
        if (this->thread_.joinable()) {
            this->thread_.join();
        }
    }

    void start(std::thread&& thread)
    {
        this->thread_ = std::move(thread);
    }

  private:
    std::thread thread_;
};

/// Asks the node to stop from a thread, after a delay.
struct StopAfter
{
    milliseconds delay;

    void operator()() const
    {
        std::this_thread::sleep_for(this->delay);
        hypha::requestStop();
    }
};

/// The result of the one worker of a test application. Reset by each test.
struct WorkerResult
{
    std::atomic<bool>    started{false};
    std::atomic<bool>    slept{true};
    std::atomic<int64_t> sleepMillis{-1};
    std::atomic<int>     ticks{0};
    std::atomic<int64_t> missed{-1};

    void reset()
    {
        this->started = false;
        this->slept = true;
        this->sleepMillis = -1;
        this->ticks = 0;
        this->missed = -1;
    }
};

WorkerResult gResult;

/// A worker that sleeps ten seconds on the node's stop; the node is stopped from outside.
class SleepingWorkerApp final : public hypha::ApplicationNode
{
  public:
    void started(hypha::Node&) override
    {
        this->worker_.start(std::thread(Sleep{this->stopToken()}));
        this->stopper_.start(std::thread(StopAfter{milliseconds(300)}));
    }

  private:
    struct Sleep
    {
        std::stop_token stop;

        void operator()() const
        {
            const Clock::time_point start = Clock::now();
            gResult.started = true;
            gResult.slept = hypha::sleepFor(this->stop, seconds(10));
            gResult.sleepMillis = elapsedMillis(start);
        }
    };

    Worker worker_;
    Worker stopper_;
};

/// A worker that loops on a rate until the node stops.
class RateWorkerApp final : public hypha::ApplicationNode
{
  public:
    void started(hypha::Node&) override
    {
        this->worker_.start(std::thread(Loop{this->stopToken()}));
        this->stopper_.start(std::thread(StopAfter{milliseconds(550)}));
    }

  private:
    struct Loop
    {
        std::stop_token stop;

        void operator()() const
        {
            hypha::Rate rate(20.0, this->stop);
            gResult.started = true;
            do {
                ++gResult.ticks;
            } while (rate.sleep());
            gResult.missed = static_cast<int64_t>(rate.missedCount());
        }
    };

    Worker worker_;
    Worker stopper_;
};

/// Starts a sleeping worker in setup(), then refuses start-up: nothing stops the node
/// but the failure, and the destructor joins the worker.
class SetupFailsAfterWorkerApp final : public hypha::ApplicationNode
{
  public:
    void setup(hypha::Node&) override
    {
        this->worker_.start(std::thread(Sleep{this->stopToken()}));
        throw std::runtime_error("setup refused");
    }

  private:
    struct Sleep
    {
        std::stop_token stop;

        void operator()() const
        {
            gResult.started = true;
            gResult.slept = hypha::sleepFor(this->stop, seconds(10));
        }
    };

    Worker worker_;
};

/// A worker that fails the run itself, then sleeps: the failure is the stop.
class FailingWorkerApp final : public hypha::ApplicationNode
{
  public:
    void started(hypha::Node&) override
    {
        this->worker_.start(std::thread(Fail{this, this->stopToken()}));
    }

  private:
    struct Fail
    {
        FailingWorkerApp* app;
        std::stop_token   stop;

        void operator()() const
        {
            this->app->failRun("the device is gone");
            gResult.slept = hypha::sleepFor(this->stop, seconds(10));
        }
    };

    Worker worker_;
};

}  // namespace

TEST(WorkerStop, ASleepOfTenSecondsEndsWhenTheNodeIsStopped)
{
    gResult.reset();
    const Clock::time_point start = Clock::now();
    const int               exitCode = runWithoutArguments(&hypha::runNode<SleepingWorkerApp>);
    EXPECT_EQ(exitCode, hypha::kExitClean);
    EXPECT_LT(elapsedMillis(start), kStopBoundMillis);
    EXPECT_TRUE(gResult.started);
    EXPECT_FALSE(gResult.slept) << "the sleep must say that the stop ended it";
    EXPECT_LT(gResult.sleepMillis, kStopBoundMillis);
    EXPECT_GE(gResult.sleepMillis, 250) << "the sleep ended before the stop was requested";
}

TEST(WorkerStop, ALoopOnARateEndsWhenTheNodeIsStopped)
{
    gResult.reset();
    const Clock::time_point start = Clock::now();
    const int               exitCode = runWithoutArguments(&hypha::runNode<RateWorkerApp>);
    EXPECT_EQ(exitCode, hypha::kExitClean);
    EXPECT_LT(elapsedMillis(start), kStopBoundMillis);
    // 20 Hz for about 550 ms: the first pass at once, then one each 50 ms.
    EXPECT_GE(gResult.ticks, 8);
    EXPECT_LE(gResult.ticks, 13);
    EXPECT_EQ(gResult.missed, 0);
}

TEST(WorkerStop, AFailedStartUpStopsTheWorkerBeforeTheApplicationIsDestroyed)
{
    gResult.reset();
    const Clock::time_point start = Clock::now();
    const int exitCode = runWithoutArguments(&hypha::runNode<SetupFailsAfterWorkerApp>);
    EXPECT_NE(exitCode, hypha::kExitClean);
    EXPECT_LT(elapsedMillis(start), kStopBoundMillis);
    EXPECT_FALSE(gResult.slept);
}

TEST(WorkerStop, AFailRunFromTheWorkerEndsItsSleep)
{
    gResult.reset();
    const Clock::time_point start = Clock::now();
    const int               exitCode = runWithoutArguments(&hypha::runNode<FailingWorkerApp>);
    EXPECT_EQ(exitCode, hypha::kExitRuntime);
    EXPECT_LT(elapsedMillis(start), kStopBoundMillis);
    EXPECT_FALSE(gResult.slept);
}

TEST(WorkerStop, ASleepOnAStopThatCameBeforeReturnsAtOnce)
{
    std::stop_source source;
    source.request_stop();
    const Clock::time_point start = Clock::now();
    EXPECT_FALSE(hypha::sleepFor(source.get_token(), seconds(10)));
    EXPECT_FALSE(hypha::sleepFor(source.get_token(), hypha::Duration::fromSeconds(10.0)));
    EXPECT_FALSE(hypha::sleepUntil(source.get_token(), Clock::now() + seconds(10)));
    EXPECT_LT(elapsedMillis(start), 200);
}

TEST(WorkerStop, ASleepThatIsNotStoppedSleepsTheWholeTimeAndSaysSo)
{
    std::stop_source        source;
    const Clock::time_point start = Clock::now();
    EXPECT_TRUE(hypha::sleepFor(source.get_token(), milliseconds(120)));
    EXPECT_GE(elapsedMillis(start), 119);
    EXPECT_TRUE(hypha::sleepFor(source.get_token(), hypha::Duration::fromMillis(30)));
    EXPECT_TRUE(hypha::sleepUntil(source.get_token(), Clock::now() + milliseconds(30)));
    // A span that is not positive is over at once and is no stop.
    EXPECT_TRUE(hypha::sleepFor(source.get_token(), milliseconds(0)));
    EXPECT_TRUE(hypha::sleepFor(source.get_token(), milliseconds(-5)));
}

TEST(WorkerStop, ASleepIsEndedByAStopRequestedFromAnotherThread)
{
    std::stop_source        source;
    std::thread             stopper([&source] {
        std::this_thread::sleep_for(milliseconds(100));
        source.request_stop();
    });
    const Clock::time_point start = Clock::now();
    EXPECT_FALSE(hypha::sleepFor(source.get_token(), seconds(10)));
    EXPECT_LT(elapsedMillis(start), kStopBoundMillis);
    stopper.join();
}

TEST(WorkerStop, ASleepOfAnEndlessSpanIsRefusedByNothingAndEndsAtTheStop)
{
    std::stop_source source;
    std::thread      stopper([&source] {
        std::this_thread::sleep_for(milliseconds(50));
        source.request_stop();
    });
    EXPECT_FALSE(hypha::sleepFor(source.get_token(), hypha::Duration::fromNanos(INT64_MAX)));
    EXPECT_FALSE(hypha::sleepFor(source.get_token(), std::chrono::hours(1000000)));
    stopper.join();
}

TEST(WorkerStop, ARateKeepsItsGridAndDoesNotDrift)
{
    std::stop_source source;
    hypha::Rate      rate(50.0, source.get_token());
    EXPECT_EQ(rate.period(), hypha::Duration::fromMillis(20));
    const Clock::time_point start = Clock::now();
    for (int i = 0; i < 10; ++i) {
        EXPECT_TRUE(rate.sleep());
        // A pass of work of 12 ms in each period, which must not stretch the period: a
        // sleep of a whole period after the work would take 10 x 32 ms.
        std::this_thread::sleep_for(milliseconds(12));
    }
    // Ten periods of 20 ms from the start, the last one's work included.
    EXPECT_GE(elapsedMillis(start), 200);
    EXPECT_LT(elapsedMillis(start), 290);
    EXPECT_EQ(rate.missedCount(), 0U);
}

TEST(WorkerStop, ARateCountsThePeriodsAPassOverranAndSkipsThem)
{
    std::stop_source source;
    hypha::Rate      rate(50.0, source.get_token());
    EXPECT_TRUE(rate.sleep());
    // The first tick was at 20 ms. 75 ms of work then ends at about 95 ms: the ticks at 40
    // and 60 ms are skipped and counted, the one at 80 ms is late and not counted.
    std::this_thread::sleep_for(milliseconds(75));
    const Clock::time_point start = Clock::now();
    EXPECT_TRUE(rate.sleep());
    EXPECT_LT(elapsedMillis(start), 10) << "an overrun returns at once, with no burst to follow";
    EXPECT_GE(rate.missedCount(), 2U);
    EXPECT_LE(rate.missedCount(), 3U);
    // The next wait is for the next grid point, not for a whole period from now.
    EXPECT_TRUE(rate.sleep());
    EXPECT_LE(elapsedMillis(start), 40);
}

TEST(WorkerStop, ARateEndsAtTheStopAndStaysEnded)
{
    std::stop_source        source;
    hypha::Rate             rate(2.0, source.get_token());
    std::thread             stopper([&source] {
        std::this_thread::sleep_for(milliseconds(100));
        source.request_stop();
    });
    const Clock::time_point start = Clock::now();
    EXPECT_FALSE(rate.sleep());
    EXPECT_LT(elapsedMillis(start), kStopBoundMillis);
    EXPECT_FALSE(rate.sleep());
    stopper.join();
}

TEST(WorkerStop, ARateResetStartsTheGridAgain)
{
    std::stop_source source;
    hypha::Rate      rate(20.0, source.get_token());
    std::this_thread::sleep_for(milliseconds(120));
    rate.reset();
    const Clock::time_point start = Clock::now();
    EXPECT_TRUE(rate.sleep());
    EXPECT_GE(elapsedMillis(start), 45);
    EXPECT_EQ(rate.missedCount(), 0U);
}

TEST(WorkerStop, ARateRefusesWhatIsNotAPeriod)
{
    const std::stop_token stop;
    EXPECT_THROW(hypha::Rate(0.0, stop), std::invalid_argument);
    EXPECT_THROW(hypha::Rate(-1.0, stop), std::invalid_argument);
    EXPECT_THROW(hypha::Rate(std::nan(""), stop), std::invalid_argument);
    EXPECT_THROW(hypha::Rate(1e12, stop), std::invalid_argument);
    EXPECT_THROW(hypha::Rate(1e-9, stop), std::invalid_argument);
    EXPECT_THROW(hypha::Rate(hypha::Duration::fromNanos(0), stop), std::invalid_argument);
    EXPECT_THROW(hypha::Rate(hypha::Duration::fromMillis(-3), stop), std::invalid_argument);
    EXPECT_NO_THROW(hypha::Rate(hypha::Duration::fromMillis(3), stop));
}
