/// Measures what publishing and receiving a camera image or a lidar cloud costs, by path.
///
///   hypha_loan_bench pub <image|cloud> <bytes> <hz> <seconds> <typed|raw|loan>
///   hypha_loan_bench sub <image|cloud> <bytes> <seconds> <typed|decode|view>
///   hypha_loan_bench memcpy <bytes>
///
/// pub typed: the driver copies its frame into the message, then publish(message).
/// pub raw:   the message is encoded by hand into a raw loan (measurement only).
/// pub loan:  Publisher<M>::loan(), the frame written once into the slot.
/// sub typed: an ordinary typed subscriber. decode: a raw subscriber that decodes as the typed
///            one does (make_shared + decode) and times it. view: the bytes are read in place.

#include <hypha/hypha.h>

#include "bench_util.h"

#include <sensor_msgs/Image.h>
#include <sensor_msgs/PointCloud2.h>
#include <time.h>

#include <algorithm>
#include <chrono>
#include <cinttypes>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <span>
#include <string>
#include <vector>

namespace {

using Clock = std::chrono::steady_clock;

/// CPU time of the calling thread: a preempted thread does not count the time it waited.
int64_t threadCpuNanos()
{
    timespec now{};
    clock_gettime(CLOCK_THREAD_CPUTIME_ID, &now);
    return static_cast<int64_t>(now.tv_sec) * 1'000'000'000 + now.tv_nsec;
}

int64_t nanosSince(Clock::time_point start)
{
    return std::chrono::duration_cast<std::chrono::nanoseconds>(Clock::now() - start).count();
}

inline constexpr hypha::Qos BENCH_QOS{
    .name = "loan_bench", .publishers = 1, .subscribers = 2, .maxDepth = 3
};

// The vendor's point, as the Hesai SDK hands it over.
struct VendorPoint
{
    float    x, y, z, intensity;
    uint16_t ring;
    double   timestamp;
};

#pragma pack(push, 1)

struct CloudPoint
{
    float    x, y, z, intensity;
    uint16_t ring;
    double   timestamp;
};

#pragma pack(pop)
static_assert(sizeof(CloudPoint) == 26);

void fillFields(sensor_msgs::PointCloud2& cloud)
{
    using F = sensor_msgs::PointField;
    cloud.fields = {
        {.name = "x", .offset = offsetof(CloudPoint, x), .datatype = F::FLOAT32, .count = 1},
        {.name = "y", .offset = offsetof(CloudPoint, y), .datatype = F::FLOAT32, .count = 1},
        {.name = "z", .offset = offsetof(CloudPoint, z), .datatype = F::FLOAT32, .count = 1},
        {.name = "intensity",
         .offset = offsetof(CloudPoint, intensity),
         .datatype = F::FLOAT32,
         .count = 1},
        {.name = "ring", .offset = offsetof(CloudPoint, ring), .datatype = F::UINT16, .count = 1},
        {.name = "timestamp",
         .offset = offsetof(CloudPoint, timestamp),
         .datatype = F::FLOAT64,
         .count = 1},
    };
}

void convertPoints(const VendorPoint* in, size_t count, std::byte* out)
{
    for (size_t i = 0; i < count; ++i) {
        const CloudPoint p{in[i].x, in[i].y, in[i].z, in[i].intensity, in[i].ring, in[i].timestamp};
        std::memcpy(out + i * sizeof(CloudPoint), &p, sizeof(CloudPoint));
    }
}

// Four vendor buffers in turn, so a frame is not in the cache when it is copied (a camera's
// DMA or a lidar's packet decoder wrote it).
constexpr size_t RING = 4;

struct Stats
{
    std::vector<int64_t> totalNs;  // build + publish
    std::vector<int64_t> publishNs;
    std::vector<int64_t> cpuFrameNs;  // thread CPU of build + publish
    std::vector<int64_t> cpuLoanNs;  // thread CPU of the loan call
    std::vector<int64_t> cpuPublishNs;
    uint64_t             sent{0};
    uint64_t             failed{0};
    hypha::Status        lastFailure{hypha::Status::Ok};
};

void report(
    const char*                      label,
    Stats&                           stats,
    double                           seconds,
    const hypha_bench::RusageSample& a,
    const hypha_bench::RusageSample& b
)
{
    for (std::vector<int64_t>* v :
         {&stats.totalNs,
          &stats.publishNs,
          &stats.cpuFrameNs,
          &stats.cpuLoanNs,
          &stats.cpuPublishNs}) {
        std::sort(v->begin(), v->end());
    }
    const double cpu = (b.userSeconds - a.userSeconds) + (b.systemSeconds - a.systemSeconds);
    std::printf(
        "RESULT %s sent=%" PRIu64 " failed=%" PRIu64
        " last_failure=%s frame_p50_us=%.0f frame_p99_us=%.0f "
        "publish_p50_us=%.0f publish_p99_us=%.0f cpu_pct=%.1f cpu_ms_per_msg=%.2f "
        "thread_cpu_us: frame_p50=%.0f loan_p50=%.0f publish_p50=%.0f\n",
        label,
        stats.sent,
        stats.failed,
        stats.lastFailure.name().data(),
        hypha_bench::percentileNs(stats.totalNs, 0.5) / 1e3,
        hypha_bench::percentileNs(stats.totalNs, 0.99) / 1e3,
        hypha_bench::percentileNs(stats.publishNs, 0.5) / 1e3,
        hypha_bench::percentileNs(stats.publishNs, 0.99) / 1e3,
        100.0 * cpu / seconds,
        stats.sent ? 1e3 * cpu / static_cast<double>(stats.sent) : 0.0,
        hypha_bench::percentileNs(stats.cpuFrameNs, 0.5) / 1e3,
        hypha_bench::percentileNs(stats.cpuLoanNs, 0.5) / 1e3,
        hypha_bench::percentileNs(stats.cpuPublishNs, 0.5) / 1e3
    );
}

struct PubConfig
{
    bool        image{true};
    size_t      bytes{0};
    double      hz{30};
    double      seconds{10};
    std::string mode;
};

/// Image: 2448 wide, mono8 or rgb8 by size. Cloud: 26-byte points.
sensor_msgs::Image imageHead(size_t bytes)
{
    sensor_msgs::Image head;
    head.header.frame_id = "camera_optical";
    head.width = 2448;
    const bool rgb = bytes > 10'000'000;
    head.encoding = rgb ? "rgb8" : "mono8";
    head.step = head.width * (rgb ? 3U : 1U);
    head.height = static_cast<uint32_t>(bytes / head.step);
    return head;
}

size_t imageBytes(size_t bytes)
{
    const sensor_msgs::Image head = imageHead(bytes);
    return static_cast<size_t>(head.step) * head.height;
}

template <typename M>
hypha::RawType rawTypeOf()
{
    return {hypha::codec::dataType<M>(), hypha::codec::md5Sum<M>(), hypha::codec::definition<M>()};
}

/// The message with an empty payload field, encoded: the prefix, a zero length, the tail.
template <typename M>
std::vector<uint8_t> encodedHead(const M& head)
{
    return hypha::codec::encode(head);
}

int runPub(const PubConfig& config)
{
    hypha::Node  node("loan_bench_pub");
    const size_t payload = config.image ? imageBytes(config.bytes) : config.bytes / 26 * 26;
    const size_t points = payload / 26;
    const size_t bound = payload + 4096;

    std::vector<std::vector<uint8_t>>     frames(RING);
    std::vector<std::vector<VendorPoint>> clouds(RING);
    for (size_t r = 0; r < RING; ++r) {
        if (config.image) {
            frames[r].resize(payload);
            for (size_t i = 0; i < payload; ++i) {
                frames[r][i] = static_cast<uint8_t>(i * 7 + r);
            }
        } else {
            clouds[r].resize(points);
            for (size_t i = 0; i < points; ++i) {
                const float f = static_cast<float>(i + r);
                clouds[r][i] = {
                    f,
                    f + 1,
                    f + 2,
                    f + 3,
                    static_cast<uint16_t>(i % 64),
                    static_cast<double>(f) * 1e-6
                };
            }
        }
    }

    Stats stats;
    stats.totalNs.reserve(4096);
    stats.publishNs.reserve(4096);
    const int count = static_cast<int>(config.seconds * config.hz);
    int       frame = 0;
    const int warmup = 5;

    hypha::Publisher<sensor_msgs::Image>              imagePub;
    hypha::Publisher<sensor_msgs::PointCloud2>        cloudPub;
    std::expected<hypha::RawPublisher, hypha::Status> raw =
        std::unexpected(hypha::Status::InvalidState);
    if (config.mode == "raw") {
        raw = config.image ? hypha::rawPublisher(
                                 "/bench",
                                 rawTypeOf<sensor_msgs::Image>(),
                                 BENCH_QOS,
                                 hypha::maxPayloadDynamic(bound)
                             )
                           : hypha::rawPublisher(
                                 "/bench",
                                 rawTypeOf<sensor_msgs::PointCloud2>(),
                                 BENCH_QOS,
                                 hypha::maxPayloadDynamic(bound)
                             );
        if (!raw) {
            std::printf("raw publisher refused: %s\n", raw.error().name().data());
            return 1;
        }
    } else if (config.image) {
        imagePub = node.advertise<sensor_msgs::Image>(
            hypha::Topic{"/bench"}, BENCH_QOS, hypha::maxPayloadDynamic(bound)
        );
    } else {
        cloudPub = node.advertise<sensor_msgs::PointCloud2>(
            hypha::Topic{"/bench"}, BENCH_QOS, hypha::maxPayloadDynamic(bound)
        );
    }

    // The message a driver keeps across frames (the Arena port reuses its Image).
    sensor_msgs::Image       imageMsg = imageHead(config.bytes);
    sensor_msgs::PointCloud2 cloudHead;
    cloudHead.header.frame_id = "lidar";
    cloudHead.height = 1;
    fillFields(cloudHead);
    cloudHead.point_step = 26;
    cloudHead.is_dense = false;

    const auto step = [&] {
        if (frame >= count + warmup) {
            hypha::requestStop();
            return;
        }
        const size_t            r = static_cast<size_t>(frame) % RING;
        const Clock::time_point start = Clock::now();
        const int64_t           cpuStart = threadCpuNanos();
        int64_t                 publishNs = 0;
        int64_t                 cpuLoan = 0;
        int64_t                 cpuPublish = 0;
        hypha::Status           status = hypha::Status::Ok;
        if (config.mode == "typed" && config.image) {
            imageMsg.data.assign(frames[r].begin(), frames[r].end());
            const Clock::time_point p = Clock::now();
            const int64_t           cpuP = threadCpuNanos();
            status = imagePub.publish(imageMsg) ? hypha::Status(hypha::Status::Ok)
                                                : imagePub.lastPublishStatus();
            publishNs = nanosSince(p);
            cpuPublish = threadCpuNanos() - cpuP;
        } else if (config.mode == "typed") {
            // As the Hesai port does: a new message per revolution, resized, then filled.
            sensor_msgs::PointCloud2 cloud = cloudHead;
            cloud.width = static_cast<uint32_t>(points);
            cloud.row_step = cloud.width * cloud.point_step;
            cloud.data.resize(points * 26);
            convertPoints(
                clouds[r].data(), points, reinterpret_cast<std::byte*>(cloud.data.data())
            );
            const Clock::time_point p = Clock::now();
            const int64_t           cpuP = threadCpuNanos();
            status = cloudPub.publish(cloud) ? hypha::Status(hypha::Status::Ok)
                                             : cloudPub.lastPublishStatus();
            publishNs = nanosSince(p);
            cpuPublish = threadCpuNanos() - cpuP;
        } else if (config.mode == "raw") {
            // Encoded by hand for the measurement: prefix, length, payload, tail.
            std::vector<uint8_t> head;
            if (config.image) {
                head = encodedHead(imageHead(config.bytes));
            } else {
                sensor_msgs::PointCloud2 cloud = cloudHead;
                cloud.width = static_cast<uint32_t>(points);
                cloud.row_step = cloud.width * cloud.point_step;
                head = encodedHead(cloud);
            }
            const size_t                                      tail = config.image ? 0 : 1;
            const size_t                                      prefix = head.size() - 4 - tail;
            const int64_t                                     cpuL = threadCpuNanos();
            std::expected<hypha::LoanedBuffer, hypha::Status> loaned =
                raw->loan(prefix + 4 + payload + tail);
            cpuLoan = threadCpuNanos() - cpuL;
            if (!loaned) {
                status = loaned.error();
            } else {
                uint8_t* out = loaned->data();
                std::memcpy(out, head.data(), prefix);
                const uint32_t length = static_cast<uint32_t>(payload);
                std::memcpy(out + prefix, &length, 4);
                if (config.image) {
                    std::memcpy(out + prefix + 4, frames[r].data(), payload);
                } else {
                    convertPoints(
                        clouds[r].data(), points, reinterpret_cast<std::byte*>(out + prefix + 4)
                    );
                    out[prefix + 4 + payload] = head.back();
                }
                const Clock::time_point p = Clock::now();
                const int64_t           cpuP = threadCpuNanos();
                status = raw->publishLoan(std::move(*loaned));
                publishNs = nanosSince(p);
                cpuPublish = threadCpuNanos() - cpuP;
            }
        } else if (config.mode == "loan" && config.image) {
            const sensor_msgs::Image head = imageHead(config.bytes);
            const int64_t            cpuL = threadCpuNanos();
            auto                     loaned = imagePub.loan(head, payload);
            cpuLoan = threadCpuNanos() - cpuL;
            if (!loaned) {
                status = loaned.error();
            } else {
                std::memcpy(loaned->data().data(), frames[r].data(), payload);
                const Clock::time_point p = Clock::now();
                const int64_t           cpuP = threadCpuNanos();
                status = imagePub.publish(std::move(*loaned)) ? hypha::Status(hypha::Status::Ok)
                                                              : imagePub.lastPublishStatus();
                publishNs = nanosSince(p);
                cpuPublish = threadCpuNanos() - cpuP;
            }
        } else if (config.mode == "loan") {
            const int64_t cpuL = threadCpuNanos();
            auto          loaned = cloudPub.loan(cloudHead, payload);
            cpuLoan = threadCpuNanos() - cpuL;
            if (!loaned) {
                status = loaned.error();
            } else {
                // put() copies each point with memcpy: data() has no alignment.
                const VendorPoint* in = clouds[r].data();
                for (size_t i = 0; i < points; ++i) {
                    loaned->put(
                        i,
                        CloudPoint{
                            in[i].x, in[i].y, in[i].z, in[i].intensity, in[i].ring, in[i].timestamp
                        }
                    );
                }
                loaned->message().width = static_cast<uint32_t>(points);
                loaned->message().row_step = static_cast<uint32_t>(points * 26);
                const Clock::time_point p = Clock::now();
                const int64_t           cpuP = threadCpuNanos();
                status = cloudPub.publish(std::move(*loaned)) ? hypha::Status(hypha::Status::Ok)
                                                              : cloudPub.lastPublishStatus();
                publishNs = nanosSince(p);
                cpuPublish = threadCpuNanos() - cpuP;
            }
        }
        const int64_t total = nanosSince(start);
        if (status == hypha::Status::Ok) {
            ++stats.sent;
        } else {
            ++stats.failed;
            stats.lastFailure = status;
        }
        if (frame >= warmup && status == hypha::Status::Ok) {
            stats.totalNs.push_back(total);
            stats.publishNs.push_back(publishNs);
            stats.cpuFrameNs.push_back(threadCpuNanos() - cpuStart);
            stats.cpuLoanNs.push_back(cpuLoan);
            stats.cpuPublishNs.push_back(cpuPublish);
        }
        ++frame;
    };

    const hypha_bench::RusageSample before = hypha_bench::sampleRusage();
    const Clock::time_point         began = Clock::now();
    const auto             timer = node.timer(hypha::Duration::fromSeconds(1.0 / config.hz), step);
    const hypha::RunResult run = node.run();
    const double           elapsed = static_cast<double>(nanosSince(began)) / 1e9;
    const hypha_bench::RusageSample after = hypha_bench::sampleRusage();
    char                            label[128];
    std::snprintf(
        label,
        sizeof label,
        "pub %s %zu B %.0f Hz %s",
        config.image ? "image" : "cloud",
        payload,
        config.hz,
        config.mode.c_str()
    );
    report(label, stats, elapsed, before, after);
    return run.ok() ? 0 : 1;
}

int runSub(bool image, double seconds, const std::string& mode)
{
    hypha::Node          node("loan_bench_sub");
    std::vector<int64_t> decodeNs;
    decodeNs.reserve(4096);
    uint64_t received = 0;
    uint64_t checksum = 0;

    hypha::Subscriber<sensor_msgs::Image>              imageSub;
    hypha::Subscriber<sensor_msgs::PointCloud2>        cloudSub;
    std::expected<hypha::RawSubscriber, hypha::Status> raw =
        std::unexpected(hypha::Status::InvalidState);
    if (mode == "typed" && image) {
        imageSub = node.subscribe<sensor_msgs::Image>(
            hypha::Topic{"/bench"},
            [&](const sensor_msgs::Image::ConstPtr& m) {
                ++received;
                checksum += m->data.back();
            },
            BENCH_QOS
        );
    } else if (mode == "typed") {
        cloudSub = node.subscribe<sensor_msgs::PointCloud2>(
            hypha::Topic{"/bench"},
            [&](const sensor_msgs::PointCloud2::ConstPtr& m) {
                ++received;
                checksum += m->data.back();
            },
            BENCH_QOS
        );
    } else {
        const auto callback = [&](const hypha::RawSample& sample) {
            ++received;
            if (mode == "decode") {
                const int64_t start = threadCpuNanos();
                if (image) {
                    std::shared_ptr<sensor_msgs::Image> m = std::make_shared<sensor_msgs::Image>();
                    checksum += hypha::codec::decode(sample.bytes.data(), sample.bytes.size(), *m)
                                  ? m->data.back()
                                  : 0;
                } else {
                    std::shared_ptr<sensor_msgs::PointCloud2> m =
                        std::make_shared<sensor_msgs::PointCloud2>();
                    checksum += hypha::codec::decode(sample.bytes.data(), sample.bytes.size(), *m)
                                  ? m->data.back()
                                  : 0;
                }
                decodeNs.push_back(threadCpuNanos() - start);
            } else {
                checksum += sample.bytes.back();
            }
        };
        raw = hypha::subscribeRawOfType(
            "/bench",
            image
                ? hypha::
                      RawType{.dataType = hypha::codec::dataType<sensor_msgs::Image>(), .md5 = hypha::codec::md5Sum<sensor_msgs::Image>()}
                : hypha::
                      RawType{.dataType = hypha::codec::dataType<sensor_msgs::PointCloud2>(), .md5 = hypha::codec::md5Sum<sensor_msgs::PointCloud2>()},
            callback,
            0,
            BENCH_QOS
        );
        if (!raw) {
            std::printf("raw subscriber refused: %s\n", raw.error().name().data());
            return 1;
        }
    }
    const hypha_bench::RusageSample before = hypha_bench::sampleRusage();
    const Clock::time_point         began = Clock::now();
    const auto                      timer = node.timer(hypha::Duration::fromSeconds(0.1), [&] {
        if (nanosSince(began) > static_cast<int64_t>(seconds * 1e9)) {
            hypha::requestStop();
        }
    });
    const hypha::RunResult          run = node.run();
    const double                    elapsed = static_cast<double>(nanosSince(began)) / 1e9;
    const hypha_bench::RusageSample after = hypha_bench::sampleRusage();
    const double                    cpu =
        (after.userSeconds - before.userSeconds) + (after.systemSeconds - before.systemSeconds);
    std::sort(decodeNs.begin(), decodeNs.end());
    std::printf(
        "RESULT sub %s %s received=%" PRIu64 " cpu_pct=%.1f cpu_ms_per_msg=%.2f decode_p50_us=%.0f "
        "decode_p99_us=%.0f (checksum %" PRIu64 ")\n",
        image ? "image" : "cloud",
        mode.c_str(),
        received,
        100.0 * cpu / elapsed,
        received ? 1e3 * cpu / static_cast<double>(received) : 0.0,
        hypha_bench::percentileNs(decodeNs, 0.5) / 1e3,
        hypha_bench::percentileNs(decodeNs, 0.99) / 1e3,
        checksum
    );
    return run.ok() ? 0 : 1;
}

int runMemcpy(size_t bytes)
{
    std::vector<std::vector<uint8_t>> src(RING, std::vector<uint8_t>(bytes, 1));
    std::vector<uint8_t>              dst(bytes, 0);
    std::vector<int64_t>              copyNs;
    std::vector<int64_t>              zeroNs;
    std::vector<int64_t>              freshNs;
    for (int i = 0; i < 64; ++i) {
        const Clock::time_point a = Clock::now();
        std::memcpy(dst.data(), src[static_cast<size_t>(i) % RING].data(), bytes);
        copyNs.push_back(nanosSince(a));
        const Clock::time_point b = Clock::now();
        std::memset(dst.data(), 0, bytes);
        zeroNs.push_back(nanosSince(b));
        // A new vector of the frame's size, filled: what make_shared<M> + decode does.
        const Clock::time_point c = Clock::now();
        std::vector<uint8_t>    fresh(bytes);
        std::memcpy(fresh.data(), src[static_cast<size_t>(i) % RING].data(), bytes);
        freshNs.push_back(nanosSince(c));
        if (fresh[bytes - 1] != 1) {
            return 1;
        }
    }
    std::sort(copyNs.begin(), copyNs.end());
    std::sort(zeroNs.begin(), zeroNs.end());
    std::sort(freshNs.begin(), freshNs.end());
    std::printf(
        "RESULT memcpy %zu B: memcpy_p50_us=%.0f (%.1f GB/s) memset_p50_us=%.0f "
        "new_vector_and_copy_p50_us=%.0f\n",
        bytes,
        hypha_bench::percentileNs(copyNs, 0.5) / 1e3,
        static_cast<double>(bytes) / static_cast<double>(hypha_bench::percentileNs(copyNs, 0.5)),
        hypha_bench::percentileNs(zeroNs, 0.5) / 1e3,
        hypha_bench::percentileNs(freshNs, 0.5) / 1e3
    );
    return 0;
}

}  // namespace

int main(int argc, char** argv)
{
    if (argc < 3) {
        std::fprintf(stderr, "usage: see the file comment\n");
        return 2;
    }
    const std::string verb = argv[1];
    try {
        if (verb == "memcpy") {
            return runMemcpy(std::strtoull(argv[2], nullptr, 10));
        }
        const bool image = std::string(argv[2]) == "image";
        if (verb == "pub" && argc == 7) {
            return runPub(
                {image,
                 std::strtoull(argv[3], nullptr, 10),
                 std::atof(argv[4]),
                 std::atof(argv[5]),
                 argv[6]}
            );
        }
        if (verb == "sub" && argc == 6) {
            return runSub(image, std::atof(argv[4]), argv[5]);
        }
    } catch (const std::exception& error) {
        std::fprintf(stderr, "loan_bench: %s\n", error.what());
        return 1;
    }
    std::fprintf(stderr, "usage: see the file comment\n");
    return 2;
}
