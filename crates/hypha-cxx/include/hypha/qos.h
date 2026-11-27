#pragma once

/// @file
/// Qos: the service policy of a topic, and QosOf<M>: the policy of a message type.
///
/// The endpoint that opens a topic first creates its service with the policy; every later
/// typed endpoint must declare the same name and limits or it is refused with
/// Status::QosMismatch. Map message types in a header that every node includes:
///
///     namespace my::policies {
///     inline constexpr hypha::Qos camera{
///         .name = "camera", .publishers = 1, .subscribers = 4, .maxDepth = 2,
///         .maxPayloadDynamic = 8'000'000};
///     }
///     template <>
///     struct hypha::QosOf<my_msgs::Image>
///     {
///         static constexpr hypha::Qos value = my::policies::camera;
///     };
///
/// A type without a specialisation has the policy `hypha::defaultQos`. `/clock` and
/// `/clock_progress` have fixed limits: a policy declared on one of them is ignored.

#include "hypha/hypha_ffi.h"

#include <ros/message_traits.h>

#include <cstdint>
#include <string>
#include <string_view>

namespace tf2_msgs {
struct TFMessage;
}  // namespace tf2_msgs

namespace hypha_msgs {
struct NodeState;
struct NodeIntrospection;
}  // namespace hypha_msgs

namespace hypha {

/// @brief A service policy: its name and the limits of the topic's service.
///
/// Out-of-range fields are refused with InvalidArgument: `name` is 1 to 32 letters, digits,
/// `_`, `-` or `.`, starting with a letter or digit; `publishers` and `subscribers` are 1
/// to 256; `maxDepth` is 1 to 1024.
struct Qos
{
    /// The policy name. The text must have static storage: an endpoint keeps the view.
    std::string_view name;
    /// Publishers the topic admits.
    uint32_t publishers{0};
    /// Subscribers the topic admits.
    uint32_t subscribers{0};
    /// The deepest queue a subscriber of the topic may ask for.
    uint32_t maxDepth{0};
    /// The largest message of a publisher, in serialized bytes, for a type whose size
    /// varies. Zero is growable. Not part of the policy two endpoints must agree on.
    uint64_t maxPayloadDynamic{0};
    /// The first slot size, in serialized bytes, of a publisher whose type varies in size
    /// and that has no payload bound: a hint, not a bound. Zero is 4096.
    uint64_t initialPayload{0};
};

/// @brief The policy of every message type the application maps to no other; the
///        library's built-in `default` policy.
inline constexpr Qos defaultQos{
    .name = "default", .publishers = 8, .subscribers = 16, .maxDepth = 32
};

/// @brief The policy of `/tf` and `/tf_static`.
inline constexpr Qos tfQos{.name = "tf", .publishers = 32, .subscribers = 32, .maxDepth = 32};

/// @brief The policy of a node's own `~/state` and `~/introspection`.
inline constexpr Qos reportQos{
    .name = "report", .publishers = 2, .subscribers = 2, .maxDepth = 1, .initialPayload = 1024
};

/// @brief The policy of the message type @p M; the application specialises it.
template <typename M>
struct QosOf
{
    static constexpr Qos value = defaultQos;
};

/// @brief A view (hypha/view.h) has the policy of its message.
template <typename M>
struct QosOf<View<M>> : QosOf<M>
{
};

/// @brief A policy hypha maps itself.
template <>
struct QosOf<tf2_msgs::TFMessage>
{
    static constexpr Qos value = tfQos;
};

/// @brief A policy hypha maps itself.
template <>
struct QosOf<hypha_msgs::NodeState>
{
    static constexpr Qos value = reportQos;
};

/// @brief A policy hypha maps itself.
template <>
struct QosOf<hypha_msgs::NodeIntrospection>
{
    static constexpr Qos value = reportQos;
};

namespace detail {

/// @brief @p policy as the C ABI takes it, with the payload bound @p maxPayloadDynamic.
///        The result borrows the name bytes of @p policy.
inline ::HyphaQos declaredQos(const Qos& policy, uint64_t maxPayloadDynamic)
{
    ::HyphaQos raw{};
    raw.name = policy.name.data();
    raw.name_len = policy.name.size();
    raw.max_publishers = policy.publishers;
    raw.max_subscribers = policy.subscribers;
    raw.max_depth = policy.maxDepth;
    raw.max_payload_dynamic = maxPayloadDynamic;
    raw.initial_payload = policy.initialPayload;
    return raw;
}

/// @brief @p policy as text for a message: its name and limits.
inline std::string describeQos(const Qos& policy)
{
    return "'" + std::string(policy.name) + "' (publishers " + std::to_string(policy.publishers)
         + ", subscribers " + std::to_string(policy.subscribers) + ", maxDepth "
         + std::to_string(policy.maxDepth) + ")";
}

}  // namespace detail

}  // namespace hypha
