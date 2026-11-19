/// Publishes large frames and reports what the publish path itself costs.
///
///   large_source --size-mb 2 --rate 200 --count 2000

#include <hypha/hypha.h>

#include "bench_util.h"
#include "large_frame_qos.h"

#include <algorithm>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

namespace {

struct Options
{
    size_t sizeBytes{2U * 1024 * 1024};
    double rateHz{200.0};
    int    count{2000};
    int    warmup{50};
};

Options parseOptions(int argc, char** argv)
{
    Options options;
    for (int i = 1; i + 1 < argc; i += 2) {
        const std::string flag = argv[i];
        if (flag == "--size-mb") {
            options.sizeBytes = static_cast<size_t>(std::atoi(argv[i + 1])) * 1024 * 1024;
        } else if (flag == "--size-kb") {
            options.sizeBytes = static_cast<size_t>(std::atoi(argv[i + 1])) * 1024;
        } else if (flag == "--rate") {
            options.rateHz = std::atof(argv[i + 1]);
        } else if (flag == "--count") {
            options.count = std::atoi(argv[i + 1]);
        }
    }
    return options;
}

}  // namespace

/// One publish per timer period, with the cost of each stage recorded.
struct Source
{
    hypha::Node&        node;
    const Options&      options;
    hypha::RawPublisher publisher;

    std::vector<int64_t> loanNs;
    std::vector<int64_t> fillNs;
    std::vector<int64_t> publishNs;

    uint64_t sent{0};
    uint64_t refused{0};
    uint64_t failed{0};
    int      sequence{0};

    void step()
    {
        if (this->sequence >= this->options.count) {
            hypha::requestStop();
            return;
        }
        const int     current = this->sequence++;
        const int64_t loanStarted = hypha_bench::wallNowNanos();

        std::expected<hypha::LoanedBuffer, hypha::Status> loaned =
            this->publisher.loan(this->options.sizeBytes);
        if (!loaned.has_value()) {
            HYPHA_LOG_WARN_THROTTLE(
                1.0, "large_source: loan refused with status {}", loaned.error().asInt()
            );
            ++this->refused;
            return;
        }
        const int64_t loanDone = hypha_bench::wallNowNanos();

        // Fill first, then stamp: the send time must not include the time spent
        // writing the payload, or the measurement reports memory bandwidth.
        std::memset(loaned->data(), 0xA5, loaned->size());
        const int64_t sentAt = hypha_bench::wallNowNanos();
        std::memcpy(loaned->data(), &current, sizeof(current));
        std::memcpy(loaned->data() + sizeof(current), &sentAt, sizeof(sentAt));

        const hypha::Status sendStatus = this->publisher.publishLoan(std::move(*loaned));
        const int64_t       publishDone = hypha_bench::wallNowNanos();
        const bool          sendOk = sendStatus == hypha::Status::Ok;
        if (sendOk) {
            ++this->sent;
        } else {
            ++this->failed;
            HYPHA_LOG_WARN_THROTTLE(
                1.0, "large_source: publish failed with status {}", sendStatus.asInt()
            );
        }

        const bool pastWarmup = current >= this->options.warmup;
        if (sendOk && pastWarmup) {
            this->loanNs.push_back(loanDone - loanStarted);
            this->fillNs.push_back(sentAt - loanDone);
            this->publishNs.push_back(publishDone - sentAt);
        }
    }
};

int runSource(hypha::Node& node, const Options& options)
{
    HYPHA_LOG_INFO(
        "large_source: {} MB at {} Hz", options.sizeBytes / (1024 * 1024), options.rateHz
    );

    // A raw identity needs a data type and a 32-digit lower-case hex md5.
    std::expected<hypha::RawPublisher, hypha::Status> created = hypha::rawPublisher(
        "/large_frames",
        {.dataType = "hypha/LargeFrame",
         .md5 = "0123456789abcdef0123456789abcdef",
         .definition = ""},
        hypha_bench::LARGE_FRAME_QOS,
        // Bound the payload to the frame size: the pool is then one fixed segment.
        hypha::maxPayloadDynamic(options.sizeBytes)
    );
    if (!created) {
        HYPHA_LOG_ERROR(
            "large_source: publisher creation failed with status {}", created.error().asInt()
        );
        return 1;
    }
    const auto reserved = [&options] {
        std::vector<int64_t> samples;
        samples.reserve(static_cast<size_t>(options.count));
        return samples;
    };
    Source source{node, options, std::move(*created), reserved(), reserved(), reserved()};

    // A rate that is not positive gives a period the timer refuses.
    const std::expected<hypha::Timer, hypha::Status> timer =
        node.timer(hypha::Duration::fromSeconds(1.0 / options.rateHz), [&source] {
            source.step();
        });
    if (!timer) {
        HYPHA_LOG_ERROR(
            "large_source: timer creation failed with status {}", timer.error().asInt()
        );
        return 1;
    }

    const hypha::RunResult run = node.run();
    if (!run.ok()) {
        HYPHA_LOG_ERROR(
            "the executor failed with status {}", static_cast<int>(run.status().asInt())
        );
        return 1;
    }

    std::sort(source.loanNs.begin(), source.loanNs.end());
    std::sort(source.fillNs.begin(), source.fillNs.end());
    std::sort(source.publishNs.begin(), source.publishNs.end());
    HYPHA_LOG_INFO(
        "large_source: sent={} refused={} failed={} loan_p50={}us fill_p50={}us put_p50={}us "
        "put_p99={}us",
        source.sent,
        source.refused,
        source.failed,
        hypha_bench::percentileUs(source.loanNs, 0.50),
        hypha_bench::percentileUs(source.fillNs, 0.50),
        hypha_bench::percentileUs(source.publishNs, 0.50),
        hypha_bench::percentileUs(source.publishNs, 0.99)
    );

    // A frame that was refused or failed to send is a lost frame. The exit
    // status must say so.
    const bool anyLost = source.failed > 0 || source.refused > 0;
    return anyLost ? 1 : 0;
}

int main(int argc, char** argv)
{
    const Options options = parseOptions(argc, argv);
    try {
        hypha::Node node("large_source");
        return runSource(node, options);
    } catch (const std::exception& error) {
        HYPHA_LOG_ERROR("large_source: {}", error.what());
        return 1;
    }
}
