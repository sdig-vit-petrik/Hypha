/// A transport benchmark. It names no backend, so it measures what an
/// application sees through the SDK. Latency is now() minus the send time each
/// publisher writes as the first 8 bytes of the loaned buffer.
///
///   <backend>_bench [duration_seconds]     default 10, must be a finite number above 0

#include "hypha/hypha.h"

#include <algorithm>
#include <atomic>
#include <cinttypes>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <expected>
#include <functional>
#include <optional>
#include <string>
#include <thread>
#include <vector>

namespace {

struct TopicLoad
{
    std::string topic;
    size_t      payloadBytes;
    double      rateHz;
};

const std::vector<TopicLoad> WORKLOAD = {
    {"/bench/point_cloud", 2U * 1024 * 1024, 10.0},
    {"/bench/image", 2U * 1024 * 1024, 30.0},
    {"/bench/control", 100, 1000.0},
};

struct TopicStats
{
    std::vector<double> latenciesUs;
    uint64_t            delivered{0};
    uint64_t            shortFrames{0};
    uint64_t            loanRefused{0};
    uint64_t            publishFailed{0};
};

void writeSendTime(uint8_t* buffer, size_t size)
{
    const int64_t nanos = hypha::now().nanos();
    std::memcpy(buffer, &nanos, sizeof(nanos));
    if (size > sizeof(nanos)) {
        std::memset(buffer + sizeof(nanos), 0x42, size - sizeof(nanos));
    }
}

double readLatencyUs(const hypha::RawSample& sample)
{
    int64_t sentNanos = 0;
    std::memcpy(&sentNanos, sample.bytes.data(), sizeof(sentNanos));
    const int64_t nowNanos = hypha::now().nanos();
    return static_cast<double>(nowNanos - sentNanos) / 1000.0;
}

/// Records one latency per frame. A frame too short to hold the send time is
/// counted and skipped. A named functor: the subscriber keeps it.
struct LatencyRecorder
{
    explicit LatencyRecorder(TopicStats& targetStats) : stats(targetStats)
    {
    }

    void operator()(const hypha::RawSample& sample)
    {
        const bool headerFits = sample.bytes.size() >= sizeof(int64_t);
        if (!headerFits) {
            ++this->stats.shortFrames;
            if (this->stats.shortFrames == 1) {
                std::fprintf(
                    stderr,
                    "bench: short frame of %zu bytes skipped (later ones only counted)\n",
                    sample.bytes.size()
                );
            }
            return;
        }
        this->stats.latenciesUs.push_back(readLatencyUs(sample));
        ++this->stats.delivered;
    }

  private:
    TopicStats& stats;
};

/// Publishes one frame per timer period. A loan the pool refuses and a publish
/// the transport refuses are counted, so the table shows lost frames. A named
/// functor: the timer keeps it for its whole life.
class BenchTick
{
  public:
    BenchTick(hypha::RawPublisher rawPublisher, size_t numPayloadBytes, TopicStats& targetStats)
        : publisher(std::move(rawPublisher)), payloadBytes(numPayloadBytes), stats(targetStats)
    {
    }

    void operator()()
    {
        std::expected<hypha::LoanedBuffer, hypha::Status> loaned =
            this->publisher.loan(this->payloadBytes);
        const bool loanRefused = !loaned.has_value();
        if (loanRefused) {
            ++this->stats.loanRefused;
            return;
        }
        writeSendTime(loaned->data(), loaned->size());
        const hypha::Status sendStatus = this->publisher.publishLoan(std::move(*loaned));
        const bool          sendOk = sendStatus == hypha::Status::Ok;
        if (!sendOk) {
            ++this->stats.publishFailed;
        }
    }

  private:
    hypha::RawPublisher publisher;
    size_t              payloadBytes;
    TopicStats&         stats;
};

double percentile(std::vector<double> sorted, double fraction)
{
    if (sorted.empty()) {
        return 0.0;
    }
    const size_t index = static_cast<size_t>(fraction * static_cast<double>(sorted.size() - 1));
    return sorted[index];
}

void report(const TopicLoad& load, TopicStats& stats, double durationSeconds)
{
    std::sort(stats.latenciesUs.begin(), stats.latenciesUs.end());
    const double expected = load.rateHz * durationSeconds;
    std::printf(
        "%-20s  n=%6" PRIu64
        " / expected~%6.0f   p50=%8.1fus  p99=%8.1fus  p99.9=%8.1fus  max=%8.1fus\n",
        load.topic.c_str(),
        stats.delivered,
        expected,
        percentile(stats.latenciesUs, 0.50),
        percentile(stats.latenciesUs, 0.99),
        percentile(stats.latenciesUs, 0.999),
        stats.latenciesUs.empty() ? 0.0 : stats.latenciesUs.back()
    );
}

/// Prints the latency table and the per-topic loss counters. Called only after
/// a run that completed, so the numbers are never shown as complete otherwise.
void printResults(
    std::vector<TopicStats>& stats, const std::vector<hypha::Timer>& timers, double durationSeconds
)
{
    std::printf(
        "\n%-20s  %-30s  %-12s  %-12s  %-12s  %-12s\n",
        "topic",
        "delivered/expected",
        "p50",
        "p99",
        "p99.9",
        "max"
    );
    for (size_t i = 0; i < WORKLOAD.size(); ++i) {
        report(WORKLOAD[i], stats[i], durationSeconds);
        std::printf("  timer missed periods: %" PRIu64 "\n", timers[i].missedCount());
        std::printf("  short frames skipped: %" PRIu64 "\n", stats[i].shortFrames);
        std::printf(
            "  loans refused: %" PRIu64 "  publishes failed: %" PRIu64 "\n",
            stats[i].loanRefused,
            stats[i].publishFailed
        );
    }
}

/// Runs the node on a thread this program created. It keeps the outcome of
/// run() and sets a flag when run() returns. The main thread reads the outcome
/// only after join().
class RunTask
{
  public:
    explicit RunTask(hypha::Node& runNode) : node(runNode)
    {
    }

