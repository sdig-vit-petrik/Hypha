#pragma once

/// @file Small helpers shared by the benchmark example nodes: a latency
///       percentile and a CPU/context-switch snapshot from getrusage().

#include <sys/resource.h>
#include <sys/time.h>

#include <chrono>
#include <cstdint>
#include <vector>

namespace hypha_bench {

/// @return Wall-clock nanoseconds since the Unix epoch. Latency across two
///         processes is measured on this clock, not on simulated node time.
inline int64_t wallNowNanos()
{
    const auto sinceEpoch = std::chrono::system_clock::now().time_since_epoch();
    return std::chrono::duration_cast<std::chrono::nanoseconds>(sinceEpoch).count();
}

/// @param sorted Values in nanoseconds, already sorted ascending.
inline int64_t percentileNs(const std::vector<int64_t>& sorted, double fraction)
{
    if (sorted.empty()) {
        return 0;
    }
    const size_t index = static_cast<size_t>(fraction * static_cast<double>(sorted.size() - 1));
    return sorted[index];
}

/// @param sorted Latencies in nanoseconds, already sorted ascending.
inline int64_t percentileUs(std::vector<int64_t>& sorted, double fraction)
{
    return percentileNs(sorted, fraction) / 1000;
}

struct RusageSample
{
    double  userSeconds{0.0};
    double  systemSeconds{0.0};
    int64_t voluntaryContextSwitches{0};  // this thread chose to block (a wait, a lock, I/O)
    int64_t involuntaryContextSwitches{0};  // the scheduler preempted this thread
};

/// @return CPU time and context switches for the calling process so far. Two
///         calls bracketing a run give the cost of that run by subtraction.
inline RusageSample sampleRusage()
{
    struct rusage usage{};
    ::getrusage(RUSAGE_SELF, &usage);
    RusageSample sample;
    sample.userSeconds = static_cast<double>(usage.ru_utime.tv_sec) + usage.ru_utime.tv_usec / 1e6;
    sample.systemSeconds =
        static_cast<double>(usage.ru_stime.tv_sec) + usage.ru_stime.tv_usec / 1e6;
    sample.voluntaryContextSwitches = usage.ru_nvcsw;
    sample.involuntaryContextSwitches = usage.ru_nivcsw;
    return sample;
}

}  // namespace hypha_bench
