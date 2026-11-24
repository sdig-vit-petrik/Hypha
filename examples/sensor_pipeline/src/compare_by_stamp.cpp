/// sensor_pipeline_compare: compares the centroid outputs of two recordings by header
/// stamp, and counts the clouds of a recording. No bus, no node: it reads MCAP files with
/// the test-only reader of hypha::testing_mcap (hypha/testing/mcap_replay.h).
///
///   sensor_pipeline_compare compare <expected.mcap> <actual.mcap> <topic> <tolerance_m>
///                                   [<expected topic>]
///   sensor_pipeline_compare common <a.mcap> <b.mcap> <topic> <tolerance_m>
///   sensor_pipeline_compare clouds <recording.mcap> <topic>
///
/// compare passes (exit 0) when both lists have the same length, each pair has the same
/// stamp and frame, and each coordinate differs by at most <tolerance_m>; otherwise it
/// prints the first divergent stamp and exits 1. common pairs the messages of the two
/// files by stamp instead: it passes when at least one stamp is in both and every such
/// pair matches, and it counts the stamps that only one file holds. A file that does not
/// open or a topic it does not hold exits 2.

#include <hypha/hypha.h>
#include <hypha/testing/mcap_replay.h>

#include <geometry_msgs/PointStamped.h>
#include <sensor_msgs/PointCloud2.h>

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <exception>
#include <map>
#include <optional>
#include <string>
#include <vector>

