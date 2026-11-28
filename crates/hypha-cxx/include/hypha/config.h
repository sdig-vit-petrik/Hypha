#pragma once

#include "hypha/backend/config_readers.h"
#include "hypha/backend/session.h"
#include "hypha/constraints.h"
#include "hypha/detail/parameters.h"

#include <array>
#include <concepts>
#include <cstddef>
#include <cstdint>
#include <format>
#include <iterator>
#include <optional>
#include <stdexcept>
#include <string>
#include <type_traits>
#include <utility>
#include <vector>

namespace hypha {

class ConfigList;
template <typename T>
struct Matrix;

/// A scalar configuration scope, or one indexed table entry.
class ConfigView
{
  public:
    explicit ConfigView(std::string prefix = {}) : prefix_(std::move(prefix))
    {
    }

    ConfigView(std::string list, size_t index) : prefix_(std::move(list)), index_(index)
    {
    }

    std::string name(const std::string& key = {}) const
    {
        std::string result = this->prefix_;
        if (this->index_) {
            result += "[" + std::to_string(*this->index_) + "]";
        }
        if (result.empty()) {
            return key;
        }
        if (!key.empty()) {
            result += "." + key;
        }
        return result;
    }

    /// @brief The last segment of this scope's name, `cmd_vel` for
    ///        `config("ports.inputs.cmd_vel")`; the index in brackets for a list entry.
    std::string key() const
    {
        if (this->index_) {
            return "[" + std::to_string(*this->index_) + "]";
        }
        return this->prefix_.substr(this->prefix_.rfind('.') + 1);
    }

    template <typename T, typename Constraint = detail::Unconstrained>
    T required(const std::string& key, const Constraint& constraint = {}) const
    {
        const std::expected<T, Status> value = this->read<T>(key);
        if (!value) {
            this->refuse(key, value.error());
        }
        this->validate(key, *value, constraint);
        detail::recordRequiredParameter(this->name(key), *value);
        return *value;
    }

    template <typename T, typename Constraint = detail::Unconstrained>
        requires requires(const Constraint& constraint, const T& value) {
            constraint.accepts(value);
        }
    std::optional<T> optional(const std::string& key, const Constraint& constraint = {}) const
    {
        const std::expected<T, Status> value = this->read<T>(key);
        if (!value) {
            if (value.error() == Status::NotAvailable) {
                return std::nullopt;
            }
            this->refuse(key, value.error());
        }
        this->validate(key, *value, constraint);
        detail::recordRequiredParameter(this->name(key), *value);
        return *value;
    }

    template <typename T, typename Constraint = detail::Unconstrained>
    T optional(
        const std::string& key, const T& defaultValue, const Constraint& constraint = {}
    ) const
    {
        const std::expected<T, Status> value = this->read<T>(key);
        if (!value) {
            if (value.error() != Status::NotAvailable) {
                this->refuse(key, value.error());
            }
            this->validate(key, defaultValue, constraint);
            detail::recordDefaultParameter(this->name(key), defaultValue);
            return defaultValue;
        }
        this->validate(key, *value, constraint);
        detail::recordParameter(this->name(key), *value, defaultValue);
        return *value;
    }

    /// @brief The required file name @p key, resolved against the directory of the file
    ///        that wrote it; an absolute name stays. @throws ParameterError when unset,
    ///        empty or not text.
    std::string path(const std::string& key) const
    {
        const std::string name = this->required<std::string>(key, constraints::nonempty());
        const std::expected<std::string, Status> resolved =
            backend::resolveConfigKey(this->name(key));
        if (!resolved) {
            throw ParameterError(
                "hypha: parameter '" + this->name(key) + "' is not a usable file name"
            );
        }
        return *resolved;
    }

    /// @brief The list @p key, of scalars or of tables, also inside a list entry.
    /// @throws ParameterError when not a list, or unset (list() only).
    ConfigList                list(const std::string& key) const;
    std::optional<ConfigList> optionalList(const std::string& key) const;

