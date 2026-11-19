#pragma once

#include "hypha/backend/session.h"
#include "hypha/backend/text_readers.h"
#include "hypha/hypha_ffi.h"
#include "hypha/raw.h"

#include <cstddef>
#include <cstdint>
#include <expected>
#include <mutex>
#include <optional>
#include <string>
#include <utility>
#include <vector>

namespace hypha::backend {

inline std::optional<std::string>
lookupDefinition(const std::string& topic, const std::string& dataType, const std::string& md5)
{
    ffi::Session& state = ffi::session();
    HyphaNode*    node = nullptr;
    {
        const std::lock_guard<std::mutex> lock(state.mutex);
        node = state.node;
    }
    if (node == nullptr) {
        return std::nullopt;
    }

    const std::expected<std::string, Status> definition =
        readSchemaLookup(node, topic, dataType, md5);
    if (!definition) {
        return std::nullopt;
    }
    return *definition;
}

struct ListNodeCallback
{
    static void onNode(const HyphaNodeInfo* info, void* user_data)
    {
        const bool missing = info == nullptr || user_data == nullptr;
        if (missing) {
            return;
        }
        std::vector<hypha::NodeInfo>* list = static_cast<std::vector<hypha::NodeInfo>*>(user_data);
        hypha::NodeInfo               n;
        n.name = info->name != nullptr ? info->name : "";
        n.executable = info->executable != nullptr ? info->executable : "";
        n.pid = info->pid;
        n.alive = (info->alive != 0);
        list->push_back(std::move(n));
    }
};

inline std::vector<hypha::NodeInfo> listNodes()
{
    std::vector<hypha::NodeInfo> out;
    ::hypha_list_nodes(ListNodeCallback::onNode, &out);
    return out;
}

/// The message that @p call encodes for `node`, retried once with the length TooLarge
/// reports.
template <typename Call>
inline std::expected<std::vector<uint8_t>, Status> readEncoded(Call call, HyphaNode* node)
{
    constexpr size_t     FIRST_CAPACITY = 4096;
    std::vector<uint8_t> bytes(FIRST_CAPACITY);
    for (int attempt = 0; attempt < READ_ATTEMPTS; ++attempt) {
        size_t              length = 0;
        const ::HyphaStatus status = call(node, bytes.data(), bytes.size(), &length);
        if (status != HYPHA_STATUS_TOO_LARGE) {
            if (status != HYPHA_STATUS_OK) {
                return std::unexpected(Status(status));
            }
            bytes.resize(length);
            return bytes;
        }
        bytes.resize(length);
    }
    return std::unexpected(Status::TooLarge);
}

/// The encoded NodeIntrospection of `node`.
inline std::expected<std::vector<uint8_t>, Status> readIntrospection(HyphaNode* node)
{
    return readEncoded(&hypha_node_introspect, node);
}

/// The encoded NodeState of `node`, judged now.
inline std::expected<std::vector<uint8_t>, Status> readNodeState(HyphaNode* node)
{
    return readEncoded(&hypha_node_state, node);
}

struct MetricCallback
{
    static void onMetric(const char* name, size_t nameLength, uint64_t value, void* user_data)
    {
        const bool missing = name == nullptr || user_data == nullptr;
        if (missing) {
            return;
        }
        std::vector<hypha::Metric>* list = static_cast<std::vector<hypha::Metric>*>(user_data);
        list->push_back(hypha::Metric{std::string(name, nameLength), value});
    }
};

/// Every metric of `node`, or of the process alone when `node` is null, in emission order.
inline std::expected<std::vector<hypha::Metric>, Status> readMetrics(HyphaNode* node)
{
    std::vector<hypha::Metric> list;
    const Status               status = hypha_node_metrics(node, MetricCallback::onMetric, &list);
    if (status != HYPHA_STATUS_OK) {
        return std::unexpected(status);
    }
    return list;
}

}  // namespace hypha::backend
