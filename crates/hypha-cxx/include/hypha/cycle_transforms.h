#pragma once

/// @file
/// Transforms: a cycle's TF input.

#include "hypha/backend/cycle.h"
#include "hypha/backend/transform_buffer.h"
#include "hypha/backend/typed.h"
#include "hypha/cycle_port.h"
#include "hypha/cycle_runtime.h"
#include "hypha/message_stamp.h"
#include "hypha/qos.h"
#include "hypha/source.h"
#include "hypha/transform_types.h"

#include <geometry_msgs/TransformStamped.h>
#include <tf2_msgs/TFMessage.h>

#include <cstdint>
#include <expected>
#include <optional>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>

namespace hypha {

/// @brief A cycle's TF input: the transforms of `/tf` and `/tf_static` the step's
///        capture admits, kept for `cache` per frame (default 10 s).
///
/// Both topics carry the policy `hypha::tfQos`. A topic that a tool or a raw endpoint
/// created first carries the `default` policy, and the port is then refused with
/// QosMismatch.
class Transforms final : public Port
{
  public:
    explicit Transforms(
        Cycle&   owner,
        Duration cache = Duration::fromNanos(static_cast<int64_t>(HYPHA_DEFAULT_TF_CACHE_NANOS)),
        TransformTopics topics = {}
    )
        : Port(owner), cache_(cache), topics_(std::move(topics))
    {
    }

    /// @brief The transform that maps @p source coordinates into @p target at @p at.
    ///        Never waits. In a step only.
    /// @return The transform, or why none was found.
    /// @throws std::logic_error outside a step. std::runtime_error when the C ABI
    ///         refuses the lookup; std::out_of_range when the stamp does not fit a RosTime.
    std::expected<geometry_msgs::TransformStamped, TransformError>
    lookup(std::string_view target, std::string_view source, Time at) const
    {
        return this->find(target, source, at.nanos());
    }

    /// @brief As lookup(), at the latest time common to the chain, which becomes the
    ///        stamp (0 for an all-static chain).
    std::expected<geometry_msgs::TransformStamped, TransformError>
    lookupLatest(std::string_view target, std::string_view source) const
    {
        return this->find(target, source, std::nullopt);
    }

  private:
    friend class testing::StepHarness;

    ::HyphaStatus bind(HyphaCycle* cycle) override
    {
        const int64_t cacheNanos = this->cache_.nanos();
        if (cacheNanos <= 0) {
            return HYPHA_STATUS_INVALID_ARGUMENT;
        }
        HyphaSubscriberOptions qos =
            backend::declaredSubscriberOptions(QosOf<tf2_msgs::TFMessage>::value, 0);
        qos.no_publisher_warning_after_nanos = 0;
        using Opening = std::pair<const std::string*, detail::InputSource*>;
        const Opening openings[] = {
            Opening{&this->topics_.tf, &this->tf_},
            Opening{&this->topics_.tfStatic, &this->tfStatic_},
        };
        for (const Opening& opening : openings) {
            std::expected<detail::InputSource, Status> opened =
                detail::openSource<tf2_msgs::TFMessage>(
                    backend::defaultWaitSet(), *opening.first, qos
                );
            if (!opened) {
                return static_cast<::HyphaStatus>(opened.error());
            }
            *opening.second = std::move(*opened);
        }
        return backend::cycleBindTransforms(
            cycle,
            this->tf_.native(),
            this->tfStatic_.native(),
            static_cast<uint64_t>(cacheNanos),
            &this->index_
        );
    }

    ::HyphaStatus capture(const HyphaCycleImage* image) override
    {
        this->image_ = image;
        return HYPHA_STATUS_OK;
    }

    void endStep() override
    {
    }

    void leaveStep() override
    {
        this->image_ = nullptr;
    }

    std::string describeFailure() const override
    {
        return "Transforms (" + this->topics_.tf + ", " + this->topics_.tfStatic + ")";
    }

    std::expected<geometry_msgs::TransformStamped, TransformError>
    find(std::string_view target, std::string_view source, std::optional<int64_t> atNanos) const
    {
        uint32_t                    result = 0;
        HyphaTransform              raw{};
        ::HyphaStatus               status = HYPHA_STATUS_OK;
        detail::CaptureStore* const harness = this->harnessStore();
        if (harness != nullptr) {
            const HyphaTransformBuffer* fed = harness->transformsFor(*this, this->cache_.nanos());
            status = backend::transformBufferLookup(fed, target, source, atNanos, &result, &raw);
        } else if (this->image_ != nullptr) {
            status = backend::cycleImageLookupTransform(
                this->image_, this->index_, target, source, atNanos, &result, &raw
            );
        } else {
            throw std::logic_error("hypha: Transforms::lookup() outside a step");
        }
        if (status != HYPHA_STATUS_OK) {
            throw std::runtime_error(
                "hypha: Transforms::lookup() refused with status " + std::to_string(status)
            );
        }
        return detail::transformAnswer(result, raw, target, source, "Transforms::lookup()");
    }

    Duration               cache_;
    TransformTopics        topics_;
    detail::InputSource    tf_;
    detail::InputSource    tfStatic_;
    uint32_t               index_{0};
    const HyphaCycleImage* image_{nullptr};
};

}  // namespace hypha
