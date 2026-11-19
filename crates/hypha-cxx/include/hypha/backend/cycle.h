#pragma once

/// @file
/// The cycle calls of the C ABI. Each function takes the arguments of its C function.

#include "hypha/backend/handles.h"
#include "hypha/hypha_ffi.h"

#include <cstddef>
#include <cstdint>
#include <expected>
#include <optional>
#include <string>
#include <string_view>
#include <utility>

namespace hypha::backend {

/// @brief Create a cycle on @p waitset, which owns it. The C ABI releases @p context with
///        @p release when the cycle is destroyed; a refused create leaves it to the caller.
inline std::expected<HyphaCycle*, ::HyphaStatus> createCycle(
    HyphaWaitSet*       waitset,
    std::string_view    name,
    uint64_t            periodNanos,
    HyphaCycleStep      step,
    void*               context,
    HyphaContextRelease release
)
{
    HyphaCycle*         created = nullptr;
    const ::HyphaStatus status = hypha_cycle_create(
        waitset, name.data(), name.size(), periodNanos, step, context, release, &created
    );
    if (status != HYPHA_STATUS_OK) {
        return std::unexpected(status);
    }
    return created;
}

/// @brief Destroy a cycle before its node. Refused with INVALID_STATE while the
///        WaitSet runs; the cycle then lives until the node destroys it.
inline ::HyphaStatus cycleDestroy(HyphaCycle* cycle)
{
    return hypha_cycle_destroy(cycle);
}

/// @brief A managed source over @p subscriber, which the source takes; a refused
///        create destroys it. A null @p subscriber makes a disabled source.
inline std::expected<UniqueInputSource, ::HyphaStatus>
createInputSource(HyphaWaitSet* waitset, HyphaSubscriber* subscriber)
{
    HyphaInputSource*   created = nullptr;
    const ::HyphaStatus status = hypha_input_source_create(waitset, subscriber, &created);
    if (status != HYPHA_STATUS_OK) {
        if (subscriber != nullptr) {
            hypha_subscriber_destroy(subscriber);
        }
        return std::unexpected(status);
    }
    return UniqueInputSource(created);
}

inline ::HyphaStatus cycleCommit(HyphaCycle* cycle)
{
    return hypha_cycle_commit(cycle);
}

inline ::HyphaStatus cycleSetClockDriver(HyphaCycle* cycle, const HyphaClockDriverOptions* options)
{
    return hypha_cycle_set_clock_driver(cycle, options);
}

inline ::HyphaStatus cycleExpectParticipant(HyphaCycle* cycle, std::string_view name)
{
    return hypha_cycle_expect_participant(cycle, name.data(), name.size());
}

inline ::HyphaStatus cycleSetTrigger(HyphaCycle* cycle, uint32_t bindingIndex)
{
    return hypha_cycle_set_trigger(cycle, bindingIndex);
}

inline ::HyphaStatus cycleSetHostClock(HyphaCycle* cycle, uint32_t bindingIndex)
{
    return hypha_cycle_set_host_clock(cycle, bindingIndex);
}

inline ::HyphaStatus cycleSetClassBased(HyphaCycle* cycle, uint8_t classBased)
{
    return hypha_cycle_set_class_based(cycle, classBased);
}

inline ::HyphaStatus cycleBindInput(
    HyphaCycle*                     cycle,
    HyphaInputSource*               source,
    const HyphaCycleBindingOptions* options,
    uint32_t*                       outIndex
)
{
    return hypha_cycle_bind_input(cycle, source, options, outIndex);
}

/// @brief Bind a TF input over two managed sources, which stay owned by the caller.
inline ::HyphaStatus cycleBindTransforms(
    HyphaCycle*       cycle,
    HyphaInputSource* tf,
    HyphaInputSource* tfStatic,
    uint64_t          cacheNanos,
    uint32_t*         outIndex
)
{
    return hypha_cycle_bind_transforms(cycle, tf, tfStatic, cacheNanos, outIndex);
}

/// @brief Look up @p source in @p target in a step, at @p atNanos, or at the latest
///        common time when it is empty.
inline ::HyphaStatus cycleImageLookupTransform(
    const HyphaCycleImage* image,
    uint32_t               index,
    std::string_view       target,
    std::string_view       source,
    std::optional<int64_t> atNanos,
    uint32_t*              outResult,
    HyphaTransform*        out
)
{
    const char*    targetData = target.data() == nullptr ? "" : target.data();
    const char*    sourceData = source.data() == nullptr ? "" : source.data();
    const uint32_t latest = atNanos.has_value() ? 0U : 1U;
    return hypha_cycle_image_lookup_transform(
        image,
        index,
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

/// @brief Bind a publisher as an output of @p cycle. The endpoint is taken in every
///        case: the cycle owns it on success, and a refused bind destroys it.
inline ::HyphaStatus cycleBindOutput(
    HyphaCycle*     cycle,
    HyphaPublisher* endpoint,
    uint32_t        policy,
    uint32_t        maxMessagesPerStep,
    uint32_t*       outIndex
)
{
    const ::HyphaStatus bound =
        hypha_cycle_bind_output(cycle, endpoint, policy, maxMessagesPerStep, outIndex);
    if (bound != HYPHA_STATUS_OK) {
        hypha_publisher_destroy(endpoint);
    }
    return bound;
}

/// @brief Reserve @p len bytes of the step's message for @p outputIndex. On success
///        @p out is the first byte to write, or nullptr when the message was dropped.
///        Commit after writing.
inline ::HyphaStatus
cycleEmissionReserve(HyphaCycleEmission* emission, uint32_t outputIndex, size_t len, uint8_t** out)
{
    return hypha_cycle_emission_reserve(emission, outputIndex, len, out);
}

/// @brief Seal the open reservation.
inline ::HyphaStatus cycleEmissionCommit(HyphaCycleEmission* emission)
{
    return hypha_cycle_emission_commit(emission);
}

inline ::HyphaStatus cycleImageSample(
    const HyphaCycleImage* image,
    uint32_t               bindingIndex,
    size_t                 sampleIndex,
    HyphaCycleSampleView*  out
)
{
    return hypha_cycle_image_sample(image, bindingIndex, sampleIndex, out);
}

inline ::HyphaStatus
cycleImageHeld(const HyphaCycleImage* image, uint32_t bindingIndex, HyphaCycleSampleView* view)
{
    return hypha_cycle_image_held(image, bindingIndex, view);
}

inline ::HyphaStatus
cycleImageAlive(const HyphaCycleImage* image, uint32_t bindingIndex, uint64_t value)
{
    return hypha_cycle_image_alive(image, bindingIndex, value);
}

/// @brief Everything binding @p bindingIndex reports at this step's capture. @p image is
///        valid only inside the step.
inline ::HyphaStatus
cycleImageBinding(const HyphaCycleImage* image, uint32_t bindingIndex, HyphaCycleBindingView* out)
{
    return hypha_cycle_image_binding(image, bindingIndex, out);
}

/// @brief Report that sample @p sequence of binding @p bindingIndex did not decode; @p out
///        receives the binding judged without it.
inline ::HyphaStatus cycleImageRejectSample(
    const HyphaCycleImage* image,
    uint32_t               bindingIndex,
    uint64_t               sequence,
    HyphaCycleBindingView* out
)
{
    return hypha_cycle_image_reject_sample(image, bindingIndex, sequence, out);
}

inline ::HyphaStatus cycleHealth(const HyphaCycle* cycle, HyphaCycleHealth* out)
{
    return hypha_cycle_health(cycle, out);
}

/// @brief The binding view of the last finished step, readable from any thread.
inline ::HyphaStatus
cycleBinding(const HyphaCycle* cycle, uint32_t bindingIndex, HyphaCycleBindingView* out)
{
    return hypha_cycle_binding(cycle, bindingIndex, out);
}

inline ::HyphaStatus
cycleImageSetIgnored(const HyphaCycleImage* image, uint32_t bindingIndex, uint8_t ignore)
{
    return hypha_cycle_image_set_ignored(image, bindingIndex, ignore);
}

/// @brief The arguments of one input's supervision binding, as the C ABI takes them.
struct InputSupervisionRequest
{
    HyphaCycle*            cycle{nullptr};
    uint32_t               index{0};
    const HyphaNode*       node{nullptr};
    std::string_view       name;
    uint8_t                unsupervised{0};
    uint8_t                checkStagnation{0};
    uint8_t                takesTopic{0};
    HyphaSupervisionRanges ranges{};
};

/// @brief The outcome of an input's supervision binding; `detail` is set also on refusal.
struct InputSupervisionResult
{
    ::HyphaStatus status{HYPHA_STATUS_OK};
    bool          attached{false};
    /// Zero without a silence limit.
    int64_t     silenceNanos{0};
    std::string detail;
};

inline ::HyphaStatus bindInputSupervisionOnce(
    const InputSupervisionRequest& request,
    HyphaInputSupervisionResult*   out,
    char*                          detail,
    size_t                         capacity,
    size_t*                        detailLength
)
{
    HyphaInputSupervisionRequest raw{};
    raw.node = request.node;
    raw.name = request.name.data();
    raw.name_len = request.name.size();
    raw.unsupervised = request.unsupervised;
    raw.check_stagnation = request.checkStagnation;
    raw.takes_topic = request.takesTopic;
    raw.ranges = request.ranges;
    return hypha_cycle_bind_input_supervision(
        request.cycle, request.index, &raw, out, detail, capacity, detailLength
    );
}

/// @brief Check, read and attach the limits of one input.
inline InputSupervisionResult bindInputSupervision(const InputSupervisionRequest& request)
{
    HyphaInputSupervisionResult attached{};
    std::string                 detail(256, '\0');
    size_t                      detailLength = 0;
    ::HyphaStatus               status =
        bindInputSupervisionOnce(request, &attached, detail.data(), detail.size(), &detailLength);
    const bool needsMoreRoom = status == HYPHA_STATUS_TOO_LARGE;
    if (needsMoreRoom) {
        detail.resize(detailLength);
        status = bindInputSupervisionOnce(
            request, &attached, detail.data(), detail.size(), &detailLength
        );
    }
    detail.resize(detailLength);
    InputSupervisionResult result;
    result.status = status;
    result.attached = attached.attached != 0;
    result.silenceNanos = attached.limits.silence_nanos;
    result.detail = std::move(detail);
    return result;
}

}  // namespace hypha::backend
