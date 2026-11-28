#pragma once

#include "hypha/hypha_ffi.h"

#include <cstdint>
#include <expected>
#include <stdexcept>
#include <string>
#include <string_view>
#include <type_traits>
#include <utility>

namespace hypha {

/// @brief The status of a framework call: the C ABI's `::HyphaStatus` as a C++ value.
///        It converts implicitly to and from the C enum.
struct Status
{
    enum Code : int32_t {
        Ok = HYPHA_STATUS_OK,
        InvalidArgument = HYPHA_STATUS_INVALID_ARGUMENT,
        InvalidTopic = HYPHA_STATUS_INVALID_TOPIC,
        PoolExhausted = HYPHA_STATUS_POOL_EXHAUSTED,
        BorrowLimit = HYPHA_STATUS_BORROW_LIMIT,
        TooLarge = HYPHA_STATUS_TOO_LARGE,
        Disconnected = HYPHA_STATUS_DISCONNECTED,
        Backend = HYPHA_STATUS_BACKEND,
        Panic = HYPHA_STATUS_PANIC,
        InvalidFrame = HYPHA_STATUS_INVALID_FRAME,
        InvalidOperation = HYPHA_STATUS_INVALID_OPERATION,
        NotAvailable = HYPHA_STATUS_NOT_AVAILABLE,
        InvalidSchema = HYPHA_STATUS_INVALID_SCHEMA,
        ResourceLimit = HYPHA_STATUS_RESOURCE_LIMIT,
        CorruptPayload = HYPHA_STATUS_CORRUPT_PAYLOAD,
        SequenceGap = HYPHA_STATUS_SEQUENCE_GAP,
        QosMismatch = HYPHA_STATUS_QOS_MISMATCH,
        Timeout = HYPHA_STATUS_TIMEOUT,
        InvalidState = HYPHA_STATUS_INVALID_STATE,
        ClockRewound = HYPHA_STATUS_CLOCK_REWOUND,
        ApplicationFailed = HYPHA_STATUS_APPLICATION_FAILED,
        LoanLimit = HYPHA_STATUS_LOAN_LIMIT,
    };

    Code code{Ok};

    constexpr Status() = default;

    constexpr Status(Code c) : code(c)
    {
    }

    constexpr Status(::HyphaStatus c) : code(static_cast<Code>(c))
    {
    }

    constexpr operator Code() const
    {
        return this->code;
    }

    constexpr operator ::HyphaStatus() const
    {
        return static_cast<::HyphaStatus>(this->code);
    }

    constexpr bool operator==(const Status& other) const = default;

    constexpr bool operator==(Code other) const
    {
        return this->code == other;
    }

    constexpr bool operator==(::HyphaStatus other) const
    {
        return this->code == static_cast<Code>(other);
    }

    constexpr int asInt() const
    {
        return static_cast<int>(this->code);
    }

    /// @brief The name of the code, such as "NotAvailable"; "Unknown" for any other value.
    constexpr std::string_view name() const
    {
        switch (this->code) {
            case Ok:
                return "Ok";
            case InvalidArgument:
                return "InvalidArgument";
            case InvalidTopic:
                return "InvalidTopic";
            case PoolExhausted:
                return "PoolExhausted";
            case BorrowLimit:
                return "BorrowLimit";
            case TooLarge:
                return "TooLarge";
            case Disconnected:
                return "Disconnected";
            case Backend:
                return "Backend";
            case Panic:
                return "Panic";
            case InvalidFrame:
                return "InvalidFrame";
            case InvalidOperation:
                return "InvalidOperation";
            case NotAvailable:
                return "NotAvailable";
            case InvalidSchema:
                return "InvalidSchema";
            case ResourceLimit:
                return "ResourceLimit";
            case CorruptPayload:
                return "CorruptPayload";
            case SequenceGap:
                return "SequenceGap";
            case QosMismatch:
                return "QosMismatch";
            case Timeout:
                return "Timeout";
            case InvalidState:
                return "InvalidState";
            case ClockRewound:
                return "ClockRewound";
            case ApplicationFailed:
                return "ApplicationFailed";
            case LoanLimit:
                return "LoanLimit";
        }
        return "Unknown";
    }
};

/// @brief A refused configuration: a restart cannot fix it. Thrown before the run,
///        runNode() returns kExitConfiguration for it.
class ConfigError : public std::runtime_error
{
  public:
    using std::runtime_error::runtime_error;
};

namespace detail {

/// @brief The text of a failed call: its context, the name of the status and, when there is
///        one, the reason that the library gives for it.
inline std::string
failureText(std::string_view context, Status status, std::string_view reason = {})
{
    std::string text = "hypha: " + std::string(context) + " failed: " + std::string(status.name());
    if (!reason.empty()) {
        text += ": " + std::string(reason);
    }
    return text;
}

/// @brief Throw the failure of @p context: ConfigError for QosMismatch, std::runtime_error
///        otherwise.
[[noreturn]] inline void throwFailure(std::string_view context, Status status)
{
    if (status == Status::QosMismatch) {
        throw ConfigError(failureText(context, status));
    }
    throw std::runtime_error(failureText(context, status));
}

}  // namespace detail

/// @brief The value of @p result, or an exception naming @p context and the status:
///        ConfigError for QosMismatch, std::runtime_error otherwise.
template <typename T>
    requires(!std::is_void_v<T>)
T require(std::expected<T, Status>&& result, std::string_view context = "setup")
{
    if (!result) {
        detail::throwFailure(context, result.error());
    }
    return std::move(*result);
}

/// @brief Nothing, or the exception of require(std::expected<T, Status>&&).
inline void require(const std::expected<void, Status>& result, std::string_view context = "setup")
{
    if (!result) {
        detail::throwFailure(context, result.error());
    }
}

/// @brief Throw the Cycle::start() error text as std::runtime_error.
inline void require(const std::expected<void, std::string>& result)
{
    if (!result) {
        throw std::runtime_error(result.error());
    }
}

/// @brief Nothing for Status::Ok, otherwise the exception of
///        require(std::expected<T, Status>&&).
inline void require(Status status, std::string_view context = "setup")
{
    if (status != Status::Ok) {
        detail::throwFailure(context, status);
    }
}

}  // namespace hypha
