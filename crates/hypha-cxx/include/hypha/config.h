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
class ConfigFile;

/// A scalar configuration scope, or one indexed table entry, of the configuration of the
/// process or of a ConfigFile.
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
        this->recordRequired(key, *value);
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
        this->recordRequired(key, *value);
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
            if (this->config_ == nullptr) {
                detail::recordDefaultParameter(this->name(key), defaultValue);
            }
            return defaultValue;
        }
        this->validate(key, *value, constraint);
        if (this->config_ == nullptr) {
            detail::recordParameter(this->name(key), *value, defaultValue);
        }
        return *value;
    }

    /// @brief The required file name @p key, resolved against the directory of the file
    ///        that wrote it; an absolute name stays. @throws ParameterError when unset,
    ///        empty or not text.
    std::string path(const std::string& key) const
    {
        const std::string name = this->required<std::string>(key, constraints::nonempty());
        const std::expected<std::string, Status> resolved =
            backend::resolveConfigKey(this->name(key), this->config_);
        if (!resolved) {
            throw ParameterError(
                "hypha: parameter '" + this->name(key) + "' is not a usable file name"
            );
        }
        return *resolved;
    }

    /// @brief path(), or no value when @p key is not set. A key that is set and empty, or
    ///        not text, throws as path() does.
    std::optional<std::string> optionalPath(const std::string& key) const
    {
        const std::expected<std::string, Status> value = this->read<std::string>(key);
        const bool notSet = !value && value.error() == Status::NotAvailable;
        if (notSet) {
            return std::nullopt;
        }
        return this->path(key);
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

    /// @brief The keys under this view that nothing read and nothing ignored, sorted.
    std::vector<std::string> unreadKeys() const
    {
        return backend::unreadParameterKeys(this->name(), this->config_);
    }

    /// @brief Marks the key or table @p keyOrPrefix as read by a branch the code did not
    ///        take: it leaves unreadKeys() and is an ignored manifest row with @p reason.
    ///        An absent key is fine. @throws ParameterError for an empty key or reason.
    void ignore(const std::string& keyOrPrefix, const std::string& reason) const
    {
        const std::string key = this->name(keyOrPrefix);
        if (backend::ignoreParameter(key, reason, this->config_) != HYPHA_STATUS_OK) {
            throw ParameterError(
                "hypha: parameter '" + key + "' needs a key and a reason to ignore"
            );
        }
    }

  private:
    friend class ConfigFile;
    friend class ConfigList;

    ConfigView(const HyphaConfig* config, std::string prefix)
        : prefix_(std::move(prefix)), config_(config)
    {
    }

    ConfigView(const HyphaConfig* config, std::string list, size_t index)
        : prefix_(std::move(list)), index_(index), config_(config)
    {
    }

    /// A read of a ConfigFile is no parameter of the node: it is not recorded.
    template <typename T>
    void recordRequired(const std::string& key, const T& value) const
    {
        if (this->config_ == nullptr) {
            detail::recordRequiredParameter(this->name(key), value);
        }
    }

    template <typename T>
    std::optional<std::vector<T>>
    readArray(const std::string& key, std::optional<size_t> expected) const
    {
        if (this->index_) {
            throw ParameterError("hypha: nested lists are not supported: " + this->name(key));
        }
        const std::string                   listName = this->name(key);
        const std::expected<size_t, Status> length =
            detail::paramListLength(listName, this->config_);
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
            const std::expected<T, Status> element =
                detail::paramListAt<T>(listName, index, {}, this->config_);
            if (!element) {
                throw ParameterError(
                    detail::arrayElementRefusal<T>(listName, index, this->config_)
                );
            }
            values.push_back(*element);
        }
        this->recordRequired(key, detail::arrayText(values));
        return values;
    }

    template <typename T>
    std::expected<T, Status> read(const std::string& key) const
    {
        if (this->index_) {
            return detail::paramListAt<T>(this->prefix_, *this->index_, key, this->config_);
        }
        T                      value{};
        const HyphaParamResult result =
            detail::readParameterValue<T>(this->name(key), value, this->config_);
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
    /// The ConfigFile read, or null for the configuration of the process.
    const HyphaConfig* config_{nullptr};
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

/// A TOML file read through ConfigView apart from the configuration of the process, such
/// as a camera calibration: the parameter-file loader reads it (`include`, `${NAME}`), its
/// reads mark its keys read for unreadKeys(), and they are not recorded as parameters of
/// the node. path() resolves against the directory of the file. The views borrow the file:
/// they must not outlive it.
class ConfigFile
{
  public:
    /// @throws ConfigError naming @p path when the file is missing, unreadable or not a
    ///         parameter file the loader accepts (it logs the reason, with the line).
    explicit ConfigFile(const std::string& path)
    {
        const Status status = backend::loadConfigFile(path, &this->config_);
        if (status != Status::Ok) {
            throw ConfigError(
                "hypha: '" + path + "' cannot be read as a TOML file (the reason is logged above)"
            );
        }
    }

    ~ConfigFile()
    {
        backend::destroyConfigFile(this->config_);
    }

    ConfigFile(const ConfigFile&) = delete;
    ConfigFile& operator=(const ConfigFile&) = delete;
    ConfigFile(ConfigFile&&) = delete;
    ConfigFile& operator=(ConfigFile&&) = delete;

    /// @brief The table @p prefix of the file, or the whole file.
    ConfigView view(const std::string& prefix = {}) const
    {
        return ConfigView(this->config_, prefix);
    }

  private:
    HyphaConfig* config_{nullptr};
};

inline ConfigView config(const std::string& prefix = {})
{
    return ConfigView(prefix);
}

}  // namespace hypha

#include "hypha/config_list.h"
