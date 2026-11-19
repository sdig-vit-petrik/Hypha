#pragma once

/// @file
/// Transforms in a plain node: TransformBuffer looks transforms up, and
/// StaticTransformBroadcaster sends the static ones. Not part of hypha/hypha.h.
///
///     hypha::TransformBuffer tf{node};
///     const std::expected<geometry_msgs::TransformStamped, hypha::TransformError> found =
///         tf.lookup("map", "lidar", *hypha::fromRosTime(scan.header.stamp));
///
///     hypha::StaticTransformBroadcaster statics{node};
///     static_cast<void>(statics.send(baseToLidar));

#include "hypha/backend/handles.h"
#include "hypha/backend/session.h"
#include "hypha/backend/transform_buffer.h"
#include "hypha/node.h"
#include "hypha/publisher.h"
#include "hypha/qos.h"
#include "hypha/status.h"
#include "hypha/topic.h"
#include "hypha/transform_types.h"
#include "hypha_base/time.h"
#include <hypha_msgs/geometry_msgs/TransformStamped.h>
#include <hypha_msgs/tf2_msgs/TFMessage.h>

#include <cstdint>
#include <expected>
#include <optional>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace hypha {

/// @brief The transforms of `/tf` and `/tf_static` of a plain node, kept for `cache` per
///        frame (default 10 s), looked up by frame and time.
///
/// Threading: lookup() and lookupLatest() may be called from any thread while the object
/// lives. Construct and destroy it on the thread that configures the node, never while
/// node.run() runs on another thread. Nothing is ingested unless the node runs.
class TransformBuffer
{
  public:
    /// @brief Subscribe @p topics (default `/tf`, `/tf_static`) and keep @p cache of
    ///        dynamic transforms per frame. Create it before node.run().
    /// @throws ConfigError when a topic exists with another policy (QosMismatch).
    ///         std::runtime_error for any other refusal.
    explicit TransformBuffer(
        Node& /*node*/, TransformTopics topics = {}, Duration cache = defaultCache()
    )
    {
        const int64_t cacheNanos = cache.nanos();
        const bool    cacheNotPositive = cacheNanos <= 0;
        if (cacheNotPositive) {
            detail::throwFailure("transform buffer cache", Status::InvalidArgument);
        }
        HyphaNode* node = backend::nodeHandle();
        if (node == nullptr) {
            detail::throwFailure("transform buffer", Status::InvalidState);
        }
        const ::HyphaQos    qos = detail::declaredQos(QosOf<tf2_msgs::TFMessage>::value, 0);
        const ::HyphaStatus status = backend::createTransformBuffer(
            node,
            topics.tf,
            topics.tfStatic,
            qos,
            static_cast<uint64_t>(cacheNanos),
            this->buffer_.put()
        );
        require(Status(status), "transform buffer");
    }

    TransformBuffer(const TransformBuffer&) = delete;
    TransformBuffer& operator=(const TransformBuffer&) = delete;
    TransformBuffer(TransformBuffer&&) noexcept = default;
    TransformBuffer& operator=(TransformBuffer&&) noexcept = default;

    ~TransformBuffer() = default;

    /// @brief The transform that maps @p source coordinates into @p target at @p at.
    ///        Never waits.
    /// @return The transform, or why none was found.
    /// @throws std::runtime_error when the C ABI refuses the lookup; std::out_of_range
    ///         when the stamp does not fit a RosTime.
    [[nodiscard]] std::expected<geometry_msgs::TransformStamped, TransformError>
    lookup(std::string_view target, std::string_view source, Time at) const
    {
        return this->find(target, source, at.nanos());
    }

    /// @brief As lookup(), at the latest time common to the chain, which becomes the
    ///        stamp (0 for an all-static chain).
    [[nodiscard]] std::expected<geometry_msgs::TransformStamped, TransformError>
    lookupLatest(std::string_view target, std::string_view source) const
    {
        return this->find(target, source, std::nullopt);
    }

