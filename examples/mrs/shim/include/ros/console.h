#pragma once

/// @file
/// ROS_* log macros: printf-format into a string, log it through hypha. The
/// _THROTTLE variants throttle per call site in host time.

#include <hypha_log/hypha_log.h>

#include <atomic>
#include <chrono>
#include <cstdint>
#include <sstream>
#include <string>

namespace mrs {

/// printf-style formatting into a string.
std::string formatPrintf(const char* format, ...) __attribute__((format(printf, 1, 2)));

inline std::string formatPrintf(const std::string& text)
{
    return text;
}

/// How many throttled logs passed their throttle, for tests.
uint64_t throttledLogCount();

namespace detail {
/// True when the call site of @p last may log now; records the time.
bool throttlePasses(std::atomic<int64_t>& last, double periodSeconds);
}  // namespace detail

}  // namespace mrs

#define MRS_LOG_AT(LEVEL, text)                                                                    \
    do {                                                                                           \
        const std::string mrsLogText = (text);                                                     \
        HYPHA_LOG_##LEVEL("{}", mrsLogText);                                                       \
    } while (false)

#define MRS_LOG_THROTTLED(LEVEL, period, text)                                                     \
    do {                                                                                           \
        static std::atomic<int64_t> mrsLogLast{INT64_MIN};                                         \
        if (::mrs::detail::throttlePasses(mrsLogLast, static_cast<double>(period))) {              \
            MRS_LOG_AT(LEVEL, text);                                                               \
        }                                                                                          \
    } while (false)

#define MRS_LOG_ONCE(LEVEL, text)                                                                  \
    do {                                                                                           \
        static std::atomic<bool> mrsLogDone{false};                                                \
        if (!mrsLogDone.exchange(true)) {                                                          \
            MRS_LOG_AT(LEVEL, text);                                                               \
        }                                                                                          \
    } while (false)

#define MRS_STREAM_TEXT(expr)                                                                      \
    ([&] {                                                                                         \
        std::ostringstream mrsLogStream;                                                           \
        mrsLogStream << expr;                                                                      \
        return mrsLogStream.str();                                                                 \
    }())

#define ROS_DEBUG(...) MRS_LOG_AT(DEBUG, ::mrs::formatPrintf(__VA_ARGS__))
#define ROS_INFO(...) MRS_LOG_AT(INFO, ::mrs::formatPrintf(__VA_ARGS__))
#define ROS_WARN(...) MRS_LOG_AT(WARN, ::mrs::formatPrintf(__VA_ARGS__))
#define ROS_ERROR(...) MRS_LOG_AT(ERROR, ::mrs::formatPrintf(__VA_ARGS__))
#define ROS_FATAL(...) MRS_LOG_AT(FATAL, ::mrs::formatPrintf(__VA_ARGS__))

#define ROS_DEBUG_THROTTLE(p, ...) MRS_LOG_THROTTLED(DEBUG, p, ::mrs::formatPrintf(__VA_ARGS__))
#define ROS_INFO_THROTTLE(p, ...) MRS_LOG_THROTTLED(INFO, p, ::mrs::formatPrintf(__VA_ARGS__))
#define ROS_WARN_THROTTLE(p, ...) MRS_LOG_THROTTLED(WARN, p, ::mrs::formatPrintf(__VA_ARGS__))
#define ROS_ERROR_THROTTLE(p, ...) MRS_LOG_THROTTLED(ERROR, p, ::mrs::formatPrintf(__VA_ARGS__))
#define ROS_FATAL_THROTTLE(p, ...) MRS_LOG_THROTTLED(FATAL, p, ::mrs::formatPrintf(__VA_ARGS__))

#define ROS_DEBUG_ONCE(...) MRS_LOG_ONCE(DEBUG, ::mrs::formatPrintf(__VA_ARGS__))
#define ROS_INFO_ONCE(...) MRS_LOG_ONCE(INFO, ::mrs::formatPrintf(__VA_ARGS__))
#define ROS_WARN_ONCE(...) MRS_LOG_ONCE(WARN, ::mrs::formatPrintf(__VA_ARGS__))
#define ROS_ERROR_ONCE(...) MRS_LOG_ONCE(ERROR, ::mrs::formatPrintf(__VA_ARGS__))

#define ROS_DEBUG_STREAM(expr) MRS_LOG_AT(DEBUG, MRS_STREAM_TEXT(expr))
#define ROS_INFO_STREAM(expr) MRS_LOG_AT(INFO, MRS_STREAM_TEXT(expr))
#define ROS_WARN_STREAM(expr) MRS_LOG_AT(WARN, MRS_STREAM_TEXT(expr))
#define ROS_ERROR_STREAM(expr) MRS_LOG_AT(ERROR, MRS_STREAM_TEXT(expr))
#define ROS_FATAL_STREAM(expr) MRS_LOG_AT(FATAL, MRS_STREAM_TEXT(expr))

#define ROS_DEBUG_STREAM_THROTTLE(p, expr) MRS_LOG_THROTTLED(DEBUG, p, MRS_STREAM_TEXT(expr))
#define ROS_INFO_STREAM_THROTTLE(p, expr) MRS_LOG_THROTTLED(INFO, p, MRS_STREAM_TEXT(expr))
#define ROS_WARN_STREAM_THROTTLE(p, expr) MRS_LOG_THROTTLED(WARN, p, MRS_STREAM_TEXT(expr))
#define ROS_ERROR_STREAM_THROTTLE(p, expr) MRS_LOG_THROTTLED(ERROR, p, MRS_STREAM_TEXT(expr))
