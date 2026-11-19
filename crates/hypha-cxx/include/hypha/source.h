#pragma once

/// @file
/// The managed input source behind a cycle input port. Internal.

#include "hypha/backend/cycle.h"
#include "hypha/backend/endpoint_factory.h"
#include "hypha/backend/session.h"
#include "hypha/backend/subscriber_handle.h"
#include "hypha/backend/typed.h"
#include "hypha/hypha_ffi.h"
#include "hypha/qos.h"
#include "hypha/topic.h"

#include <cstdint>
#include <expected>
#include <string>
#include <utility>

namespace hypha::detail {

/// @brief An input source that drains a subscriber into a cycle, or a disabled one,
///        which has no subscriber and never receives a sample.
struct InputSource
{
    backend::UniqueInputSource handle;
    bool                       disabled{false};

    bool valid() const
    {
        return static_cast<bool>(this->handle);
    }

    /// @brief True for a source that reads a topic.
    bool enabled() const
    {
        return this->valid() && !this->disabled;
    }

    HyphaInputSource* native() const
    {
        return this->handle.get();
    }
};

/// @brief The topic of one or more input ports of messages @p M and the source that
///        reads it. The port that binds first opens the source.
template <typename M>
struct SourceSlot
{
    TopicSource topic;
    InputSource source;
    Qos policy{};
    /// The deepest queue a port of this topic asks for.
    uint32_t depth{0};
    /// The queue depth the source was opened with. Zero before it is open.
    uint32_t openedDepth{0};
};

/// @brief Open a source on @p topic for messages @p M with subscriber @p qos. An empty
///        @p topic gives a disabled source.
template <typename M>
std::expected<InputSource, Status>
openSource(HyphaWaitSet* waitset, const std::string& topic, const HyphaSubscriberOptions& qos)
{
    if (waitset == nullptr) {
        return std::unexpected(Status::InvalidState);
    }
    HyphaSubscriber* subscriber = nullptr;
    if (!topic.empty()) {
        const Status topicStatus = validateTopicName(topic);
        if (topicStatus != Status::Ok) {
            return std::unexpected(topicStatus);
        }
        backend::LoanedRawSubscriberHandle opened = backend::subscribeLoanedTypedWithQos(
            topic, codec::dataType<M>(), codec::md5Sum<M>(), codec::definition<M>(), qos
        );
        if (!opened.isValid()) {
            return std::unexpected(opened.status());
        }
        subscriber = opened.release();
    }
    std::expected<backend::UniqueInputSource, ::HyphaStatus> created =
        backend::createInputSource(waitset, subscriber);
    if (!created) {
        return std::unexpected(Status(created.error()));
    }
    if (!*created) {
        return std::unexpected(Status::InvalidState);
    }
    return InputSource{std::move(*created), topic.empty()};
}

}  // namespace hypha::detail
