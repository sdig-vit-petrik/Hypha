#pragma once

/// @file
/// Transforms in a plain node: TransformBuffer looks transforms up, TransformBroadcaster
/// sends the dynamic ones and StaticTransformBroadcaster the static ones. Both
/// broadcasters refuse a transform that every buffer would refuse. Not part of
/// hypha/hypha.h.
///
///     hypha::TransformBuffer tf{node};
///     const std::expected<geometry_msgs::TransformStamped, hypha::TransformError> found =
///         tf.lookup("map", "lidar", *hypha::fromRosTime(scan.header.stamp));
///
///     hypha::TransformBroadcaster broadcaster{node};
///     if (!broadcaster.send(mapToBase)) { log(broadcaster.lastRefusal()); }

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

namespace detail {

/// @brief What a broadcaster's send() reports: the status of the last send and, when the
///        check refused it, why.
class SendReport
{
  public:
    /// @brief True when every transform of @p transforms passes the check every buffer
    ///        applies; else records the first refusal with InvalidArgument.
    bool admits(const std::vector<geometry_msgs::TransformStamped>& transforms, bool isStatic)
    {
        for (const geometry_msgs::TransformStamped& transform : transforms) {
            const std::optional<std::string> refusal =
                backend::transformRefusal(transform, isStatic);
            if (refusal.has_value()) {
                this->status_ = Status::InvalidArgument;
                this->refusal_ = "hypha: transform refused: " + *refusal;
                return false;
            }
        }
        this->refusal_.clear();
        return true;
    }

    /// @brief Record the outcome of a publish of an admitted message.
    bool published(bool sent, Status status)
    {
        this->status_ = status;
        return sent;
    }

    Status status() const
    {
        return this->status_;
    }

    const std::string& refusal() const
    {
        return this->refusal_;
    }

  private:
    Status      status_{Status::Ok};
    std::string refusal_;
};

}  // namespace detail

/// @brief Sends dynamic transforms on `/tf`, each send() one TFMessage.
///
/// A transform any TransformBuffer or Transforms port would refuse (an empty frame name,
/// a stamp that is no time, a value that is not finite, a rotation that is not a unit
/// quaternion) is refused here, and nothing of its message is sent. Create it before
/// node.run(); send() runs on one thread at a time.
class TransformBroadcaster
{
  public:
    /// @brief Advertise a TFMessage publisher on @p topic with the `tf` policy.
    /// @throws std::runtime_error when the endpoint is refused; ConfigError for a topic
    ///         that exists with another policy (QosMismatch).
    explicit TransformBroadcaster(Node& node, Topic topic = Topic{"/tf"})
        : publisher_(node.advertise<tf2_msgs::TFMessage>(std::move(topic)))
    {
    }

    /// @brief Check and publish @p transform.
    /// @return True if the message was sent. False for a refused transform
    ///         (lastPublishStatus() InvalidArgument, lastRefusal() names the frames and
    ///         the reason) and for a refused publish, as Publisher::publish.
    [[nodiscard]] bool send(const geometry_msgs::TransformStamped& transform)
    {
        return this->send(std::vector<geometry_msgs::TransformStamped>{transform});
    }

    /// @brief Check every transform of @p transforms and publish them as one message, or
    ///        nothing when one is refused. An empty vector sends nothing and returns true.
    [[nodiscard]] bool send(const std::vector<geometry_msgs::TransformStamped>& transforms)
    {
        if (transforms.empty()) {
            return true;
        }
        if (!this->report_.admits(transforms, false)) {
            return false;
        }
        tf2_msgs::TFMessage message{};
        message.transforms = transforms;
        const bool sent = this->publisher_.publish(message);
        return this->report_.published(sent, this->publisher_.lastPublishStatus());
    }

    /// @return The status of the most recent send: InvalidArgument after a refused
    ///         transform, else the publish status, as Publisher reports it.
    Status lastPublishStatus() const
    {
        return this->report_.status();
    }

    /// @return Why the most recent send was refused, naming the frames; empty otherwise.
    const std::string& lastRefusal() const
    {
        return this->report_.refusal();
    }

    /// @return The topic this broadcaster publishes on.
    std::string topic() const
    {
        return this->publisher_.topic();
    }

  private:
    Publisher<tf2_msgs::TFMessage> publisher_;
    detail::SendReport             report_;
};

/// @brief Sends static transforms on a latched `/tf_static`, keeping every one it sent.
///
/// Each send() publishes one message holding every transform sent so far, keyed by child
/// frame. A transform that TransformBroadcaster would refuse (the stamp is not checked)
/// is refused and not kept. Create it before node.run(); send() runs on one thread at a
/// time.
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
    /// @return True if the message was sent. False for a refused transform, which is not
    ///         kept (lastPublishStatus() InvalidArgument, lastRefusal() says why), and for
    ///         a refused publish.
    [[nodiscard]] bool send(const geometry_msgs::TransformStamped& transform)
    {
        return this->send(std::vector<geometry_msgs::TransformStamped>{transform});
    }

    /// @brief Keep every transform of @p transforms, in order, and publish the whole set
    ///        as one message; keep and send nothing when one is refused. An empty vector
    ///        sends nothing and returns true.
    [[nodiscard]] bool send(const std::vector<geometry_msgs::TransformStamped>& transforms)
    {
        if (transforms.empty()) {
            return true;
        }
        if (!this->report_.admits(transforms, true)) {
            return false;
        }
        for (const geometry_msgs::TransformStamped& transform : transforms) {
            this->keep(transform);
        }
        return this->publishSet();
    }

    /// @return The status of the most recent send: InvalidArgument after a refused
    ///         transform, else the publish status, as Publisher reports it.
    Status lastPublishStatus() const
    {
        return this->report_.status();
    }

    /// @return Why the most recent send was refused, naming the frames; empty otherwise.
    const std::string& lastRefusal() const
    {
        return this->report_.refusal();
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
        const bool sent = this->publisher_.publish(message);
        return this->report_.published(sent, this->publisher_.lastPublishStatus());
    }

    Publisher<tf2_msgs::TFMessage>               publisher_;
    std::vector<geometry_msgs::TransformStamped> kept_;
    detail::SendReport                           report_;
};

}  // namespace hypha
