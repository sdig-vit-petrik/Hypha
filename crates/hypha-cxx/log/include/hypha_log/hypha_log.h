#pragma once

/// @file
/// The C++ logging macros, a thin shim over the `hypha_log_*` C functions.
///
/// @code
/// #include <hypha_log/hypha_log.h>
///
/// HYPHA_LOG_MODULE_INIT("attitude_node")
///
/// HYPHA_LOG_INFO("node started: rate={} Hz", rate);
/// HYPHA_LOG_WARN_THROTTLE(1.0, "waiting for IMU data");
/// @endcode
///
/// Put HYPHA_LOG_MODULE_INIT before the first log macro of a translation unit; a macro
/// used before it writes under the module `default`.
///
/// The macros, `<LEVEL>` one of TRACE, DEBUG, INFO, WARN, ERROR, FATAL:
/// - `HYPHA_LOG_<LEVEL>(fmt, ...)` writes one entry. `fmt` is a std::format_string, so the
///   compiler checks it. FATAL writes an entry; it does not stop the process.
/// - `HYPHA_LOG_<LEVEL>_THROTTLE(period_s, fmt, ...)`: the first call writes, later calls
///   at most once per period.
/// - `HYPHA_LOG_<LEVEL>_WHEN(state, initial, fmt, ...)` writes when `state` changes from
///   false to true at that call site; the first call compares `state` with `initial`.
/// - `HYPHA_LOG_<LEVEL>_WHEN_THROTTLE(state, initial, period_s, fmt, ...)` writes on the
///   change and, while `state` stays true, at most once per period.
///
/// A macro evaluates its format arguments only when the line is written. Throttling uses
/// host monotonic time, never framework time.

#include "hypha/hypha_ffi.h"
#include "hypha_log/detail/call_site.h"

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <exception>
#include <expected>
#include <format>
#include <string>
#include <string_view>
#include <utility>

namespace hypha::log {

}  // namespace hypha::log

namespace hypha::log::detail {

inline constexpr std::size_t MODULE_NAME_MAX_BYTES = 64;

/// A module name has 1 to 64 bytes from A-Z, a-z, 0-9, `_` and `/`.
constexpr bool isValidModuleName(std::string_view name)
{
    const bool hasValidLength = !name.empty() && name.size() <= MODULE_NAME_MAX_BYTES;
    if (!hasValidLength) {
        return false;
    }
    for (const char character : name) {
        const bool isLetter =
            (character >= 'A' && character <= 'Z') || (character >= 'a' && character <= 'z');
        const bool isDigit = character >= '0' && character <= '9';
        const bool isAllowed = isLetter || isDigit || character == '_' || character == '/';
        if (!isAllowed) {
            return false;
        }
    }
    return true;
}

/// Declared and never defined: a call in a constant expression is a compile error.
void invalidLogModuleName();

consteval std::string_view checkedModuleName(std::string_view name)
{
    if (!isValidModuleName(name)) {
        invalidLogModuleName();
    }
    return name;
}

/// @brief Register @p name with the engine; abort on failure.
inline HyphaLogModule registerModule(std::string_view name)
{
    HyphaLogModule      module = 0;
    const ::HyphaStatus status = hypha_log_module_register(name.data(), name.size(), &module);
    if (status == HYPHA_STATUS_OK) {
        return module;
    }
    std::fprintf(
        stderr,
        "hypha_log: cannot register module \"%.*s\": status %d\n",
        static_cast<int>(name.size()),
        name.data(),
        static_cast<int>(status)
    );
    std::abort();
}

inline bool enabled(HyphaLogModule module, HyphaLogLevel level)
{
    return hypha_log_enabled(module, level) == 1;
}

/// @brief Format and write one entry. A formatter that throws drops the entry.
template <typename... Args>
void write(
    HyphaLogModule module, HyphaLogLevel level, std::format_string<Args...> format, Args&&... args
)
{
    try {
        const std::string text = std::format(format, std::forward<Args>(args)...);
        hypha_log_write(module, level, text.data(), text.size());
    } catch (const std::exception&) {
        countRejectedLine(module);
    }
}

}  // namespace hypha::log::detail

/// The module of a translation unit without HYPHA_LOG_MODULE_INIT, whose non-template
/// overload wins over this template.
template <int Unused = 0>
inline HyphaLogModule hyphaLogTuModule()
{
    static const HyphaLogModule module = ::hypha::log::detail::registerModule("default");
    return module;
}

