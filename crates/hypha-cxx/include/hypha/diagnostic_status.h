#pragma once

/// @file
/// DiagnosticStatusWrapper: the `diagnostic_msgs/DiagnosticStatus` that a diagnostic task
/// fills, with the calls of ROS's `diagnostic_updater::DiagnosticStatusWrapper`
/// (`summary`, `mergeSummary`, `add`, and their printf-style `f` forms). The messages of
/// the framework stay the messages of ROS.

#include "hypha_base/time.h"

#include <diagnostic_msgs/DiagnosticStatus.h>

#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <locale>
#include <sstream>
#include <string>
#include <type_traits>
#include <vector>

namespace hypha {

namespace detail {

/// printf into a string. Output that the format cannot produce is an empty string.
[[gnu::format(printf, 1, 0)]] inline std::string formatText(const char* format, va_list arguments)
{
    va_list measure;
    va_copy(measure, arguments);
    const int length = std::vsnprintf(nullptr, 0, format, measure);
    va_end(measure);
    if (length <= 0) {
        return {};
    }
    std::vector<char> text(static_cast<size_t>(length) + 1U);
    std::vsnprintf(text.data(), text.size(), format, arguments);
    return std::string(text.data(), static_cast<size_t>(length));
}

}  // namespace detail

/// @brief A DiagnosticStatus with the calls a task uses to fill it. The levels are those of
///        the message: `OK`, `WARN`, `ERROR`, `STALE`, also reachable as `stat.OK`.
class DiagnosticStatusWrapper : public diagnostic_msgs::DiagnosticStatus
{
  public:
    /// @brief Set the level and the message.
    void summary(int8_t level, const std::string& message)
    {
        this->level = level;
        this->message = message;
    }

    /// @brief Take the level and the message of @p source.
    void summary(const diagnostic_msgs::DiagnosticStatus& source)
    {
        this->summary(source.level, source.message);
    }

    /// @brief summary() with a printf-style message.
    [[gnu::format(printf, 3, 4)]] void summaryf(int8_t level, const char* format, ...)
    {
        va_list arguments;
        va_start(arguments, format);
        this->summary(level, detail::formatText(format, arguments));
        va_end(arguments);
    }

    /// @brief Level OK and no message.
    void clearSummary()
    {
        this->summary(diagnostic_msgs::DiagnosticStatus::OK, std::string{});
    }

    /// @brief Combine a summary with the one held: the level is the worst of both. Two
    ///        messages of OK, or two of a fault, join with "; "; a fault replaces an OK message.
    void mergeSummary(int8_t otherLevel, const std::string& otherMessage)
    {
        const bool sameSide = (otherLevel > 0) == (this->level > 0);
        if (sameSide) {
            if (!this->message.empty()) {
                this->message += "; ";
            }
            this->message += otherMessage;
        } else if (otherLevel > this->level) {
            this->message = otherMessage;
        }
        if (otherLevel > this->level) {
            this->level = otherLevel;
        }
    }

    void mergeSummary(const diagnostic_msgs::DiagnosticStatus& other)
    {
        this->mergeSummary(other.level, other.message);
    }

    [[gnu::format(printf, 3, 4)]] void mergeSummaryf(int8_t otherLevel, const char* format, ...)
    {
        va_list arguments;
        va_start(arguments, format);
        this->mergeSummary(otherLevel, detail::formatText(format, arguments));
        va_end(arguments);
    }

    /// @brief Append a key and its value. A bool reads `True` or `False`, other values are
    ///        written with `operator<<` in the "C" locale.
    template <typename T>
    void add(const std::string& key, const T& value)
    {
        diagnostic_msgs::KeyValue entry;
        entry.key = key;
        if constexpr (std::is_same_v<T, bool>) {
            entry.value = value ? "True" : "False";
        } else if constexpr (std::is_convertible_v<const T&, std::string>) {
            entry.value = value;
        } else {
            std::ostringstream text;
            text.imbue(std::locale::classic());
            text << value;
            entry.value = text.str();
        }
        this->values.push_back(std::move(entry));
    }

    /// @brief add() with a printf-style value.
    [[gnu::format(printf, 3, 4)]] void addf(const std::string& key, const char* format, ...)
    {
        va_list arguments;
        va_start(arguments, format);
        this->add(key, detail::formatText(format, arguments));
        va_end(arguments);
    }

    /// @brief Remove the keys and values; the summary stays.
    void clear()
    {
        this->values.clear();
    }

    /// @brief The framework time of the update that runs the task: what the task measures
    ///        against, so that a replay or a test decides it. Zero outside an update.
    Time now() const
    {
        return this->updateTime_;
    }

    /// @brief Set by the updater before it runs a task.
    void setNow(Time time)
    {
        this->updateTime_ = time;
    }

  private:
    Time updateTime_;
};

}  // namespace hypha
