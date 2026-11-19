/// The simulated twin. Runs on the /clock of hypha_sim_clock: three cycles at
/// 10, 50 and 100 Hz and one timer every 20 ms. Each tick is written to stdout
/// as a trace line, and a summary per stream follows when the run stops.
#include <hypha/cycle_runtime.h>
#include <hypha/hypha.h>

#include <cstdint>
#include <cstdio>
#include <deque>
#include <exception>
#include <expected>
#include <string>

namespace {

/// Counts of one stream of ticks: a cycle or the timer.
struct StreamStats
{
    std::string name;
    uint64_t    ticks{0};
    int64_t     firstNanos{0};
    int64_t     lastNanos{0};
    /// Last cumulative missed count seen. The framework counts missed periods
    /// cumulatively and modulo 2^64, so each tick records the difference.
    uint64_t lastMissedTotal{0};
    uint64_t missed{0};
};

/// Counts one tick and prints its trace line. `missedTotal` is the cumulative
/// missed-period count the framework reports.
void recordTick(StreamStats& stats, int64_t nanos, uint64_t missedTotal)
{
    if (stats.ticks == 0) {
        stats.firstNanos = nanos;
    }
    stats.lastNanos = nanos;
    const uint64_t missed = missedTotal - stats.lastMissedTotal;
    stats.lastMissedTotal = missedTotal;
    stats.missed += missed;
    std::printf(
        "trace,%s,%llu,%lld,%llu\n",
        stats.name.c_str(),
        static_cast<unsigned long long>(stats.ticks),
        static_cast<long long>(nanos),
        static_cast<unsigned long long>(missed)
    );
    ++stats.ticks;
}

struct CycleTick
{
    StreamStats* stats;

    void operator()(const hypha::CycleTime& time) const
    {
        recordTick(*this->stats, time.now.nanos(), time.missed);
    }
};

/// The timer callback. It stops the run once simulated time reaches the stop time.
struct TimerTick
{
    StreamStats*        stats;
    const hypha::Timer* timer;
    int64_t             stopNanos;

    void operator()() const
    {
        const int64_t nanos = hypha::now().nanos();
        recordTick(*this->stats, nanos, this->timer->missedCount());
        if (nanos >= this->stopNanos) {
            hypha::requestStop();
        }
    }
};

/// Starts one cycle. Returns false after printing why it did not start.
bool startCycle(hypha::Cycle& cycle, hypha::Node& node, double rateHz, StreamStats& stats)
{
    cycle.setRate(hypha::Hz(rateHz));
    const std::expected<void, std::string> started = cycle.start(node, CycleTick{&stats});
    if (!started.has_value()) {
        std::fprintf(stderr, "sim_twin: %s\n", started.error().c_str());
        return false;
    }
    return true;
}

void printSummary(const StreamStats& stats)
{
    std::printf(
        "summary,%s,ticks=%llu,first_ns=%lld,last_ns=%lld,missed=%llu\n",
        stats.name.c_str(),
        static_cast<unsigned long long>(stats.ticks),
        static_cast<long long>(stats.firstNanos),
        static_cast<long long>(stats.lastNanos),
        static_cast<unsigned long long>(stats.missed)
    );
}

}  // namespace

int main(int argc, char** argv)
{
    if (argc != 2) {
        std::fprintf(stderr, "usage: hypha_sim_twin <parameter file>\n");
        return 2;
    }
    try {
        hypha::Node   node("sim_twin", argv[1]);
        const int64_t stopNanos = hypha::config().required<int64_t>("stop_sim_ns");

        std::deque<StreamStats> streams;
        streams.push_back(StreamStats{.name = "cycle_10hz"});
        streams.push_back(StreamStats{.name = "cycle_50hz"});
        streams.push_back(StreamStats{.name = "cycle_100hz"});
        streams.push_back(StreamStats{.name = "timer_20ms"});
        StreamStats& timerStats = streams[3];

        hypha::Cycle cycle10("twin_10hz");
        hypha::Cycle cycle50("twin_50hz");
        hypha::Cycle cycle100("twin_100hz");
        const bool   started = startCycle(cycle10, node, 10.0, streams[0])
                            && startCycle(cycle50, node, 50.0, streams[1])
                            && startCycle(cycle100, node, 100.0, streams[2]);
        if (!started) {
            return 1;
        }

        hypha::Timer timer;
        timer =
            node.timer(hypha::Duration::fromMillis(20), TimerTick{&timerStats, &timer, stopNanos});
        std::printf("sim_twin: ready, stop at %lld ns\n", static_cast<long long>(stopNanos));
        std::fflush(stdout);
        const hypha::RunResult result = node.run();
        for (const StreamStats& stats : streams) {
            printSummary(stats);
        }
        return result.ok() ? 0 : 1;
    } catch (const std::exception& error) {
        std::fprintf(stderr, "sim_twin: %s\n", error.what());
        return 1;
    }
}
