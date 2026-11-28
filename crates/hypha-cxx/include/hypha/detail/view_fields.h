#pragma once

/// @file
/// The field walks of a View (hypha/view.h): the generated serializer's walk over a message's
/// or a view's top-level fields, used to make one from the other.

#include <ros/message_traits.h>
#include <ros/serialization.h>

#include <array>
#include <cstddef>
#include <span>
#include <stdexcept>
#include <type_traits>
#include <vector>

namespace hypha::detail {

template <typename T>
struct IsSpan : std::false_type
{
};

template <typename E>
struct IsSpan<std::span<const E>> : std::true_type
{
    using Element = E;
};

template <typename T>
struct IsVector : std::false_type
{
};

template <typename E, typename A>
struct IsVector<std::vector<E, A>> : std::true_type
{
};

template <typename T>
struct IsView : std::false_type
{
};

template <typename M>
struct IsView<View<M>> : std::true_type
{
    using Message = M;
};

/// The addresses of the top-level fields of a message or a view, in their order.
class FieldWalk
{
  public:
    template <typename T>
    void next(const T& field)
    {
        if (this->count_ >= this->at_.size()) {
            throw std::length_error("hypha: a view of more than 64 fields");
        }
        this->at_[this->count_] = &field;
        ++this->count_;
    }

    const void* at(size_t index) const
    {
        return this->at_.at(index);
    }

  private:
    std::array<const void*, 64> at_{};
    size_t                      count_{0};
};

/// Sets each field of a view from the field at the same place of its message: the span of
/// the message's array, a copy of every other field.
class ViewFromMessage
{
  public:
    explicit ViewFromMessage(const FieldWalk& message) : message_(message)
    {
    }

    template <typename T>
    void next(T& field)
    {
        const void* source = this->message_.at(this->index_++);
        if constexpr (IsSpan<T>::value) {
            using Element = typename IsSpan<T>::Element;
            const auto& array = *static_cast<const std::vector<Element>*>(source);
            field = T(array.data(), array.size());
        } else {
            field = *static_cast<const T*>(source);
        }
    }

  private:
    const FieldWalk& message_;
    size_t           index_{0};
};

/// Sets each field of a message from the field at the same place of its view, copying the
/// bytes of the span.
class MessageFromView
{
  public:
    MessageFromView(const FieldWalk& view, size_t spanAt) : view_(view), spanAt_(spanAt)
    {
    }

    template <typename T>
    void next(T& field)
    {
        const size_t index = this->index_++;
        const void*  source = this->view_.at(index);
        if constexpr (IsVector<T>::value) {
            if (index == this->spanAt_) {
                using Element = typename T::value_type;
                const auto& bytes = *static_cast<const std::span<const Element>*>(source);
                field.assign(bytes.begin(), bytes.end());
                return;
            }
        }
        field = *static_cast<const T*>(source);
    }

  private:
    const FieldWalk& view_;
    size_t           spanAt_;
    size_t           index_{0};
};

/// Calls @p visit with the span field of a view.
template <typename Visit>
class SpanWalk
{
  public:
    explicit SpanWalk(Visit& visit) : visit_(visit)
    {
    }

    template <typename T>
    void next(T& field)
    {
        if constexpr (IsSpan<T>::value) {
            this->visit_(field);
            this->spanAt_ = this->index_;
        }
        ++this->index_;
    }

    size_t spanAt() const
    {
        return this->spanAt_;
    }

  private:
    Visit& visit_;
    size_t index_{0};
    size_t spanAt_{0};
};

template <typename M, typename Visit>
size_t visitSpan(View<M>& view, Visit visit)
{
    SpanWalk<Visit> walk(visit);
    ros::serialization::Serializer<M>::template allInOne<SpanWalk<Visit>, View<M>&>(walk, view);
    return walk.spanAt();
}

}  // namespace hypha::detail
