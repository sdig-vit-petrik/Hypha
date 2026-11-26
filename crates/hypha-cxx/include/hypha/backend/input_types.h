#pragma once

/// @file
/// The message type each topic is read as in this session. A second input of another
/// type is refused where it is created: the bus gives a topic one type, so that input
/// would never receive a frame of its own.

#include "hypha/backend/session.h"

#include <algorithm>
#include <cstdint>
#include <expected>
#include <map>
#include <memory>
#include <mutex>
#include <string>
#include <vector>

namespace hypha::backend {

namespace detail {

/// One live input of a topic: what it reads it as, and who asked.
struct InputClaim
{
    std::string               type;
    std::string               md5;
    std::string               reader;
    uint64_t                  generation{0};
    std::weak_ptr<const void> holder;
};

struct InputTypes
{
    std::mutex                                     mutex;
    std::map<std::string, std::vector<InputClaim>> byTopic;
};

inline InputTypes& inputTypes()
{
    static InputTypes state;
    return state;
}

}  // namespace detail

/// @brief Record that @p reader reads @p topic as @p type for as long as @p holder lives.
///
/// The same type again is allowed, from the same port or another. Wildcard patterns are
/// not recorded: they match topics, they do not name one.
/// @param topic   The resolved topic.
/// @param type    The message type name, such as `std_msgs/Bool`.
/// @param md5     The MD5 of the message definition; a changed definition is another type.
/// @param reader  What names the input in a refusal: `ports.inputs.<name>` or the topic.
/// @param holder  The input's own state; the claim ends when it is gone, or when the
///                session it was made in has left.
/// @return The refusal, one line without a prefix, when a live input of this session reads
///         @p topic as another type; else nothing.
inline std::expected<void, std::string> claimInputType(
    const std::string&                 topic,
    const std::string&                 type,
    const std::string&                 md5,
    const std::string&                 reader,
    const std::shared_ptr<const void>& holder
)
{
    if (topic.empty() || topic.find('*') != std::string::npos) {
        return {};
    }
    const uint64_t                    generation = sessionGeneration();
    detail::InputTypes&               state = detail::inputTypes();
    const std::lock_guard<std::mutex> lock(state.mutex);
    std::vector<detail::InputClaim>&  claims = state.byTopic[topic];
    std::erase_if(claims, [generation](const detail::InputClaim& claim) {
        return claim.generation != generation || claim.holder.expired();
    });
    if (!claims.empty() && (claims.front().type != type || claims.front().md5 != md5)) {
        const detail::InputClaim& first = claims.front();
        const std::string         second = type == first.type ? type + " (md5 " + md5 + ")" : type;
        const std::string         earlier =
            first.type == type ? first.type + " (md5 " + first.md5 + ")" : first.type;
        return std::unexpected(
            reader + " reads " + topic + " as " + second + ", but " + first.reader
            + " already reads it as " + earlier
            + "; a topic carries one message type: give the second input a topic of its own"
        );
    }
    const bool held = std::ranges::any_of(claims, [&holder](const detail::InputClaim& claim) {
        return claim.holder.lock() == holder;
    });
    if (!held) {
        claims.push_back(detail::InputClaim{type, md5, reader, generation, holder});
    }
    return {};
}

}  // namespace hypha::backend
