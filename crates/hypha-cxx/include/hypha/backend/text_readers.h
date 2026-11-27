#pragma once

/// @file
/// Text readers over the C ABI. Each returns the text the C function wrote, or the
/// status it returned.

#include "hypha/hypha_ffi.h"
#include "hypha/status.h"

#include <cstddef>
#include <cstdint>
#include <expected>
#include <string>
#include <string_view>
#include <utility>

namespace hypha::backend {

/// A size query that is followed by a copy may see the value grow; retry this often.
inline constexpr int READ_ATTEMPTS = 2;

/// A length the buffer cannot hold is a backend fault.
inline std::expected<std::string, hypha::Status> textOfLength(std::string& buffer, size_t length)
{
    const bool fits = length <= buffer.size();
    if (!fits) {
        return std::unexpected(hypha::Status(hypha::Status::Backend));
    }
    buffer.resize(length);
    return std::move(buffer);
}

/// Asks for the size, then copies. @p call takes the leading arguments, then an output
/// buffer, its capacity and the length slot.
template <typename Call, typename... Leading>
inline std::expected<std::string, hypha::Status> readSizedText(Call call, Leading... leading)
{
    std::string buffer;
    for (int attempt = 0; attempt < READ_ATTEMPTS; ++attempt) {
        size_t              length = 0;
        const ::HyphaStatus sized = call(leading..., nullptr, 0, &length);
        const bool          sizeKnown = sized == HYPHA_STATUS_OK || sized == HYPHA_STATUS_TOO_LARGE;
        if (!sizeKnown) {
            return std::unexpected(hypha::Status(sized));
        }
        buffer.assign(length, '\0');
        const ::HyphaStatus copied = call(leading..., buffer.data(), buffer.size(), &length);
        if (copied == HYPHA_STATUS_OK) {
            return textOfLength(buffer, length);
        }
        if (copied != HYPHA_STATUS_TOO_LARGE) {
            return std::unexpected(hypha::Status(copied));
        }
    }
    return std::unexpected(hypha::Status(hypha::Status::TooLarge));
}

/// Starts with @p capacity bytes and retries once with the length the C ABI reports.
template <typename Call, typename... Leading>
inline std::expected<std::string, hypha::Status>
readGrownText(Call call, size_t capacity, Leading... leading)
{
    std::string   buffer(capacity, '\0');
    size_t        length = 0;
    ::HyphaStatus status = call(leading..., buffer.data(), buffer.size(), &length);
    const bool    needsMoreRoom = status == HYPHA_STATUS_TOO_LARGE;
    if (needsMoreRoom) {
        buffer.resize(length);
        status = call(leading..., buffer.data(), buffer.size(), &length);
    }
    if (status != HYPHA_STATUS_OK) {
        return std::unexpected(hypha::Status(status));
    }
    return textOfLength(buffer, length);
}

/// The reason of the last refusal on this thread, or an empty string.
inline std::string lastError()
{
    return readSizedText(&hypha_last_error).value_or(std::string{});
}

inline std::expected<std::string, hypha::Status> readNodeName(const HyphaNode* node)
{
    return readGrownText(::hypha_node_name, 64, node);
}

/// The newline-separated unread keys under @p prefix of @p config.
inline std::expected<std::string, hypha::Status>
readUnreadKeys(const HyphaConfig* config, std::string_view prefix)
{
    return readSizedText(::hypha_config_unread_keys, config, prefix.data(), prefix.size());
}

/// The newline-separated names of the tables directly under @p prefix of @p config.
inline std::expected<std::string, hypha::Status>
readTableNames(const HyphaConfig* config, std::string_view prefix)
{
    return readSizedText(::hypha_config_table_names, config, prefix.data(), prefix.size());
}

inline std::expected<std::string, hypha::Status>
readParamTopic(const HyphaNode* node, uint32_t direction, std::string_view name, uint32_t rule)
{
    return readGrownText(::hypha_param_topic, 256, node, direction, name.data(), name.size(), rule);
}

inline std::expected<std::string, hypha::Status> readSchemaLookup(
    const HyphaNode* node, std::string_view topic, std::string_view dataType, std::string_view md5
)
{
    return readSizedText(
        ::hypha_schema_lookup,
        node,
        topic.data(),
        topic.size(),
        dataType.data(),
        dataType.size(),
        md5.data(),
        md5.size()
    );
}

inline std::expected<std::string, hypha::Status>
readSubscriberTopic(const HyphaSubscriber* subscriber)
{
    return readGrownText(::hypha_subscriber_topic, 256, subscriber);
}

}  // namespace hypha::backend