    /// @brief The tables directly under this scope, by name order: `config("ports.inputs")`
    ///        gives `[ports.inputs.a]` and `[ports.inputs.b]`, each a scope with its own
    ///        key(). A key that holds a value or a list is no table, and a table with no
    ///        key is not there. Reading nothing, it leaves every key unread.
    std::vector<ConfigView> tables() const;

    /// @brief The array @p key of scalars, in file order, each read as required<T> reads.
    /// @throws ParameterError when unset, when not an array, or when an element is not a T;
    ///         an element's message names `key[index]`.
    template <typename T>
        requires detail::ConfigValueType<T>
    std::vector<T> requiredArray(const std::string& key) const
    {
        std::optional<std::vector<T>> values = this->readArray<T>(key, std::nullopt);
        if (!values) {
            throw ParameterError("hypha: required parameter not set: " + this->name(key));
        }
        return std::move(*values);
    }

    /// @brief The array @p key of exactly N scalars. @throws ParameterError as above, and
    ///        for any other length: `expected 3 elements, found 4`.
    template <typename T, size_t N>
        requires detail::ConfigValueType<T>
    std::array<T, N> requiredArray(const std::string& key) const
    {
        const std::optional<std::vector<T>> values = this->readArray<T>(key, N);
        if (!values) {
            throw ParameterError("hypha: required parameter not set: " + this->name(key));
        }
        return detail::toArray<T, N>(*values);
    }

    /// @brief requiredArray<T>(), or no value when @p key is not set. A key that is set and
    ///        malformed throws.
    template <typename T>
        requires detail::ConfigValueType<T>
    std::optional<std::vector<T>> optionalArray(const std::string& key) const
    {
        return this->readArray<T>(key, std::nullopt);
    }

    /// @brief requiredArray<T, N>(), or no value when @p key is not set.
    template <typename T, size_t N>
        requires detail::ConfigValueType<T>
    std::optional<std::array<T, N>> optionalArray(const std::string& key) const
    {
        const std::optional<std::vector<T>> values = this->readArray<T>(key, N);
        if (!values) {
            return std::nullopt;
        }
        return detail::toArray<T, N>(*values);
    }

    /// @brief The key @p key as a list of T, given either as one value or as an array of
    ///        them (`topic = "/a"` or `topic = ["/a", "/b"]`), the ROS idiom of a parameter
    ///        that takes one or many. An array is read as requiredArray<T>().
    /// @throws ParameterError when unset, or for an element that is not a T.
    template <typename T>
        requires detail::ConfigValueType<T>
    std::vector<T> requiredScalarOrArray(const std::string& key) const
    {
        std::optional<std::vector<T>> values = this->optionalScalarOrArray<T>(key);
        if (!values) {
            throw ParameterError("hypha: required parameter not set: " + this->name(key));
        }
        return std::move(*values);
    }

    /// @brief requiredScalarOrArray<T>(), or no value when @p key is not set.
    template <typename T>
        requires detail::ConfigValueType<T>
    std::optional<std::vector<T>> optionalScalarOrArray(const std::string& key) const
    {
        const std::expected<size_t, Status> length = detail::paramListLength(this->name(key));
        const bool isScalar = !length && length.error() == Status::InvalidArgument;
        if (!isScalar) {
            return this->readArray<T>(key, std::nullopt);
        }
        const std::expected<T, Status> value = this->read<T>(key);
        if (!value) {
            this->refuse(key, value.error());
        }
        detail::recordRequiredParameter(this->name(key), *value);
        return std::vector<T>{*value};
    }

    /// @brief The array @p key as a matrix of @p rows x @p cols, row by row: the flat list
    ///        of a ROS matrix parameter. A 0 for @p rows or @p cols takes it from the length
    ///        (`requiredMatrix<double>("mixer", 0, 6)`: any number of rows of 6). See
    ///        config_matrix.h. @throws ParameterError as requiredArray(), and when the length
    ///        does not fit.
    template <typename T>
        requires detail::ConfigValueType<T>
    Matrix<T> requiredMatrix(const std::string& key, size_t rows, size_t cols) const;

