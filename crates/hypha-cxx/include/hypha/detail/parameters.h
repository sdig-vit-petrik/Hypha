#pragma once

/// @file
/// The typed reads and validation under hypha::config(). Internal: config.h is the API.

#include "hypha/backend/config_readers.h"
#include "hypha/backend/session.h"
#include "hypha/backend/text_readers.h"
#include "hypha/status.h"

#include <array>
#include <cmath>
#include <cstddef>
#include <cstdint>
#include <expected>
#include <format>
#include <limits>
#include <optional>
#include <stdexcept>
#include <string>
#include <type_traits>
#include <utility>
#include <vector>

namespace hypha {

/// @brief A parameter that is missing, of the wrong type or outside its constraint.
class ParameterError : public ConfigError
{
  public:
    using ConfigError::ConfigError;
};

namespace detail {

/// An integer type config() reads: not bool and not a character type.
template <typename T>
concept ParamInteger = std::is_integral_v<T> && !std::is_same_v<T, bool> && !std::is_same_v<T, char>
                    && !std::is_same_v<T, wchar_t> && !std::is_same_v<T, char8_t>
                    && !std::is_same_v<T, char16_t> && !std::is_same_v<T, char32_t>;

/// The types a parameter read converts to: bool, an integer, float, double or std::string.
template <typename T>
concept ConfigValueType =
    std::is_same_v<T, bool> || std::is_same_v<T, float> || std::is_same_v<T, double>
    || std::is_same_v<T, std::string> || ParamInteger<T>;

/// @brief The stored value as T, or std::nullopt for another kind or a value out of the
///        range of T. A double also reads a whole number.
template <typename T>
std::optional<T> convertConfigValue(const HyphaConfigValue& value)
{
    static_assert(
        ConfigValueType<T>, "parameters read bool, an integer, float, double or std::string"
    );
    if constexpr (std::is_same_v<T, bool>) {
        if (value.kind == HYPHA_CONFIG_VALUE_BOOL) {
            return value.bool_value != 0;
        }
        return std::nullopt;
    } else if constexpr (ParamInteger<T>) {
        const bool fits = value.kind == HYPHA_CONFIG_VALUE_INT && std::in_range<T>(value.int_value);
        if (fits) {
            return static_cast<T>(value.int_value);
        }
        return std::nullopt;
    } else if constexpr (std::is_same_v<T, double>) {
        if (value.kind == HYPHA_CONFIG_VALUE_FLOAT) {
            return value.float_value;
        }
        if (value.kind == HYPHA_CONFIG_VALUE_INT) {
            return static_cast<double>(value.int_value);
        }
        return std::nullopt;
    } else if constexpr (std::is_same_v<T, float>) {
        const std::optional<double> number = convertConfigValue<double>(value);
        if (!number) {
            return std::nullopt;
        }
        const float narrowed = static_cast<float>(*number);
        const bool  overflowed = std::isfinite(*number) && !std::isfinite(narrowed);
        if (overflowed) {
            return std::nullopt;
        }
        return narrowed;
    } else {
        static_assert(std::is_same_v<T, std::string>);
        if (value.kind != HYPHA_CONFIG_VALUE_TEXT) {
            return std::nullopt;
        }
        if (value.text_len == 0) {
            return std::string{};
        }
        return std::string(value.text, value.text_len);
    }
}

/// Record a parameter read with its fallback as the default.
template <typename T>
void recordParameter(const std::string& name, const T& value, const T& fallback)
{
    backend::recordParameterRow(
        name, std::format("{}", value), std::format("{}", fallback), HYPHA_CONFIG_SOURCE_PARAMETER
    );
}

/// Record a parameter read that took its fallback.
template <typename T>
void recordDefaultParameter(const std::string& name, const T& fallback)
{
    backend::recordParameterRow(
        name, std::format("{}", fallback), std::format("{}", fallback), HYPHA_CONFIG_SOURCE_DEFAULT
    );
}

template <typename T>
void recordRequiredParameter(const std::string& name, const T& value)
{
    backend::recordParameterRow(
        name, std::format("{}", value), std::nullopt, HYPHA_CONFIG_SOURCE_PARAMETER
    );
}

[[noreturn]] inline void reportMalformedParameter(const std::string& name)
{
    throw ParameterError(
        "hypha: parameter '" + name
        + "' is set, but its value has another type than the node asked for, or "
          "is out of that type's range"
    );
}

/// A missing list, index or field is NotAvailable; a wrong kind is InvalidArgument.
inline Status listReadStatus(HyphaParamResult result)
{
    switch (result) {
        case HYPHA_PARAM_RESULT_OK:
            return Status::Ok;
        case HYPHA_PARAM_RESULT_ABSENT:
            return Status::NotAvailable;
        case HYPHA_PARAM_RESULT_MALFORMED:
        case HYPHA_PARAM_RESULT_INVALID_ARGUMENT:
            break;
    }
    return Status::InvalidArgument;
}

/// @brief Read the scalar parameter @p name into @p out. A value of another type, or out of
///        the range of T, is MALFORMED and leaves @p out unchanged.
template <typename T>
HyphaParamResult readParameterValue(const std::string& name, T& out)
{
    using Read = std::pair<HyphaParamResult, HyphaConfigValue>;
    const Read              read = backend::readConfigValue(name);
    const HyphaParamResult  result = read.first;
    const HyphaConfigValue& stored = read.second;
    if (result != HYPHA_PARAM_RESULT_OK) {
        return result;
    }
    const std::optional<T> converted = convertConfigValue<T>(stored);
    if (!converted) {
        return HYPHA_PARAM_RESULT_MALFORMED;
    }
    out = *converted;
    return HYPHA_PARAM_RESULT_OK;
}

/// @brief Read element @p index of the list @p name, or its @p field, as readParameterValue.
template <typename T>
HyphaParamResult
readListValue(const std::string& name, size_t index, const std::string& field, T& out)
{
    using Read = std::pair<HyphaParamResult, HyphaConfigValue>;
    const Read              read = backend::readListConfigValue(name, index, field);
    const HyphaParamResult  result = read.first;
    const HyphaConfigValue& stored = read.second;
    if (result != HYPHA_PARAM_RESULT_OK) {
        return result;
    }
    const std::optional<T> converted = convertConfigValue<T>(stored);
    if (!converted) {
        return HYPHA_PARAM_RESULT_MALFORMED;
    }
    out = *converted;
    return HYPHA_PARAM_RESULT_OK;
}

/// @brief Number of elements in the TOML array `name`. A missing name is NotAvailable, a
///        scalar InvalidArgument.
inline std::expected<size_t, Status> paramListLength(const std::string& name)
{
    size_t                 length = 0;
    const HyphaParamResult result = backend::paramListLength(name, &length);
    if (result != HYPHA_PARAM_RESULT_OK) {
        return std::unexpected(listReadStatus(result));
    }
    return length;
}

/// @brief Element `index` of the TOML array `name`, typed as T. A table element is read
///        through `field`, a dotted name such as "limits.max". Nested arrays are not
///        supported. A missing index or field is NotAvailable; a wrong kind or range
///        InvalidArgument.
template <typename T>
std::expected<T, Status>
paramListAt(const std::string& name, size_t index, const std::string& field = {})
{
    static_assert(
        ConfigValueType<T>,
        "ConfigView element reads take bool, an integer, float, double or std::string"
    );
    T                      value{};
    const HyphaParamResult result = readListValue<T>(name, index, field, value);
    if (result != HYPHA_PARAM_RESULT_OK) {
        return std::unexpected(listReadStatus(result));
    }
    return value;
}

template <typename T>
std::string expectedKindText()
{
    static_assert(ConfigValueType<T>);
    if constexpr (std::is_same_v<T, bool>) {
        return "a boolean";
    } else if constexpr (std::is_same_v<T, std::string>) {
        return "text";
    } else if constexpr (std::is_same_v<T, double>) {
        return "a number";
    } else if constexpr (std::is_same_v<T, float>) {
        return "a number that fits a float";
    } else if constexpr (std::is_signed_v<T>) {
        return std::format(
            "an integer in [{}, {}]",
            static_cast<int64_t>(std::numeric_limits<T>::min()),
            static_cast<int64_t>(std::numeric_limits<T>::max())
        );
    } else {
        return std::format(
            "an integer in [{}, {}]",
            static_cast<uint64_t>(std::numeric_limits<T>::min()),
            static_cast<uint64_t>(std::numeric_limits<T>::max())
        );
    }
}

inline std::string storedValueText(const std::pair<HyphaParamResult, HyphaConfigValue>& read)
{
    if (read.first == HYPHA_PARAM_RESULT_MALFORMED) {
        return "a table";
    }
    if (read.first != HYPHA_PARAM_RESULT_OK) {
        return "no value";
    }
    const HyphaConfigValue& value = read.second;
    switch (value.kind) {
        case HYPHA_CONFIG_VALUE_BOOL:
            return value.bool_value != 0 ? "true" : "false";
        case HYPHA_CONFIG_VALUE_INT:
            return std::format("{}", value.int_value);
        case HYPHA_CONFIG_VALUE_FLOAT:
            return std::format("{}", value.float_value);
        case HYPHA_CONFIG_VALUE_TEXT:
            return "\"" + std::string(value.text, value.text_len) + "\"";
        default:
            break;
    }
    return "an unknown value";
}

template <typename T>
std::string arrayElementRefusal(const std::string& list, size_t index)
{
    const std::pair<HyphaParamResult, HyphaConfigValue> read =
        backend::readListConfigValue(list, index, std::string{});
    return "hypha: parameter '" + list + "[" + std::to_string(index)
         + "]': expected " + expectedKindText<T>() + ", found " + storedValueText(read);
}

/// The first N of @p values as an std::array; the caller checked the length.
template <typename T, size_t N>
std::array<T, N> toArray(const std::vector<T>& values)
{
    std::array<T, N> result{};
    for (size_t index = 0; index < N; ++index) {
        result[index] = values[index];
    }
    return result;
}

template <typename T>
std::string arrayText(const std::vector<T>& values)
{
    std::string text = "[";
    for (size_t index = 0; index < values.size(); ++index) {
        if (index != 0) {
            text += ", ";
        }
        text += std::format("{}", static_cast<T>(values[index]));
    }
    return text + "]";
}

}  // namespace detail

}  // namespace hypha