    void operator()()
    {
        this->outcome = this->node.run();
        this->done.store(true, std::memory_order_release);
    }

    bool finished() const
    {
        return this->done.load(std::memory_order_acquire);
    }

    const hypha::RunResult& result() const
    {
        return this->outcome;
    }

  private:
    hypha::Node&      node;
    hypha::RunResult  outcome;
    std::atomic<bool> done{false};
};

/// @return The duration in seconds, or no value when the text is not a finite
///         number above zero.
std::optional<double> parseDuration(const char* text)
{
    char*        end = nullptr;
    const double value = std::strtod(text, &end);
    const bool   wholeTextParsed = end != text && *end == '\0';
    if (!wholeTextParsed) {
        return std::nullopt;
    }
    const bool finite = std::isfinite(value);
    const bool positive = value > 0.0;
    const bool usable = finite && positive;
    if (!usable) {
        return std::nullopt;
    }
    return value;
}

}  // namespace

int runBench(hypha::Node& node, double durationSeconds)
{
    std::vector<hypha::RawPublisher>  publishers;
    std::vector<hypha::RawSubscriber> subscribers;
    std::vector<hypha::Timer>         timers;
    std::vector<TopicStats>           stats(WORKLOAD.size());

    for (size_t i = 0; i < WORKLOAD.size(); ++i) {
        const TopicLoad& load = WORKLOAD[i];

        hypha::RawPublisher publisher = hypha::require(
            hypha::rawPublisher(
                load.topic,
                {.dataType = "hypha_bench/Payload", .md5 = std::string(32, '0'), .definition = ""},
                hypha::maxPayloadDynamic(load.payloadBytes)
            ),
            "benchmark publisher"
        );
        publishers.push_back(publisher);

        subscribers.push_back(
            hypha::require(
                hypha::subscribeRaw(load.topic, LatencyRecorder(stats[i])), "benchmark subscriber"
            )
        );

        timers.push_back(node.timer(
            hypha::Duration::fromSeconds(1.0 / load.rateHz),
            BenchTick(publisher, load.payloadBytes, stats[i])
        ));
    }

    std::printf("running for %.1fs against %zu topics...\n", durationSeconds, WORKLOAD.size());

    // run() owns the wait. The main thread times the run and asks it to stop. It stops early
    // when the run ends by itself, so a failed run does not produce numbers.
    RunTask           runTask(node);
    std::thread       runner(std::ref(runTask));
    const hypha::Time start = hypha::now();
    for (;;) {
        const bool runEnded = runTask.finished();
        const bool timeUp = (hypha::now() - start).seconds() >= durationSeconds;
        const bool stopWaiting = runEnded || timeUp;
        if (stopWaiting) {
            break;
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(50));
    }
    hypha::requestStop();
    runner.join();
    const bool runOk = runTask.result().ok();
    if (runOk) {
        printResults(stats, timers, durationSeconds);
    } else {
        std::fprintf(
            stderr,
            "the run failed: stop reason %d, status %d; no results printed\n",
            static_cast<int>(runTask.result().reason()),
            runTask.result().status().asInt()
        );
    }

    return runOk ? 0 : 1;
}

int main(int argc, char** argv)
{
    double durationSeconds = 10.0;
    if (argc > 1) {
        const std::optional<double> parsed = parseDuration(argv[1]);
        if (!parsed.has_value()) {
            std::fprintf(
                stderr,
                "hypha_bench: duration must be a finite number above 0, got '%s'\n"
                "usage: hypha_bench [duration_seconds]\n",
                argv[1]
            );
            return 2;
        }
        durationSeconds = *parsed;
    }

    try {
        hypha::Node node("hypha_bench");
        return runBench(node, durationSeconds);
    } catch (const std::exception& error) {
        std::fprintf(stderr, "hypha_bench: %s\n", error.what());
        return 1;
    }
}
