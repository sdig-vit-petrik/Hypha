#pragma once

/// @file
/// The introspection snapshot of this node and the node state judged from its inputs.
///
///     const auto snapshot = hypha::introspect();
///     for (const hypha_msgs::IntrospectedEndpoint& row : snapshot->subscribers) { ... }

#include "hypha/backend/config_readers.h"
#include "hypha/backend/introspection.h"
#include "hypha/backend/ros1_codec.h"
#include "hypha/backend/session.h"
#include "hypha/status.h"
#include <hypha_msgs/NodeIntrospection.h>
#include <hypha_msgs/NodeState.h>

#include <cstdint>
#include <expected>
#include <format>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

namespace hypha {

namespace detail {

/// @brief Read one encoded record of this node with @p read and decode it as @p M.
/// @throws std::runtime_error naming @p call before a Node exists.
template <typename M, typename Reader>
std::expected<M, Status> readNodeRecord(Reader read, const char* call)
{
    HyphaNode* node = backend::nodeHandle();
    if (node == nullptr) {
        throw std::runtime_error(std::string("hypha: ") + call + " before a Node exists");
    }
    const std::expected<std::vector<uint8_t>, Status> bytes = read(node);
    if (!bytes) {
        return std::unexpected(bytes.error());
    }
    M record{};
    if (!codec::decode(*bytes, record)) {
        return std::unexpected(Status::CorruptPayload);
    }
    return record;
}

}  // namespace detail

/// @brief A snapshot of this node, taken now.
/// @return The snapshot, or the status that refused it.
/// @throws std::runtime_error before a Node exists.
inline std::expected<hypha_msgs::NodeIntrospection, Status> introspect()
{
    return detail::readNodeRecord<hypha_msgs::NodeIntrospection>(
        backend::readIntrospection, "introspect()"
    );
}

/// @brief Record a value the application sets itself as a configuration row of the node
///        manifest. Without a configuration it records nothing. Do not record a secret.
template <typename T>
void recordConfig(const std::string& key, const T& value)
{
    const bool nowhereToRecord =
        backend::nodeHandle() == nullptr && backend::configHandle() == nullptr;
    if (nowhereToRecord) {
        return;
    }
    backend::recordParameterRow(
        key, std::format("{}", value), std::nullopt, HYPHA_CONFIG_SOURCE_APPLICATION
    );
}

/// @brief The state of this node, judged now from the inputs' ok bits and the checks of the
///        application (`DiagnosticUpdater`): `WAITING` while a required input never arrived,
///        else `DEGRADED` while one is not ok (stale, stagnant) or a check is at ERROR or
///        STALE, else `INPUTS_USABLE`. `checks` lists the checks as last reported. Call it on
///        the thread that runs the node.
/// @throws std::runtime_error before a Node exists.
inline std::expected<hypha_msgs::NodeState, Status> nodeState()
{
    return detail::readNodeRecord<hypha_msgs::NodeState>(backend::readNodeState, "nodeState()");
}

/// One check of the application in `~/state` (#121): a name, a level of
/// `diagnostic_msgs/DiagnosticStatus` (OK, WARN, ERROR, STALE) and a message. An ERROR or
/// STALE check makes the node DEGRADED.
using HealthCheck = backend::CheckText;

/// @brief Replace the checks that @p owner reported to this process's node with @p checks;
///        none removes them. A driver reports its own; `DiagnosticUpdater` reports its tasks.
///        Without a node nothing is reported.
/// @return InvalidArgument for a name of 0 or over 128 bytes or a level over 3, TooLarge for
///         over 64 checks in all; a refusal changes nothing.
inline Status reportChecks(uint64_t owner, const std::vector<HealthCheck>& checks)
{
    HyphaNode* const node = backend::nodeHandle();
    if (node == nullptr) {
        return Status(Status::Ok);
    }
    return backend::setChecks(node, owner, checks);
}

}  // namespace hypha
