#pragma once

/// @file
/// hypha::sharing(): a port that reads the topic of another port.

#include "hypha/source.h"

#include <memory>

namespace hypha {

template <typename M>
class Input;

/// @brief Names the topic that another port reads. See sharing().
template <typename M>
struct SharedWith
{
    std::shared_ptr<detail::SourceSlot<M>> slot;
};

/// @brief Make a port read the topic of @p port, as a second view of the same samples:
///
///     Latest<VehMode> mode{*this, "mode"};
///     Latest<VehMode> acs{*this, "acs", hypha::sharing(mode)};
///
/// Both read one subscriber; each view is supervised by itself. Construct both before
/// either cycle starts: the first start opens the subscriber.
template <typename M>
SharedWith<M> sharing(const Input<M>& port)
{
    return {port.slot_};
}

}  // namespace hypha
