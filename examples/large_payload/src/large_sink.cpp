/// Receives what large_source publishes and reports latency and loss.
///
///   large_sink --expect 2000
///
/// The loop does not sleep: a poll interval would dominate the number this
/// program exists to measure.

#include <hypha/hypha.h>

#include "bench_util.h"
#include "large_frame_qos.h"

#include <algorithm>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <functional>
#include <string>
#include <vector>

namespace {

/// Counts each frame and records its latency. A named functor: the subscriber
/// keeps it for its whole life.
struct FrameStats
{
    FrameStats(uint64_t warmupCount, std::vector<int64_t>& latencies)
        : warmup(warmupCount), latencyNs(latencies)
    {
    }

    void operator()(const hypha::RawSample& sample)
    {
        const int64_t arrivedAt = hypha_bench::wallNowNanos();

        // A frame shorter than the header is not a sample. Count it and skip
        // it before the header is read.
        const size_t headerBytes = sizeof(uint32_t) + sizeof(int64_t);
        const bool   headerFits = sample.bytes.size() >= headerBytes;
        if (!headerFits) {
            ++this->shortFrames;
            HYPHA_LOG_WARN_THROTTLE(
                1.0, "large_sink: short frame of {} bytes skipped", sample.bytes.size()
            );
            return;
        }

        uint32_t sequence = 0;
        int64_t  sentAt = 0;
        std::memcpy(&sequence, sample.bytes.data(), sizeof(sequence));
        std::memcpy(&sentAt, sample.bytes.data() + sizeof(sequence), sizeof(sentAt));

        // Sequence numbers wrap, so the gap is taken modulo 2^32.
        const bool skipped = this->hasLastSeen && sequence != this->lastSeen + 1;
        if (skipped) {
            this->gaps += sequence - this->lastSeen - 1;
        }
        this->lastSeen = sequence;
        this->hasLastSeen = true;
        this->frameBytes = sample.bytes.size();

        const bool pastWarmup = this->received >= this->warmup;
        if (pastWarmup) {
            this->latencyNs.push_back(arrivedAt - sentAt);
        }
        ++this->received;
    }

    uint64_t              warmup;
    std::vector<int64_t>& latencyNs;
    uint64_t              received = 0;
    bool                  hasLastSeen = false;
    uint32_t              lastSeen = 0;
    uint64_t              gaps = 0;
    uint64_t              shortFrames = 0;
    uint64_t              frameBytes = 0;
};

}  // namespace

int runSink(hypha::Node& node, uint64_t warmup, uint64_t expectedFrames)
{
    std::vector<int64_t> latencyNs;
    latencyNs.reserve(static_cast<size_t>(expectedFrames));

    FrameStats stats(warmup, latencyNs);
    auto       subscriber = hypha::require(
        // Depth zero: the deepest queue the policy allows.
        hypha::subscribeRawOfType(
            "/large_frames", "hypha/LargeFrame", std::ref(stats), 0, hypha_bench::LARGE_FRAME_QOS
        ),
        "large frame subscriber"
    );

    const int64_t deadline = hypha_bench::wallNowNanos() + 60'000'000'000;
    for (;;) {
        const bool enoughFrames = stats.received >= expectedFrames;
        const bool deadlinePassed = hypha_bench::wallNowNanos() >= deadline;
        const bool stopNow = !hypha::ok() || enoughFrames || deadlinePassed;
        if (stopNow) {
            break;
        }
        const hypha::RunResult turn = node.runFor(hypha::Duration::fromMillis(1));
        if (!turn.ok()) {
            HYPHA_LOG_ERROR(
                "the executor failed with status {}", static_cast<int>(turn.status().asInt())
            );
            return 1;
        }
    }

    std::sort(latencyNs.begin(), latencyNs.end());
    HYPHA_LOG_INFO(
        "large_sink: received={} gaps={} short_frames={} bytes={} p50={}us p99={}us "
        "p999={}us",
        stats.received,
        stats.gaps,
        stats.shortFrames,
        stats.frameBytes,
        hypha_bench::percentileUs(latencyNs, 0.50),
        hypha_bench::percentileUs(latencyNs, 0.99),
        hypha_bench::percentileUs(latencyNs, 0.999)
    );

    return stats.received > 0 ? 0 : 1;
}

int main(int argc, char** argv)
{
    int            expected = 2000;
    const uint64_t warmup = 50;
    for (int i = 1; i + 1 < argc; i += 2) {
        if (std::string(argv[i]) == "--expect") {
            expected = std::atoi(argv[i + 1]);
        }
    }
    // A non-positive --expect means no frame is awaited. The run then ends at once.
    const uint64_t expectedFrames = expected > 0 ? static_cast<uint64_t>(expected) : 0;

    try {
        hypha::Node node("large_sink");
        return runSink(node, warmup, expectedFrames);
    } catch (const std::exception& error) {
        HYPHA_LOG_ERROR("large_sink: {}", error.what());
        return 1;
    }
}
