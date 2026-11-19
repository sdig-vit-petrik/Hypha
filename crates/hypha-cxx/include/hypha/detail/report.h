#pragma once

/// @file
/// How hypha reports a fault. A deployment error throws at start-up; bad data from the
/// bus is counted, logged and dropped.

#include <atomic>
#include <cstdio>
#include <string>

#if !defined(HYPHA_NO_LOGGING)
#    include <hypha_log/hypha_log.h>
#endif

namespace hypha::detail {

#if !defined(HYPHA_NO_LOGGING)
/// The module of every line this header writes.
inline HyphaLogModule hyphaLogTuModule()
{
    static const HyphaLogModule module = ::hypha::log::detail::registerModule("hypha");
    return module;
}
#endif

/// How often a recurring fault repeats in the log, in seconds.
inline constexpr double FAULT_REPEAT_PERIOD_SECONDS = 10.0;

inline void info(const std::string& message)
{
#if !defined(HYPHA_NO_LOGGING)
    HYPHA_LOG_INFO("{}", message);
#else
    std::fprintf(stderr, "hypha: %s\n", message.c_str());
#endif
}

inline void warn(const std::string& message)
{
#if !defined(HYPHA_NO_LOGGING)
    HYPHA_LOG_WARN("{}", message);
#else
    std::fprintf(stderr, "hypha: %s\n", message.c_str());
#endif
}

inline void error(const std::string& message)
{
#if !defined(HYPHA_NO_LOGGING)
    HYPHA_LOG_ERROR("{}", message);
#else
    std::fprintf(stderr, "hypha: %s\n", message.c_str());
#endif
}

/// @brief A fault that recurs at message rate: counted always, logged at a
///        bounded rate with the running count attached.
class RecurringFault
{
  public:
    /// @return The total number of occurrences, including this one.
    uint64_t note(const std::string& message)
    {
        const uint64_t total = this->occurrences.fetch_add(1, std::memory_order_relaxed) + 1;
#if !defined(HYPHA_NO_LOGGING)
        HYPHA_LOG_ERROR_THROTTLE(FAULT_REPEAT_PERIOD_SECONDS, "{} (occurrence {})", message, total);
#else
        if (total == 1) {
            std::fprintf(stderr, "hypha: %s\n", message.c_str());
        }
#endif
        return total;
    }

  private:
    std::atomic<uint64_t> occurrences{0};
};

}  // namespace hypha::detail
