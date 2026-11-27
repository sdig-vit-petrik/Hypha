#pragma once

/// @file
/// A received large message read in place: View<M> holds M's small fields, decoded, and a
/// std::span of its array of bytes where the message was received, in the shared-memory
/// slot; ViewPtr<M> owns one, as `M::ConstPtr` owns a message.

#include "hypha/detail/view_fields.h"
#include "hypha/hypha_ffi.h"

#include <ros/message_traits.h>
#include <ros/serialization.h>

#include <array>
#include <cstddef>
#include <cstdint>
#include <memory>
#include <span>
#include <stdexcept>
#include <type_traits>
#include <utility>
#include <vector>

namespace hypha {

/// @brief A message with a View: its loaned field (hypha/loan.h) is an array of bytes, as
///        in `sensor_msgs/Image`, `CompressedImage`, `PointCloud2`, `nav_msgs/OccupancyGrid`
///        and the byte MultiArrays. The generator emits `hypha::View<M>` for them.
template <typename M>
concept ViewableMessage = requires { typename View<M>::Message; };

namespace detail {

/// Where the bytes of one View live, and what keeps them: a message, a copy, or the
/// received sample, which hypha_received_sample_release returns to its publisher.
template <typename M>
struct ViewState
{
    View<M>                     view;
    std::shared_ptr<const void> owner;
    HyphaReceivedSample*        kept{nullptr};
    /// True while the bytes are a slot the library holds only until the callback or the
    /// step returns: then the library keeps them (kept) or copies them (owner).
    bool                        lent{false};

    ViewState() = default;
    ViewState(const ViewState&) = delete;
    ViewState& operator=(const ViewState&) = delete;

    ~ViewState()
    {
        hypha_received_sample_release(this->kept);
    }

    /// Copy the bytes the span reads into memory of this state, and read those.
    void copyBytes()
    {
        visitSpan(this->view, [this](auto& span) {
            using Element = typename IsSpan<std::remove_cvref_t<decltype(span)>>::Element;
            auto copy = std::make_shared<std::vector<Element>>(span.begin(), span.end());
            span = std::span<const Element>(copy->data(), copy->size());
            this->owner = std::move(copy);
        });
        this->lent = false;
    }
};

template <typename M>
class ViewAccess;

}  // namespace detail

/// @brief Shared ownership of a View<M>, as `M::ConstPtr` of a message: `msg->width`,
///        `msg->data[i]`, `msg->data.size()`, `*msg` for sensor_msgs::PointCloud2ConstIterator.
///        Copies share one view. A subscriber callback or a cycle step that receives one
///        reads the message where it arrived, after one CRC check, with no copy.
///
/// Lifetime: the bytes stay valid while any copy lives. A copy still held when the callback
/// or the step returns keeps them past it: a subscriber's callbacks keep up to the
/// subscriber's loan budget of received slots (SubscriberMetrics::loanBudget, one), counted
/// in loanHeld; past the budget, and always past a cycle step, the bytes are copied once at
/// that return, logged and counted (loanRefused for a subscriber). So a held view is never
/// lost and never dangles; only a std::span or pointer taken out of it lives no longer than
/// the ViewPtr it came from. Do not read a ViewPtr on another thread before its callback or
/// step returns: that return may move its bytes.
template <ViewableMessage M>
class ViewPtr
{
  public:
    using element_type = const View<M>;

    ViewPtr() = default;

    ViewPtr(std::nullptr_t)
    {
    }

    /// @brief A view of @p message, whose array the view reads and keeps; no copy. Lets an
    ///        upstream unit test pass a `M::ConstPtr` where the ported code takes a view.
    ViewPtr(std::shared_ptr<const M> message)
    {
        if (!message) {
            return;
        }
        auto              state = std::make_shared<detail::ViewState<M>>();
        detail::FieldWalk fields;
        ros::serialization::Serializer<M>::template allInOne<detail::FieldWalk, const M&>(
            fields, *message
        );
        detail::ViewFromMessage assign(fields);
        ros::serialization::Serializer<M>::template allInOne<detail::ViewFromMessage, View<M>&>(
            assign, state->view
        );
        state->owner = std::move(message);
        this->state_ = std::move(state);
    }

