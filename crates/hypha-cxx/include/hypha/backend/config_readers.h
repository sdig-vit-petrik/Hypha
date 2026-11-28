#pragma once

#include "hypha/backend/session.h"
#include "hypha/backend/text_readers.h"
#include "hypha/detail/report.h"
#include "hypha/hypha_ffi.h"

#include <cstddef>
#include <cstdint>
#include <expected>
#include <optional>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace hypha::backend {

inline ::HyphaStatus
paramValidateNumber(double value, uint32_t constraint, double minimum, double maximum)
{
    return hypha_param_validate_number(value, constraint, minimum, maximum);
}

inline ::HyphaStatus paramValidateNonempty(const char* value, size_t valueLen)
{
    return hypha_param_validate_nonempty(value, valueLen);
}

/// @brief The stored value of the scalar parameter @p name. A TEXT value borrows the
///        configuration: copy it before the configuration is destroyed. Without a
///        configuration the result is ABSENT.
inline std::pair<HyphaParamResult, HyphaConfigValue> readConfigValue(const std::string& name)
{
    HyphaConfigValue   value{};
    const HyphaConfig* config = configHandle();
    if (config == nullptr) {
        return {HYPHA_PARAM_RESULT_ABSENT, value};
    }
    const HyphaParamResult result = ::hypha_config_value(config, name.data(), name.size(), &value);
    return {result, value};
}

/// @brief The stored value of element @p index of the list @p name, or of its @p field. The
///        text rule of readConfigValue applies.
inline std::pair<HyphaParamResult, HyphaConfigValue>
readListConfigValue(const std::string& name, size_t index, const std::string& field)
{
    HyphaConfigValue   value{};
    const HyphaConfig* config = configHandle();
    if (config == nullptr) {
        return {HYPHA_PARAM_RESULT_ABSENT, value};
    }
    const HyphaParamResult result = ::hypha_config_list_value(
        config, name.data(), name.size(), index, field.data(), field.size(), &value
    );
    return {result, value};
}

/// @brief The length of the TOML array @p name. A missing name is ABSENT.
inline HyphaParamResult paramListLength(const std::string& name, size_t* outLen)
{
    return ::hypha_config_list_len(configHandle(), name.data(), name.size(), outLen);
}

/// The lines of @p text, which is empty for no line.
inline std::vector<std::string> splitLines(const std::string& text)
{
    std::vector<std::string> lines;
    if (text.empty()) {
        return lines;
    }
    size_t start = 0;
    while (start <= text.size()) {
        const size_t newline = text.find('\n', start);
        if (newline == std::string::npos) {
            lines.push_back(text.substr(start));
            break;
        }
        lines.push_back(text.substr(start, newline - start));
        start = newline + 1;
    }
    return lines;
}

/// @brief Every unread key under @p prefix in the active configuration, sorted.
inline std::vector<std::string> unreadParameterKeys(const std::string& prefix)
{
    const HyphaConfig* config = configHandle();
    if (config == nullptr) {
        return {};
    }
    const std::expected<std::string, Status> text = readUnreadKeys(config, prefix);
    return text ? splitLines(*text) : std::vector<std::string>{};
}

/// @brief The names of the tables directly under @p prefix, sorted; none without a
///        configuration.
inline std::vector<std::string> tableNames(const std::string& prefix)
{
    const HyphaConfig* config = configHandle();
    if (config == nullptr) {
        return {};
    }
    const std::expected<std::string, Status> text = readTableNames(config, prefix);
    return text ? splitLines(*text) : std::vector<std::string>{};
}

/// @brief Marks @p key, a key or a table, as set on purpose and not read, for @p reason.
///        Without a configuration nothing is set, so there is nothing to mark: Ok.
inline ::HyphaStatus ignoreParameter(const std::string& key, const std::string& reason)
{
    const HyphaConfig* config = configHandle();
    if (config == nullptr) {
        return HYPHA_STATUS_OK;
    }
    return ::hypha_config_ignore(config, key.data(), key.size(), reason.data(), reason.size());
}

/// @brief The file name that the text parameter @p key holds, resolved against the
///        directory of the file that wrote @p key. InvalidState without a loaded
///        configuration, InvalidArgument when @p key holds no text.
inline std::expected<std::string, Status> resolveConfigKey(const std::string& key)
{
    const HyphaConfig* config = configHandle();
    if (config == nullptr) {
        return std::unexpected(Status(HYPHA_STATUS_INVALID_STATE));
    }
    return readSizedText(::hypha_config_resolve_key, config, key.data(), key.size());
}

/// Record one configuration row of the node introspection. InvalidState without a
/// configuration.
inline ::HyphaStatus recordConfigRow(
    const std::string&                key,
    const std::string&                valueText,
    const std::optional<std::string>& defaultText,
    uint32_t                          source
)
{
    HyphaNode*  node = nodeHandle();
    const char* defaultPointer = defaultText ? defaultText->c_str() : nullptr;
    if (node != nullptr) {
        return hypha_node_record_config(
            node, key.c_str(), valueText.c_str(), defaultPointer, source
        );
    }
    const HyphaConfig* config = configHandle();
    return config == nullptr ? HYPHA_STATUS_INVALID_STATE
                             : hypha_config_record_config(
                                   config, key.c_str(), valueText.c_str(), defaultPointer, source
                               );
}

/// Record a parameter read in the node introspection. A refused row is logged.
inline void recordParameterRow(
    const std::string&                name,
    const std::string&                valueText,
    const std::optional<std::string>& defaultText,
    uint32_t                          source
)
{
    const ::HyphaStatus status = recordConfigRow(name, valueText, defaultText, source);
    if (status != HYPHA_STATUS_OK) {
        hypha::detail::error(
            "hypha: the node introspection refused parameter '" + name + "': status "
            + std::to_string(static_cast<int>(status))
        );
    }
}

/// The topic of the port `name` in `ports.inputs.<name>`, or `ports.outputs.<name>` when
/// @p output is set. Empty text is a disabled endpoint.
inline std::expected<std::string, Status>
paramTopic(const std::string& name, uint32_t rule, bool output)
{
    HyphaNode* node = nodeHandle();
    if (node == nullptr) {
        return std::unexpected(Status(HYPHA_STATUS_INVALID_STATE));
    }
    const uint32_t direction = output ? HYPHA_PORT_OUTPUT : HYPHA_PORT_INPUT;
    return readParamTopic(node, direction, name, rule);
}

inline ::HyphaStatus validatePublicTopicName(std::string_view topic)
{
    return hypha_topic_validate_public_name(topic.data(), topic.size());
}

}  // namespace hypha::backend
