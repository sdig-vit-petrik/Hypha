#pragma once

/// @file
/// The C ABI of the transform buffer of a plain node, as the public headers call it.

#include "hypha/hypha_ffi.h"

#include <cstddef>
#include <cstdint>
#include <optional>
#include <string>
#include <string_view>

namespace hypha::backend {

/// @brief Create a transform buffer of @p node.
inline ::HyphaStatus createTransformBuffer(
    HyphaNode*             node,
    const std::string&     tfTopic,
    const std::string&     tfStaticTopic,
    const ::HyphaQos&      qos,
    uint64_t               cacheNanos,
    HyphaTransformBuffer** out
)
{
    return hypha_transform_buffer_create(
        node,
        tfTopic.data(),
        tfTopic.size(),
        tfStaticTopic.data(),
        tfStaticTopic.size(),
        &qos,
        cacheNanos,
        out
    );
}

/// @brief Look up @p source in @p target at @p atNanos, or at the latest common time
///        when it is empty.
inline ::HyphaStatus transformBufferLookup(
    const HyphaTransformBuffer* buffer,
    std::string_view            target,
    std::string_view            source,
    std::optional<int64_t>      atNanos,
    uint32_t*                   outResult,
    HyphaTransform*             out
)
{
    const char*    targetData = target.data() == nullptr ? "" : target.data();
    const char*    sourceData = source.data() == nullptr ? "" : source.data();
    const uint32_t latest = atNanos.has_value() ? 0U : 1U;
    return hypha_transform_buffer_lookup(
        buffer,
        targetData,
        target.size(),
        sourceData,
        source.size(),
        atNanos.value_or(0),
        latest,
        outResult,
        out
    );
}

}  // namespace hypha::backend
