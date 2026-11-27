#pragma once

/// @file
/// The type-erased message shape of the raw endpoints, and the run outcome.

#include "hypha/detail/report.h"
#include "hypha/status.h"
#include "hypha_base/clock.h"

#include <cstdint>
#include <optional>
#include <span>
#include <string>
#include <string_view>
#include <vector>

namespace hypha {

/// @brief Why a run returned.
enum class StopReason : uint8_t {
    /// The application asked for the stop.
    Requested = 0,
    /// Building the native attachments failed. No callback ran.
    SetupFailed = 1,
    /// The run failed after it started.
    RuntimeFailed = 2,
    /// A bounded run reached its time limit. No stop was asked for.
    Deadline = 3,
};

/// @brief The name of @p reason, such as "Deadline"; "Unknown" for any other value.
constexpr std::string_view name(StopReason reason)
{
    switch (reason) {
        case StopReason::Requested:
            return "Requested";
        case StopReason::SetupFailed:
            return "SetupFailed";
        case StopReason::RuntimeFailed:
            return "RuntimeFailed";
        case StopReason::Deadline:
            return "Deadline";
    }
    return "Unknown";
}

/// @brief The outcome of one blocking run: whether the call was refused, and how the run
///        ended.
class [[nodiscard]] RunResult
{
  public:
    /// The call was refused. No run happened.
    static RunResult apiFailure(Status status)
    {
        RunResult result;
        result.callStatus = status;
        result.stopReason = StopReason::SetupFailed;
        result.runStatus = status;
        return result;
    }

    /// The call completed. The report says how the run itself ended.
    static RunResult fromReport(const HyphaWaitSetRunReport& report)
    {
        RunResult result;
        result.callStatus = HYPHA_STATUS_OK;
        result.stopReason = static_cast<StopReason>(report.reason);
        result.runStatus = Status(static_cast<::HyphaStatus>(report.status));
        result.attachment = report.attachment;
        return result;
    }

    /// True only when the call succeeded and the run stopped as requested or at its time limit.
    bool ok() const
    {
        const bool callSucceeded = this->callStatus == HYPHA_STATUS_OK;
        const bool stoppedNormally =
            this->stopReason == StopReason::Requested || this->stopReason == StopReason::Deadline;
        return callSucceeded && stoppedNormally;
    }

    Status call() const
    {
        return this->callStatus;
    }

    StopReason reason() const
    {
        return this->stopReason;
    }

    Status status() const
    {
        return this->runStatus;
    }

    uint64_t failedAttachment() const
    {
        return this->attachment;
    }

  private:
    Status     callStatus = HYPHA_STATUS_OK;
    StopReason stopReason = StopReason::Requested;
    Status     runStatus = HYPHA_STATUS_OK;
    uint64_t   attachment = 0;
};

/// @brief One bus message with its topic, schema hash, transport timing, and
///        exact wire payload. `bytes` is never re-serialized.
struct RawSample
{
    /// The topic of the sample. Valid only for the duration of the callback, as `bytes`.
    std::string_view topic;

    /// The wire payload, not copied. Valid only for the duration of the callback.
    std::span<const uint8_t> bytes;

    Time transportSent;

    /// Framework time at which the publisher sent the frame.
    Time frameworkSent;

    /// Time between the send and this delivery, on the host monotonic clock.
    Duration transportAge;

    /// 64-bit schema hash prefix derived from the message definition's MD5.
    uint64_t schemaHash{0};

    /// The delivery of a WaitSet callback, which hypha_delivery_retain keeps; null
    /// elsewhere. Valid only for the duration of the callback.
    const HyphaDelivery* delivery{nullptr};
};

/// @brief Description of a published or subscribed topic endpoint on a node.
struct TopicEndpoint
{
    std::string topic;
    std::string dataType;
    std::string md5;
};

/// @brief One process's presence on the bus and its published endpoints.
struct NodeInfo
{
    std::string                name;  ///< Empty when the backend could not resolve it.
    std::string                executable;  ///< Empty when the backend could not resolve it.
    int32_t                    pid{0};
    bool                       alive{true};
    std::vector<TopicEndpoint> publications;
    std::vector<TopicEndpoint> subscriptions;
};

/// @brief One named counter of the node or the process. The name is a copy.
struct Metric
{
    std::string name;
    uint64_t    value{0};
};

/// @brief The counters of one subscriber, read one by one, not at one instant. Each but
///        `loanBudget`, `loanHeld` and `otherSchemas` only grows, and wraps.
struct SubscriberMetrics
{
    /// Frames refused because their sequence was not newer than the last accepted one of
    /// their publisher.
    uint64_t sequenceGap{0};
    /// Frames whose layout is not a hypha frame.
    uint64_t invalidFrame{0};
    /// Frames whose CRC-32C does not match.
    uint64_t corruptPayload{0};
    /// Frames whose header names another schema than the stream they arrived on: a fault
    /// of their sender. A frame of another type of the topic is on another stream.
    uint64_t schemaMismatch{0};
    /// Frames sent later than the host clock reads.
    uint64_t invalidTimestamp{0};
    /// The owning samples the subscriber may hold at once.
    uint64_t loanBudget{0};
    /// The owning samples it holds now.
    uint64_t loanHeld{0};
    /// Owning receives refused because the budget was full.
    uint64_t loanRefused{0};
    /// The other schemas (MD5s) of the topic that a live publisher sends, as the node's
    /// housekeeping last found them, at most a second old while the node runs: a fact about
    /// the topic, not a fault. Empty before the first check, and for a subscriber of no
    /// single stream. Nonzero beside a publisher of the own version means a publisher of
    /// another version is on the topic.
    std::optional<uint64_t> otherSchemas;

    bool operator==(const SubscriberMetrics&) const = default;
};

}  // namespace hypha
