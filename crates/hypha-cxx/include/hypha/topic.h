#pragma once

/// @file
/// How an endpoint names its topic: a bare string names a port table of the parameter
/// file, `hypha::Topic{"/tf"}` a literal.

#include "hypha/backend/config_readers.h"
#include "hypha/backend/session.h"
#include "hypha/raw.h"
#include "hypha/status.h"
#include "hypha_base/clock.h"

#include <cstdint>
#include <expected>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>

namespace hypha {

/// @brief Check that a topic is a canonical public name, such as `/node/example`.
/// @return Ok, or InvalidTopic for a name the transport would refuse.
inline Status validateTopicName(std::string_view topic)
{
    return backend::validatePublicTopicName(topic) == HYPHA_STATUS_OK ? Status::Ok
                                                                      : Status::InvalidTopic;
}

/// @brief A topic that code fixes, such as `hypha::Topic{"/tf"}` or `hypha::Topic{"~/state"}`.
///        `~/x` is `/<node>/x`, here and in a port table; `~` and `~x` are InvalidTopic.
struct Topic
{
    std::string name;
};

namespace detail {

/// @brief Where an endpoint takes its topic from: a literal, or a port table.
struct TopicSource
{
    std::string literal;
    std::string paramName;
    bool        paramOptional{false};
    bool        paramIsOutput{false};

    static TopicSource ofLiteral(Topic topic)
    {
        TopicSource source;
        source.literal = std::move(topic.name);
        return source;
    }

    static TopicSource ofPort(std::string name, bool isOutput, bool optional)
    {
        TopicSource source;
        source.paramName = std::move(name);
        source.paramIsOutput = isOutput;
        source.paramOptional = optional;
        return source;
    }

    bool fromParam() const
    {
        return !this->paramName.empty();
    }

    /// @brief What a failure names: `ports.outputs.<name>` or the literal topic.
    std::string describe() const
    {
        if (this->fromParam()) {
            return std::string(this->paramIsOutput ? "ports.outputs." : "ports.inputs.")
                 + this->paramName;
        }
        return "topic '" + this->literal + "'";
    }

    /// @throws ConfigError for a port table, std::runtime_error for a literal topic, when no
    ///         session exists: a handle that joins none (`hypha::Node()`) opens no endpoint
    ///         before a named Node has been created.
    void requireSession() const
    {
        if (backend::nodeHandle() == nullptr) {
            this->refuse(
                failureText(this->describe(), Status::InvalidState)
                + ": no session exists; create a named hypha::Node first"
            );
        }
    }

    /// @throws ConfigError for a port table: a restart cannot fix it. std::runtime_error
    ///         for a literal topic.
    [[noreturn]] void refuse(const std::string& text) const
    {
        if (this->fromParam()) {
            throw ConfigError(text);
        }
        throw std::runtime_error(text);
    }

    /// @return The topic; empty for a disabled port. InvalidArgument for an empty literal.
    std::expected<std::string, Status> resolve() const
    {
        if (this->fromParam()) {
            return backend::paramTopic(
                this->paramName,
                this->paramOptional ? HYPHA_PORT_TOPIC_OPTIONAL : HYPHA_PORT_TOPIC_REQUIRED,
                this->paramIsOutput
            );
        }
        if (this->literal.empty()) {
            return std::unexpected(Status::InvalidArgument);
        }
        return this->literal;
    }

    /// @return The topic; empty for a disabled port.
    /// @throws ConfigError when a port table is missing or refused: a restart cannot fix it.
    ///         std::runtime_error for a literal topic that is refused. Both when no session
    ///         exists (requireSession()).
    std::string resolveOrThrow() const
    {
        this->requireSession();
        const std::expected<std::string, Status> topic = this->resolve();
        if (topic) {
            return *topic;
        }
        this->refuse(failureText(this->describe(), topic.error()));
    }
};

}  // namespace detail

}  // namespace hypha
