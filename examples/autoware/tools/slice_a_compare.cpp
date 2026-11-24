/// autoware_slice_a_compare: the outputs of Slice A in recordings of hypha_record.
///
///   autoware_slice_a_compare compare <a.mcap> <b.mcap>
///   autoware_slice_a_compare summary <run.mcap>
///
/// compare passes (exit 0) when both recordings hold, for each output of Slice A, the same
/// messages in the same order, byte for byte; it prints the first message that differs and
/// exits 1. summary prints each output's message count, and the points of each cloud and
/// the objects of each frame (lowest, mean, highest). A file or topic that cannot be read
/// exits 2.

#include <hypha/backend/ros1_codec.h>
#include <hypha/testing/mcap_replay.h>

#include <autoware_perception_msgs/DetectedObjects.h>
#include <sensor_msgs/PointCloud2.h>

#include <algorithm>
#include <cstdint>
#include <cstdio>
#include <exception>
#include <string>
#include <vector>

namespace {

constexpr int kExitMatch = 0;
constexpr int kExitDiffer = 1;
constexpr int kExitUsage = 2;

const std::string kGroundRemoved = "/perception/obstacle_segmentation/single_frame/pointcloud";
const std::string kDownsampled = "/perception/obstacle_segmentation/pointcloud";
const std::string kObjects = "/perception/object_recognition/detection/objects";

/// The ROS 1 bytes of every message of @p topic in @p path, in log-time order.
template <typename M>
std::vector<std::vector<uint8_t>> encoded(const std::string& path, const std::string& topic)
{
    // The rate is the replay's step rate; read() does not use it.
    const hypha::testing::McapReplay  file(path, hypha::Hz(1.0));
    std::vector<std::vector<uint8_t>> out;
    for (const auto& item : file.read<M>(topic)) {
        out.push_back(hypha::codec::encode(item.message));
    }
    return out;
}

/// The two recordings' messages of @p topic are equal; prints the first difference.
template <typename M>
bool sameMessages(const std::string& a, const std::string& b, const std::string& topic)
{
    const auto left = encoded<M>(a, topic);
    const auto right = encoded<M>(b, topic);
    if (left.size() != right.size()) {
        std::printf(
            "compare: %s: %zu messages against %zu\n", topic.c_str(), left.size(), right.size()
        );
        return false;
    }
    for (size_t index = 0; index < left.size(); ++index) {
        if (left[index] != right[index]) {
            std::printf("compare: %s: message %zu differs\n", topic.c_str(), index);
            return false;
        }
    }
    std::printf("compare: %s: %zu messages, identical\n", topic.c_str(), left.size());
    return true;
}

/// Lowest, mean and highest of @p values, as one line.
void printSpread(const char* what, const std::string& topic, const std::vector<size_t>& values)
{
    if (values.empty()) {
        std::printf("summary: %s: no messages\n", topic.c_str());
        return;
    }
    size_t sum = 0;
    for (const size_t value : values) {
        sum += value;
    }
    const auto [low, high] = std::minmax_element(values.begin(), values.end());
    std::printf(
        "summary: %s: %zu messages, %s %zu / %.1f / %zu (lowest / mean / highest)\n",
        topic.c_str(),
        values.size(),
        what,
        *low,
        static_cast<double>(sum) / static_cast<double>(values.size()),
        *high
    );
}

void summarizeCloud(const std::string& path, const std::string& topic)
{
    const hypha::testing::McapReplay file(path, hypha::Hz(1.0));
    std::vector<size_t>              points;
    for (const auto& item : file.read<sensor_msgs::PointCloud2>(topic)) {
        points.push_back(static_cast<size_t>(item.message.width) * item.message.height);
    }
    printSpread("points", topic, points);
}

void summarizeObjects(const std::string& path)
{
    const hypha::testing::McapReplay file(path, hypha::Hz(1.0));
    std::vector<size_t>              objects;
    for (const auto& item : file.read<autoware_perception_msgs::DetectedObjects>(kObjects)) {
        objects.push_back(item.message.objects.size());
    }
    printSpread("objects", kObjects, objects);
}

int usage()
{
    std::fprintf(
        stderr,
        "usage: autoware_slice_a_compare compare <a.mcap> <b.mcap>\n"
        "       autoware_slice_a_compare summary <run.mcap>\n"
    );
    return kExitUsage;
}

}  // namespace

int main(int argc, char** argv)
{
    const std::vector<std::string> args(argv + 1, argv + argc);
    try {
        if (args.size() == 3 && args[0] == "compare") {
            bool same = sameMessages<sensor_msgs::PointCloud2>(args[1], args[2], kGroundRemoved);
            same = sameMessages<sensor_msgs::PointCloud2>(args[1], args[2], kDownsampled) && same;
            same =
                sameMessages<autoware_perception_msgs::DetectedObjects>(args[1], args[2], kObjects)
                && same;
            return same ? kExitMatch : kExitDiffer;
        }
        if (args.size() == 2 && args[0] == "summary") {
            summarizeCloud(args[1], kGroundRemoved);
            summarizeCloud(args[1], kDownsampled);
            summarizeObjects(args[1]);
            return kExitMatch;
        }
    } catch (const std::exception& error) {
        std::fprintf(stderr, "autoware_slice_a_compare: %s\n", error.what());
        return kExitUsage;
    }
    return usage();
}