    /// @brief The keys under this view that nothing read and nothing ignored, sorted.
    std::vector<std::string> unreadKeys() const
    {
        return backend::unreadParameterKeys(this->name());
    }

    /// @brief Marks the key or table @p keyOrPrefix as read by a branch the code did not
    ///        take: it leaves unreadKeys() and is an ignored manifest row with @p reason.
    ///        An absent key is fine. @throws ParameterError for an empty key or reason.
    void ignore(const std::string& keyOrPrefix, const std::string& reason) const
    {
        const std::string key = this->name(keyOrPrefix);
        if (backend::ignoreParameter(key, reason) != HYPHA_STATUS_OK) {
            throw ParameterError(
                "hypha: parameter '" + key + "' needs a key and a reason to ignore"
            );
        }
    }

  private:
    template <typename T>
    std::optional<std::vector<T>>
    readArray(const std::string& key, std::optional<size_t> expected) const
    {
        const std::string                   listName = this->name(key);
        const std::expected<size_t, Status> length = detail::paramListLength(listName);
        if (!length) {
            if (length.error() == Status::NotAvailable) {
                return std::nullopt;
            }
            throw ParameterError("hypha: parameter '" + listName + "' is not a list");
        }
        const bool wrongLength = expected.has_value() && *expected != *length;
        if (wrongLength) {
            throw ParameterError(
                "hypha: parameter '" + listName + "': expected " + std::to_string(*expected)
                + " elements, found " + std::to_string(*length)
            );
        }
        std::vector<T> values;
        values.reserve(*length);
        for (size_t index = 0; index < *length; ++index) {
            const std::expected<T, Status> element = detail::paramListAt<T>(listName, index);
            if (!element) {
                throw ParameterError(detail::arrayElementRefusal<T>(listName, index));
            }
            values.push_back(*element);
        }
        detail::recordRequiredParameter(listName, detail::arrayText(values));
        return values;
    }

    template <typename T>
    std::expected<T, Status> read(const std::string& key) const
    {
        if (this->index_) {
            return detail::paramListAt<T>(this->prefix_, *this->index_, key);
        }
        T                      value{};
        const HyphaParamResult result = detail::readParameterValue<T>(this->name(key), value);
        if (result != HYPHA_PARAM_RESULT_OK) {
            return std::unexpected(detail::listReadStatus(result));
        }
        return value;
    }

    [[noreturn]] void refuse(const std::string& key, Status status) const
    {
        if (status == Status::NotAvailable) {
            throw ParameterError("hypha: required parameter not set: " + this->name(key));
        }
        detail::reportMalformedParameter(this->name(key));
    }

    template <typename T, typename Constraint>
    void validate(const std::string& key, const T& value, const Constraint& constraint) const
    {
        if (!constraint.accepts(value)) {
            throw ParameterError(
                "hypha: parameter '" + this->name(key) + "' " + constraint.description
            );
        }
    }

    std::string           prefix_;
    std::optional<size_t> index_;
};

/// A standalone parameter configuration scope that does not join the bus.
class ConfigContext
{
  public:
    explicit ConfigContext(const std::string& parameterFile)
    {
        bool         created = false;
        std::string  refusal;
        const Status status = backend::initConfig(parameterFile, &created, &refusal);
        this->ownsConfig_ = created;
        if (status != Status::Ok) {
            const std::string context = "configuration '" + parameterFile + "' loading";
            throw std::runtime_error(detail::failureText(context, status, refusal));
        }
    }

    ~ConfigContext()
    {
        if (!this->ownsConfig_) {
            return;
        }
        backend::shutdownConfig();
    }

    ConfigContext(const ConfigContext&) = delete;
    ConfigContext& operator=(const ConfigContext&) = delete;
    ConfigContext(ConfigContext&&) = delete;
    ConfigContext& operator=(ConfigContext&&) = delete;

  private:
    bool ownsConfig_{false};
};

inline ConfigView config(const std::string& prefix = {})
{
    return ConfigView(prefix);
}

}  // namespace hypha

#include "hypha/config_list.h"
#include "hypha/config_matrix.h"