/// Select the module of this translation unit. Use at file scope.
#define HYPHA_LOG_MODULE_INIT(name)                                                                \
    namespace {                                                                                    \
    [[maybe_unused]] inline HyphaLogModule hyphaLogTuModule()                                      \
    {                                                                                              \
        static const HyphaLogModule module =                                                       \
            ::hypha::log::detail::registerModule(::hypha::log::detail::checkedModuleName(name));   \
        return module;                                                                             \
    }                                                                                              \
    }

#define HYPHA_LOG_IMPL_WRITE(level, ...)                                                           \
    do {                                                                                           \
        const HyphaLogModule hyphaLogModule = hyphaLogTuModule();                                  \
        if (::hypha::log::detail::enabled(hyphaLogModule, level)) {                                \
            ::hypha::log::detail::write(hyphaLogModule, level, __VA_ARGS__);                       \
        }                                                                                          \
    } while (false)

#define HYPHA_LOG_IMPL_THROTTLE(level, period, ...)                                                \
    do {                                                                                           \
        const HyphaLogModule hyphaLogModule = hyphaLogTuModule();                                  \
        if (::hypha::log::detail::enabled(hyphaLogModule, level)) {                                \
            static std::atomic<int64_t> hyphaLogLastWrite{::hypha::log::detail::NEVER_WRITTEN};    \
            if (::hypha::log::detail::throttlePermits(                                             \
                    hyphaLogModule, hyphaLogLastWrite, (period)                                    \
                )) {                                                                               \
                ::hypha::log::detail::write(hyphaLogModule, level, __VA_ARGS__);                   \
            }                                                                                      \
        }                                                                                          \
    } while (false)

#define HYPHA_LOG_IMPL_WHEN(level, state, initial, ...)                                            \
    do {                                                                                           \
        const HyphaLogModule hyphaLogModule = hyphaLogTuModule();                                  \
        if (::hypha::log::detail::enabled(hyphaLogModule, level)) {                                \
            static ::hypha::log::detail::TransitionState hyphaLogState;                            \
            if (::hypha::log::detail::transitionPermits(                                           \
                    hyphaLogState, static_cast<bool>(state), static_cast<bool>(initial)            \
                )) {                                                                               \
                ::hypha::log::detail::write(hyphaLogModule, level, __VA_ARGS__);                   \
            }                                                                                      \
        }                                                                                          \
    } while (false)

#define HYPHA_LOG_IMPL_WHEN_THROTTLE(level, state, initial, period, ...)                           \
    do {                                                                                           \
        const HyphaLogModule hyphaLogModule = hyphaLogTuModule();                                  \
        if (::hypha::log::detail::enabled(hyphaLogModule, level)) {                                \
            static ::hypha::log::detail::TransitionState hyphaLogState;                            \
            if (::hypha::log::detail::transitionThrottlePermits(                                   \
                    hyphaLogModule,                                                                \
                    hyphaLogState,                                                                 \
                    static_cast<bool>(state),                                                      \
                    static_cast<bool>(initial),                                                    \
                    (period)                                                                       \
                )) {                                                                               \
                ::hypha::log::detail::write(hyphaLogModule, level, __VA_ARGS__);                   \
            }                                                                                      \
        }                                                                                          \
    } while (false)

#define HYPHA_LOG_TRACE(...) HYPHA_LOG_IMPL_WRITE(HYPHA_LOG_LEVEL_TRACE, __VA_ARGS__)
#define HYPHA_LOG_TRACE_THROTTLE(period, ...)                                                      \
    HYPHA_LOG_IMPL_THROTTLE(HYPHA_LOG_LEVEL_TRACE, period, __VA_ARGS__)
#define HYPHA_LOG_TRACE_WHEN(state, initial, ...)                                                  \
    HYPHA_LOG_IMPL_WHEN(HYPHA_LOG_LEVEL_TRACE, state, initial, __VA_ARGS__)
#define HYPHA_LOG_TRACE_WHEN_THROTTLE(state, initial, period, ...)                                 \
    HYPHA_LOG_IMPL_WHEN_THROTTLE(HYPHA_LOG_LEVEL_TRACE, state, initial, period, __VA_ARGS__)
