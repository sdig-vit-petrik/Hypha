/// The out-of-line part of the ros/ros.h shim.
#include <hypha/node.h>

#include <ros/package.h>
#include <ros/ros.h>

#include <atomic>
#include <cstdarg>
#include <cstdio>
#include <filesystem>
#include <vector>

HYPHA_LOG_MODULE_INIT("mrs_shim")

namespace mrs {

namespace {
thread_local std::optional<hypha::Time> stepTime;
std::string                             currentNodeName;
std::atomic<uint64_t>                   throttled{0};
}  // namespace

std::optional<hypha::Time> currentStepTime()
{
    return stepTime;
}

void setCurrentStepTime(std::optional<hypha::Time> time)
{
    stepTime = time;
}

const std::string& nodeName()
{
    return currentNodeName;
}

void setNodeName(std::string name)
{
    currentNodeName = std::move(name);
}

std::string formatPrintf(const char* format, ...)
{
    va_list args;
    va_start(args, format);
    va_list copy;
    va_copy(copy, args);
    const int length = std::vsnprintf(nullptr, 0, format, copy);
    va_end(copy);
    std::string text;
    if (length > 0) {
        std::vector<char> buffer(static_cast<size_t>(length) + 1);
        std::vsnprintf(buffer.data(), buffer.size(), format, args);
        text.assign(buffer.data(), static_cast<size_t>(length));
    }
    va_end(args);
    return text;
}

uint64_t throttledLogCount()
{
    return throttled.load();
}

namespace detail {
bool throttlePasses(std::atomic<int64_t>& last, double periodSeconds)
{
    const int64_t now = std::chrono::duration_cast<std::chrono::nanoseconds>(
                            std::chrono::steady_clock::now().time_since_epoch()
    )
                            .count();
    const auto    period = static_cast<int64_t>(periodSeconds * 1e9);
    int64_t       previous = last.load();
    const bool    due = previous == INT64_MIN || now - previous >= period;
    if (!due || !last.compare_exchange_strong(previous, now)) {
        return false;
    }
    throttled.fetch_add(1);
    return true;
}
}  // namespace detail

}  // namespace mrs

namespace ros {

std::string this_node::getName()
{
    return "/" + mrs::nodeName();
}

void shutdown()
{
    hypha::requestStop();
}

bool ok()
{
    return hypha::ok();
}

std::string package::getPath(const std::string& package)
{
    const std::filesystem::path path = std::filesystem::path(MRS_UPSTREAM_DIR) / package;
    if (package.empty() || !std::filesystem::is_directory(path)) {
        HYPHA_LOG_ERROR("ros::package::getPath: package '{}' is not vendored", package);
        return "";
    }
    return path.string();
}

}  // namespace ros
