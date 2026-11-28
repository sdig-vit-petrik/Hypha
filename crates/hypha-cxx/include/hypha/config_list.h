#pragma once

/// @file
/// `ConfigList`, a list of tables or scalars of `hypha::config()`, and the walk over the
/// tables of a scope. config.h includes this header.

#include "hypha/config.h"

#include <cstddef>
#include <expected>
#include <iterator>
#include <optional>
#include <string>
#include <utility>
#include <vector>

namespace hypha {

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

        Iterator(std::string list, size_t index, const HyphaConfig* config = nullptr)
            : list_(std::move(list)), index_(index), config_(config)
        {
        }

        ConfigView operator*() const
        {
            return ConfigView(this->config_, this->list_, this->index_);
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
        std::string        list_;
        size_t             index_;
        const HyphaConfig* config_;
    };

    explicit ConfigList(std::string name, const HyphaConfig* config = nullptr)
        : name_(std::move(name)), config_(config)
    {
        const std::expected<size_t, Status> length = detail::paramListLength(this->name_, config);
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
        return ConfigView(this->config_, this->name_, index);
    }

    Iterator begin() const
    {
        return Iterator(this->name_, 0, this->config_);
    }

    Iterator end() const
    {
        return Iterator(this->name_, this->size_, this->config_);
    }

  private:
    std::string        name_;
    size_t             size_{0};
    const HyphaConfig* config_{nullptr};
};

inline ConfigList ConfigView::list(const std::string& key) const
{
    return ConfigList(this->name(key), this->config_);
}

inline std::optional<ConfigList> ConfigView::optionalList(const std::string& key) const
{
    const std::expected<size_t, Status> length =
        detail::paramListLength(this->name(key), this->config_);
    const bool notSet = !length && length.error() == Status::NotAvailable;
    if (notSet) {
        return std::nullopt;
    }
    return this->list(key);
}

inline std::vector<ConfigView> ConfigView::tables() const
{
    std::vector<ConfigView> found;
    for (const std::string& table : backend::tableNames(this->name(), this->config_)) {
        found.push_back(ConfigView(this->config_, this->name(table)));
    }
    return found;
}

}  // namespace hypha