#define HYPHA_LOG_DEBUG(...) HYPHA_LOG_IMPL_WRITE(HYPHA_LOG_LEVEL_DEBUG, __VA_ARGS__)
#define HYPHA_LOG_DEBUG_THROTTLE(period, ...)                                                      \
    HYPHA_LOG_IMPL_THROTTLE(HYPHA_LOG_LEVEL_DEBUG, period, __VA_ARGS__)
#define HYPHA_LOG_DEBUG_WHEN(state, initial, ...)                                                  \
    HYPHA_LOG_IMPL_WHEN(HYPHA_LOG_LEVEL_DEBUG, state, initial, __VA_ARGS__)
#define HYPHA_LOG_DEBUG_WHEN_THROTTLE(state, initial, period, ...)                                 \
    HYPHA_LOG_IMPL_WHEN_THROTTLE(HYPHA_LOG_LEVEL_DEBUG, state, initial, period, __VA_ARGS__)
#define HYPHA_LOG_INFO(...) HYPHA_LOG_IMPL_WRITE(HYPHA_LOG_LEVEL_INFO, __VA_ARGS__)
#define HYPHA_LOG_INFO_THROTTLE(period, ...)                                                       \
    HYPHA_LOG_IMPL_THROTTLE(HYPHA_LOG_LEVEL_INFO, period, __VA_ARGS__)
#define HYPHA_LOG_INFO_WHEN(state, initial, ...)                                                   \
    HYPHA_LOG_IMPL_WHEN(HYPHA_LOG_LEVEL_INFO, state, initial, __VA_ARGS__)
#define HYPHA_LOG_INFO_WHEN_THROTTLE(state, initial, period, ...)                                  \
    HYPHA_LOG_IMPL_WHEN_THROTTLE(HYPHA_LOG_LEVEL_INFO, state, initial, period, __VA_ARGS__)
#define HYPHA_LOG_WARN(...) HYPHA_LOG_IMPL_WRITE(HYPHA_LOG_LEVEL_WARN, __VA_ARGS__)
#define HYPHA_LOG_WARN_THROTTLE(period, ...)                                                       \
    HYPHA_LOG_IMPL_THROTTLE(HYPHA_LOG_LEVEL_WARN, period, __VA_ARGS__)
#define HYPHA_LOG_WARN_WHEN(state, initial, ...)                                                   \
    HYPHA_LOG_IMPL_WHEN(HYPHA_LOG_LEVEL_WARN, state, initial, __VA_ARGS__)
#define HYPHA_LOG_WARN_WHEN_THROTTLE(state, initial, period, ...)                                  \
    HYPHA_LOG_IMPL_WHEN_THROTTLE(HYPHA_LOG_LEVEL_WARN, state, initial, period, __VA_ARGS__)
#define HYPHA_LOG_ERROR(...) HYPHA_LOG_IMPL_WRITE(HYPHA_LOG_LEVEL_ERROR, __VA_ARGS__)
#define HYPHA_LOG_ERROR_THROTTLE(period, ...)                                                      \
    HYPHA_LOG_IMPL_THROTTLE(HYPHA_LOG_LEVEL_ERROR, period, __VA_ARGS__)
#define HYPHA_LOG_ERROR_WHEN(state, initial, ...)                                                  \
    HYPHA_LOG_IMPL_WHEN(HYPHA_LOG_LEVEL_ERROR, state, initial, __VA_ARGS__)
#define HYPHA_LOG_ERROR_WHEN_THROTTLE(state, initial, period, ...)                                 \
    HYPHA_LOG_IMPL_WHEN_THROTTLE(HYPHA_LOG_LEVEL_ERROR, state, initial, period, __VA_ARGS__)
#define HYPHA_LOG_FATAL(...) HYPHA_LOG_IMPL_WRITE(HYPHA_LOG_LEVEL_FATAL, __VA_ARGS__)
#define HYPHA_LOG_FATAL_THROTTLE(period, ...)                                                      \
    HYPHA_LOG_IMPL_THROTTLE(HYPHA_LOG_LEVEL_FATAL, period, __VA_ARGS__)
#define HYPHA_LOG_FATAL_WHEN(state, initial, ...)                                                  \
    HYPHA_LOG_IMPL_WHEN(HYPHA_LOG_LEVEL_FATAL, state, initial, __VA_ARGS__)
#define HYPHA_LOG_FATAL_WHEN_THROTTLE(state, initial, period, ...)                                 \
    HYPHA_LOG_IMPL_WHEN_THROTTLE(HYPHA_LOG_LEVEL_FATAL, state, initial, period, __VA_ARGS__)