namespace {

using RecordedPoint = hypha::testing::Recorded<geometry_msgs::PointStamped>;
using RecordedCloud = hypha::testing::Recorded<sensor_msgs::PointCloud2>;

constexpr int kExitMatch = 0;
constexpr int kExitDiffer = 1;
constexpr int kExitUsage = 2;

/// One output, keyed by its header stamp in nanoseconds.
struct StampedPoint
{
    int64_t     stampNanos;
    std::string frame;
    double      x;
    double      y;
    double      z;
};

bool earlierStamp(const StampedPoint& left, const StampedPoint& right)
{
    return left.stampNanos < right.stampNanos;
}

/// The outputs of @p topic in @p path, ordered by stamp.
/// @throws std::runtime_error when the file or the topic cannot be read, or a stamp is
///         not a valid time.
std::vector<StampedPoint> readPoints(const std::string& path, const std::string& topic)
{
    // The rate is the replay's step rate; read() does not use it.
    const hypha::testing::McapReplay file(path, hypha::Hz(1.0));
    const std::vector<RecordedPoint> recorded = file.read<geometry_msgs::PointStamped>(topic);
    std::vector<StampedPoint>        points;
    points.reserve(recorded.size());
    for (const RecordedPoint& item : recorded) {
        const std::optional<hypha::Time> stamp = hypha::fromRosTime(item.message.header.stamp);
        if (!stamp.has_value()) {
            throw std::runtime_error(path + ": a message of " + topic + " has an invalid stamp");
        }
        points.push_back(
            StampedPoint{
                stamp->nanos(),
                item.message.header.frame_id,
                item.message.point.x,
                item.message.point.y,
                item.message.point.z,
            }
        );
    }
    std::stable_sort(points.begin(), points.end(), earlierStamp);
    return points;
}

double largestDifference(const StampedPoint& expected, const StampedPoint& actual)
{
    const double dx = std::fabs(expected.x - actual.x);
    const double dy = std::fabs(expected.y - actual.y);
    const double dz = std::fabs(expected.z - actual.z);
    return std::max({dx, dy, dz});
}

void printStamp(const char* what, int64_t nanos)
{
    std::printf(
        "%s stamp %lld.%09lld\n",
        what,
        static_cast<long long>(nanos / 1'000'000'000),
        static_cast<long long>(nanos % 1'000'000'000)
    );
}

int compare(
    const std::string& expectedPath,
    const std::string& actualPath,
    const std::string& topic,
    double             tolerance,
    const std::string& expectedTopic
)
{
    const std::vector<StampedPoint> expected = readPoints(expectedPath, expectedTopic);
    const std::vector<StampedPoint> actual = readPoints(actualPath, topic);
    const size_t                    paired = std::min(expected.size(), actual.size());
    double                          largest = 0.0;
    for (size_t index = 0; index < paired; ++index) {
        const StampedPoint& want = expected[index];
        const StampedPoint& got = actual[index];
        if (want.stampNanos != got.stampNanos) {
            std::printf("compare: message %zu: the stamps differ\n", index);
            printStamp("compare: expected", want.stampNanos);
            printStamp("compare: actual  ", got.stampNanos);
            return kExitDiffer;
        }
        if (want.frame != got.frame) {
            std::printf(
                "compare: message %zu: frame '%s' expected, '%s' found\n",
                index,
                want.frame.c_str(),
                got.frame.c_str()
            );
            printStamp("compare: first divergent", want.stampNanos);
            return kExitDiffer;
        }
        const double difference = largestDifference(want, got);
        largest = std::max(largest, difference);
        const bool outside = !(difference <= tolerance);
        if (outside) {
            std::printf(
                "compare: message %zu: (%.9f, %.9f, %.9f) expected, (%.9f, %.9f, %.9f) found, "
                "difference %.3g m > %.3g m\n",
                index,
                want.x,
                want.y,
                want.z,
                got.x,
                got.y,
                got.z,
                difference,
                tolerance
            );
            printStamp("compare: first divergent", want.stampNanos);
            return kExitDiffer;
        }
    }
    if (expected.size() != actual.size()) {
        std::printf("compare: %zu messages expected, %zu found\n", expected.size(), actual.size());
        const bool    expectedLonger = expected.size() > actual.size();
        const int64_t firstUnpaired =
            expectedLonger ? expected[paired].stampNanos : actual[paired].stampNanos;
        printStamp("compare: first unpaired", firstUnpaired);
        return kExitDiffer;
    }
    std::printf(
        "compare: %zu messages of %s match by stamp; largest difference %.3g m, tolerance %.3g m\n",
        expected.size(),
        topic.c_str(),
        largest,
        tolerance
    );
    return kExitMatch;
}

int compareCommon(
    const std::string& firstPath,
    const std::string& secondPath,
    const std::string& topic,
    double             tolerance
)
{
    const std::vector<StampedPoint> first = readPoints(firstPath, topic);
    std::map<int64_t, StampedPoint> second;
    for (const StampedPoint& point : readPoints(secondPath, topic)) {
        second.emplace(point.stampNanos, point);
    }
    size_t paired = 0;
    size_t onlyFirst = 0;
    double largest = 0.0;
    for (const StampedPoint& want : first) {
        const auto found = second.find(want.stampNanos);
        if (found == second.end()) {
            ++onlyFirst;
            continue;
        }
        const StampedPoint& got = found->second;
        const double        difference = largestDifference(want, got);
        const bool          outside = !(difference <= tolerance);
        if (want.frame != got.frame || outside) {
            std::printf(
                "common: the messages differ (frame '%s' and '%s', difference %.3g m)\n",
                want.frame.c_str(),
                got.frame.c_str(),
                difference
            );
            printStamp("common: stamp", want.stampNanos);
            return kExitDiffer;
        }
        largest = std::max(largest, difference);
        second.erase(found);
        ++paired;
    }
    std::printf(
        "common: %zu messages of %s match by stamp, %zu only in the first file, %zu only in "
        "the second; largest difference %.3g m\n",
        paired,
        topic.c_str(),
        onlyFirst,
        second.size(),
        largest
    );
    return paired > 0 ? kExitMatch : kExitDiffer;
}

int countClouds(const std::string& path, const std::string& topic)
{
    const hypha::testing::McapReplay file(path, hypha::Hz(1.0));
    const std::vector<RecordedCloud> clouds = file.read<sensor_msgs::PointCloud2>(topic);
    std::printf("%zu\n", clouds.size());
    return kExitMatch;
}

int usage()
{
    std::fprintf(
        stderr,
        "usage: sensor_pipeline_compare compare <expected.mcap> <actual.mcap> <topic> "
        "<tolerance_m>\n"
        "                                       [<expected topic>]\n"
        "       sensor_pipeline_compare common <a.mcap> <b.mcap> <topic> <tolerance_m>\n"
        "       sensor_pipeline_compare clouds <recording.mcap> <topic>\n"
    );
    return kExitUsage;
}

}  // namespace

int main(int argc, char** argv)
{
    if (argc < 2) {
        return usage();
    }
    const std::string command = argv[1];
    try {
        const bool compareArguments = argc == 6 || argc == 7;
        const bool compareCommand = command == "compare" && compareArguments;
        const bool commonCommand = command == "common" && argc == 6;
        if (compareCommand || commonCommand) {
            char*        end = nullptr;
            const double tolerance = std::strtod(argv[5], &end);
            const bool   parsed = end != argv[5] && *end == '\0';
            const bool   validTolerance = parsed && std::isfinite(tolerance) && tolerance >= 0.0;
            if (!validTolerance) {
                std::fprintf(stderr, "sensor_pipeline_compare: tolerance must be a number >= 0\n");
                return kExitUsage;
            }
            const std::string topic = argv[4];
            if (commonCommand) {
                return compareCommon(argv[2], argv[3], topic, tolerance);
            }
            const std::string expectedTopic = argc == 7 ? std::string(argv[6]) : topic;
            return compare(argv[2], argv[3], topic, tolerance, expectedTopic);
        }
        const bool cloudsCommand = command == "clouds" && argc == 4;
        if (cloudsCommand) {
            return countClouds(argv[2], argv[3]);
        }
        return usage();
    } catch (const std::exception& error) {
        std::fprintf(stderr, "sensor_pipeline_compare: %s\n", error.what());
        return kExitUsage;
    }
}
