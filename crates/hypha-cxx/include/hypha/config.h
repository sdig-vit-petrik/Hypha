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

    /// @brief The list @p key. @throws ParameterError when not a list, or unset (list() only).
    ConfigList                list(const std::string& key) const;
    std::optional<ConfigList> optionalList(const std::string& key) const;

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

    std::vector<std::string> unreadKeys() const
    {
        return backend::unreadParameterKeys(this->name());
    }

  private:
    template <typename T>
    std::optional<std::vector<T>>
    readArray(const std::string& key, std::optional<size_t> expected) const
    {
        if (this->index_) {
            throw ParameterError("hypha: nested lists are not supported: " + this->name(key));
        }
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

/// Immutable list views retain TOML order and label failures with the entry index.
class ConfigList
{
  public:
    class Iterator
    {
      public:
        using value_type = ConfigView;
        using difference_type = std::ptrdiff_t;
        using iterator_category = std::input_iterator_tag;

        Iterator(std::string list, size_t index) : list_(std::move(list)), index_(index)
        {
        }

        ConfigView operator*() const
        {
            return ConfigView(this->list_, this->index_);
        }

        Iterator& operator++()
        {
            ++this->index_;
            return *this;
        }

        void operator++(int)
        {
            ++*this;
        }

        bool operator==(const Iterator& other) const
        {
            return this->index_ == other.index_;
        }

      private:
        std::string list_;
        size_t      index_;
    };

    explicit ConfigList(std::string name) : name_(std::move(name))
    {
        const std::expected<size_t, Status> length = detail::paramListLength(this->name_);
        if (!length) {
            if (length.error() == Status::NotAvailable) {
                throw ParameterError("hypha: required list not set: " + this->name_);
            }
            throw ParameterError("hypha: parameter '" + this->name_ + "' is not a list");
        }
        this->size_ = *length;
    }

    size_t size() const
    {
        return this->size_;
    }

    ConfigView at(size_t index) const
    {
        if (index >= this->size_) {
            const std::string entry = this->name_ + "[" + std::to_string(index) + "]";
            throw ParameterError("hypha: list entry not set: " + entry);
        }
        return ConfigView(this->name_, index);
    }

    Iterator begin() const
    {
        return Iterator(this->name_, 0);
    }

    Iterator end() const
    {
        return Iterator(this->name_, this->size_);
    }

  private:
    std::string name_;
    size_t      size_{0};
};

inline ConfigList ConfigView::list(const std::string& key) const
{
    if (this->index_) {
        throw ParameterError("hypha: nested lists are not supported: " + this->name(key));
    }
    return ConfigList(this->name(key));
}

inline std::optional<ConfigList> ConfigView::optionalList(const std::string& key) const
{
    const std::expected<size_t, Status> length = detail::paramListLength(this->name(key));
    const bool                          notSet = !length && length.error() == Status::NotAvailable;
    if (notSet) {
        return std::nullopt;
    }
    return this->list(key);
}

/// A standalone parameter configuration scope that does not join the bus.
class ConfigContext
{
  public:
    explicit ConfigContext(const std::string& parameterFile)
    {
        bool         created = false;
        const Status status = backend::initConfig(parameterFile, &created);
        this->ownsConfig_ = created;
        if (status != Status::Ok) {
            throw std::runtime_error(
                detail::failureText("configuration '" + parameterFile + "' loading", status)
            );
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
