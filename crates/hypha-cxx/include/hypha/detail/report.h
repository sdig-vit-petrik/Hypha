#pragma once

/// @file
/// How hypha reports a fault. A deployment error throws at start-up; bad data from the
/// bus is counted, logged and dropped.

#include <atomic>
#include <cstdint>
#include <cstdio>
#include <string>
#include <string_view>

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

/// @brief What Publisher::publishOrWarn() says about a refused message: one warning a
///        second for each publisher, with the count of refusals so far. Copies of a
///        publisher share one.
class RefusalWarning
{
  public:
    void note(const std::string& topic, std::string_view status)
    {
        const uint64_t total = this->refused_.fetch_add(1, std::memory_order_relaxed) + 1;
#if !defined(HYPHA_NO_LOGGING)
        const HyphaLogModule module = hyphaLogTuModule();
        if (::hypha::log::detail::enabled(module, HYPHA_LOG_LEVEL_WARN)
            && ::hypha::log::detail::throttlePermits(
                module, this->lastWrite_, WARNING_PERIOD_SECONDS
            )) {
            ::hypha::log::detail::write(
                module,
                HYPHA_LOG_LEVEL_WARN,
                "topic '{}': message not sent: {} ({} refused so far)",
                topic,
                status,
                total
            );
        }
#else
        if (total == 1) {
            std::fprintf(
                stderr,
                "hypha: topic '%s': message not sent: %.*s\n",
                topic.c_str(),
                static_cast<int>(status.size()),
                status.data()
            );
        }
#endif
    }

  private:
    static constexpr double WARNING_PERIOD_SECONDS = 1.0;

    std::atomic<uint64_t> refused_{0};
#if !defined(HYPHA_NO_LOGGING)
    std::atomic<int64_t> lastWrite_{::hypha::log::detail::NEVER_WRITTEN};
#endif
};

}  // namespace hypha::detail