    ViewPtr(std::shared_ptr<M> message) : ViewPtr(std::shared_ptr<const M>(std::move(message)))
    {
    }

    const View<M>& operator*() const
    {
        return this->state_->view;
    }

    const View<M>* operator->() const
    {
        return &this->state_->view;
    }

    const View<M>* get() const
    {
        return this->state_ ? &this->state_->view : nullptr;
    }

    explicit operator bool() const
    {
        return static_cast<bool>(this->state_);
    }

    long use_count() const
    {
        return this->state_.use_count();
    }

    void reset()
    {
        this->state_.reset();
    }

    friend bool operator==(const ViewPtr& ptr, std::nullptr_t)
    {
        return !ptr;
    }

  private:
    friend class detail::ViewAccess<M>;

    explicit ViewPtr(std::shared_ptr<detail::ViewState<M>> state) : state_(std::move(state))
    {
    }

    std::shared_ptr<detail::ViewState<M>> state_;
};

/// @brief A message with the fields of @p view, its array copied: what to keep when the
///        bytes must outlive every ViewPtr, or to call code that takes `const M&`.
template <typename M>
M toMessage(const View<M>& view)
{
    detail::FieldWalk fields;
    ros::serialization::Serializer<M>::template allInOne<detail::FieldWalk, const View<M>&>(
        fields, view
    );
    // The span's place among the fields.
    View<M>&     unconst = const_cast<View<M>&>(view);
    const size_t spanAt = detail::visitSpan(unconst, [](auto&) {});
    M            message;
    detail::MessageFromView assign(fields, spanAt);
    ros::serialization::Serializer<M>::template allInOne<detail::MessageFromView, M&>(
        assign, message
    );
    return message;
}

namespace detail {

/// What the library does with a ViewPtr: make one over lent bytes, and decide at the end of
/// a callback or a step what keeps the bytes of the copies still held.
template <typename M>
class ViewAccess
{
  public:
    /// A view decoded from @p bytes, which stay valid until settle(); empty when they do
    /// not decode as @p M.
    static ViewPtr<M> lend(const uint8_t* bytes, size_t length)
    {
        auto state = std::make_shared<ViewState<M>>();
        if (length > UINT32_MAX) {
            return {};
        }
        try {
            ros::serialization::IStream stream(
                const_cast<uint8_t*>(bytes), static_cast<uint32_t>(length)
            );
            ros::serialization::deserialize(stream, state->view);
            if (stream.getLength() != 0U) {
                return {};
            }
        } catch (const ros::serialization::StreamException&) {
            return {};
        }
        state->lent = true;
        return ViewPtr<M>(std::move(state));
    }

    /// The library's own handle @p view is given up. A copy the application still holds
    /// keeps the bytes: through @p keep (true when it kept the slot), else by a copy.
    /// @return True when the bytes were copied.
    template <typename Keep>
    static bool settle(ViewPtr<M>& view, Keep&& keep)
    {
        std::shared_ptr<ViewState<M>> state = std::move(view.state_);
        const bool held = state && state->lent && state.use_count() > 1;
        if (!held) {
            return false;
        }
        HyphaReceivedSample* kept = nullptr;
        if (keep(kept)) {
            state->kept = kept;
            state->lent = false;
            return false;
        }
        state->copyBytes();
        return true;
    }

    /// The end of a cycle step for its own handle @p view: lent bytes are given up, and a
    /// copy the step still holds owns a copy of them. A view of a message stays.
    /// @return True when the bytes were copied.
    static bool release(ViewPtr<M>& view)
    {
        const bool lent = view.state_ && view.state_->lent;
        if (!lent) {
            return false;
        }
        std::shared_ptr<ViewState<M>> state = std::move(view.state_);
        if (state.use_count() == 1) {
            return false;
        }
        state->copyBytes();
        return true;
    }
};

}  // namespace detail

}  // namespace hypha