  private:
    static Duration defaultCache()
    {
        return Duration::fromNanos(static_cast<int64_t>(HYPHA_DEFAULT_TF_CACHE_NANOS));
    }

    std::expected<geometry_msgs::TransformStamped, TransformError>
    find(std::string_view target, std::string_view source, std::optional<int64_t> atNanos) const
    {
        uint32_t            result = 0;
        HyphaTransform      raw{};
        const ::HyphaStatus status = backend::transformBufferLookup(
            this->buffer_.get(), target, source, atNanos, &result, &raw
        );
        if (status != HYPHA_STATUS_OK) {
            throw std::runtime_error(
                "hypha: TransformBuffer::lookup() refused with status " + std::to_string(status)
            );
        }
        return detail::transformAnswer(result, raw, target, source, "TransformBuffer::lookup()");
    }

    backend::UniqueTransformBuffer buffer_;
};

/// @brief Sends static transforms on a latched `/tf_static`, keeping every one it sent.
///
/// Each send() publishes one message holding every transform sent so far, keyed by child
/// frame. Create it before node.run(); send() runs on one thread at a time.
class StaticTransformBroadcaster
{
  public:
    /// @brief Advertise a latched TFMessage publisher on @p topic.
    /// @throws std::runtime_error when the endpoint is refused; ConfigError for a topic
    ///         that exists with another policy (QosMismatch).
    explicit StaticTransformBroadcaster(Node& node, Topic topic = Topic{"/tf_static"})
        : publisher_(node.advertise<tf2_msgs::TFMessage>(std::move(topic), latched))
    {
    }

    StaticTransformBroadcaster(const StaticTransformBroadcaster&) = delete;
    StaticTransformBroadcaster& operator=(const StaticTransformBroadcaster&) = delete;
    StaticTransformBroadcaster(StaticTransformBroadcaster&&) noexcept = default;
    StaticTransformBroadcaster& operator=(StaticTransformBroadcaster&&) noexcept = default;

    /// @brief Keep @p transform, replacing the one of its child frame, and publish the
    ///        whole set.
    /// @return What Publisher::publish returns: true if the message was sent.
    [[nodiscard]] bool send(const geometry_msgs::TransformStamped& transform)
    {
        this->keep(transform);
        return this->publishSet();
    }

    /// @brief Keep every transform of @p transforms, in order, and publish the whole set
    ///        as one message. An empty vector sends nothing and returns true.
    /// @return What Publisher::publish returns: true if the message was sent.
    [[nodiscard]] bool send(const std::vector<geometry_msgs::TransformStamped>& transforms)
    {
        if (transforms.empty()) {
            return true;
        }
        for (const geometry_msgs::TransformStamped& transform : transforms) {
            this->keep(transform);
        }
        return this->publishSet();
    }

    /// @return The status of the most recent publish attempt, as Publisher does.
    Status lastPublishStatus() const
    {
        return this->publisher_.lastPublishStatus();
    }

    /// @return The transforms kept, in message order.
    const std::vector<geometry_msgs::TransformStamped>& transforms() const
    {
        return this->kept_;
    }

  private:
    static std::string_view keyOf(const geometry_msgs::TransformStamped& transform)
    {
        std::string_view key = transform.child_frame_id;
        if (!key.empty() && key.front() == '/') {
            key.remove_prefix(1);
        }
        return key;
    }

    void keep(const geometry_msgs::TransformStamped& transform)
    {
        const std::string_view key = keyOf(transform);
        for (geometry_msgs::TransformStamped& held : this->kept_) {
            const bool sameChild = keyOf(held) == key;
            if (sameChild) {
                held = transform;
                return;
            }
        }
        this->kept_.push_back(transform);
    }

    bool publishSet()
    {
        tf2_msgs::TFMessage message{};
        message.transforms = this->kept_;
        return this->publisher_.publish(message);
    }

    Publisher<tf2_msgs::TFMessage>               publisher_;
    std::vector<geometry_msgs::TransformStamped> kept_;
};

}  // namespace hypha
