#pragma once

/// @file
/// Named counters of this node and of the process: `node.*`, `schema.*`, `log.*` and
/// `integrity.*`, `loan.*` for a subscriber.

#include "hypha/backend/introspection.h"
#include "hypha/raw.h"

#include <cstdint>
#include <expected>
#include <optional>
#include <string_view>
#include <vector>

namespace hypha {

/// @brief Every metric of the session node. Before a Node exists, only the process-wide
///        `log.*` counters.
/// @return The metrics, or the status that refused the read.
[[nodiscard]] inline std::expected<std::vector<Metric>, Status> metrics()
{
    return backend::readMetrics(backend::nodeHandle());
}

/// @brief The value of one named metric of the session node.
/// @return The value, or std::nullopt when the read fails or the node does not emit `name`.
[[nodiscard]] inline std::optional<uint64_t> metric(std::string_view name)
{
    const std::expected<std::vector<Metric>, Status> list = metrics();
    if (!list) {
        return std::nullopt;
    }
    for (const Metric& entry : *list) {
        if (entry.name == name) {
            return entry.value;
        }
    }
    return std::nullopt;
}

}  // namespace hypha
