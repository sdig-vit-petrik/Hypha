#pragma once

/// @file
/// Member definitions of hypha::Input that keep cycle_input.h within its size; included
/// at the end of that header.

#include "hypha/cycle_input.h"

#include <expected>
#include <string>

namespace hypha {

/// The first port of a topic to bind opens its source.
template <typename M>
::HyphaStatus Input<M>::ensureSource()
{
    detail::SourceSlot<M>& slot = *this->slot_;
    if (slot.source.valid()) {
        // Another port opened the subscriber before this port was constructed.
        if (this->depth_ > slot.openedDepth) {
            detail::error(
                this->portText() + " needs a queue of " + std::to_string(this->depth_)
                + " samples, but the port it shares the topic with opened it with "
                + std::to_string(slot.openedDepth)
                + "; construct both ports before either cycle starts"
            );
            return HYPHA_STATUS_RESOURCE_LIMIT;
        }
        return HYPHA_STATUS_OK;
    }
    const std::expected<std::string, Status> resolved = slot.topic.resolve();
    if (!resolved) {
        // A bad `topic` in the port's table: a restart cannot fix it.
        this->tableRefused_ = true;
        return static_cast<::HyphaStatus>(resolved.error());
    }
    std::expected<detail::InputSource, Status> opened = detail::openSource<M>(
        backend::defaultWaitSet(),
        *resolved,
        backend::declaredSubscriberOptions(slot.policy, slot.depth)
    );
    if (!opened) {
        return static_cast<::HyphaStatus>(opened.error());
    }
    slot.source = std::move(*opened);
    slot.openedDepth = slot.depth;
    return HYPHA_STATUS_OK;
}

}  // namespace hypha
