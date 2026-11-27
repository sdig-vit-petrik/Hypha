#pragma once

/// @file
/// A message written in place: its large field lives in the shared-memory slot it is sent
/// from, and the application fills it there.

#include "hypha/backend/raw_publisher.h"

#include <ros/message_traits.h>
#include <ros/serialization.h>

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <span>
#include <type_traits>
#include <utility>

namespace hypha {

/// @brief A message type with a field that a loan writes in place
///        (hypha::message_traits::LoanedField).
template <typename M>
concept LoanableMessage = requires { hypha::message_traits::LoanedField<M>::member; };

namespace backend {
template <typename M>
class PublisherHandle;
}  // namespace backend

namespace detail {

/// @brief The type of @p M's loaned field: a std::vector of numbers.
template <LoanableMessage M>
using LoanedFieldType =
    std::remove_cvref_t<decltype(std::declval<M&>().*message_traits::LoanedField<M>::member)>;

/// @brief The ROS 1 encoding of a message whose loaned field is a gap: the field's length
///        prefix says @p count elements and @p gapBytes are left as they are, because they
///        already hold them. With a null output it only counts: where the gap starts and
///        how long the encoding is.
template <typename Field>
class GapStream
{
  public:
    GapStream(uint8_t* out, size_t size, const Field* field, uint32_t count, size_t gapBytes)
        : out_(out), size_(size), field_(field), count_(count), gapBytes_(gapBytes)
    {
    }

    void write(const void* source, uint32_t length)
    {
        if (length > this->size_ - this->position_) {
            this->overrun_ = true;
            return;
        }
        if (this->out_ != nullptr) {
            std::memcpy(this->out_ + this->position_, source, length);
        }
        this->position_ += length;
    }

    template <typename T>
    void next(const T& value)
    {
        if constexpr (std::is_same_v<T, Field>) {
            if (&value == this->field_) {
                this->write(&this->count_, sizeof(this->count_));
                this->gapOffset_ = this->position_;
                const bool fits = this->gapBytes_ <= this->size_ - this->position_;
                this->overrun_ = this->overrun_ || !fits;
                this->position_ += fits ? this->gapBytes_ : 0;
                return;
            }
        }
        ros::serialization::serialize(*this, value);
    }

    /// @brief True when the whole message fit and the gap was reached.
    bool complete() const
    {
        return !this->overrun_ && this->gapOffset_ != NO_GAP;
    }

    size_t gapOffset() const
    {
        return this->gapOffset_;
    }

    size_t length() const
    {
        return this->position_;
    }

  private:
    static constexpr size_t NO_GAP = static_cast<size_t>(-1);

    uint8_t*     out_;
    size_t       size_;
    const Field* field_;
    uint32_t     count_;
    size_t       gapBytes_;
    size_t       position_{0};
    size_t       gapOffset_{NO_GAP};
    bool         overrun_{false};
};

/// @brief Where @p head's loaned field starts in its encoding, and how long the encoding is
///        with @p gapBytes in that field.
template <LoanableMessage M>
GapStream<LoanedFieldType<M>> measureGap(const M& head, size_t gapBytes)
{
    GapStream<LoanedFieldType<M>> stream(
        nullptr,
        static_cast<size_t>(-1),
        &(head.*message_traits::LoanedField<M>::member),
        0,
        gapBytes
    );
    ros::serialization::serialize(stream, head);
    return stream;
}

}  // namespace detail

/// @brief One message of @p M being written in its shared-memory slot. Publisher<M>::loan()
///        makes it; Publisher<M>::publish() sends it. Dropped unsent, it returns the slot.
///
/// message() holds the small fields. Those that set the size of the encoding (strings,
/// arrays) are fixed by loan(); the others (a stamp, `width`, `is_dense`) may change until
/// the publish. data() is the loaned field's bytes, inside the slot.
template <typename M>
class Loan
{
    static_assert(LoanableMessage<M>, "M has no field a loan writes in place");

  public:
    /// The number type of the loaned field.
    using Element = typename detail::LoanedFieldType<M>::value_type;

    Loan(Loan&&) noexcept = default;
    Loan& operator=(Loan&&) noexcept = default;
    Loan(const Loan&) = delete;
    Loan& operator=(const Loan&) = delete;

    /// @brief The message's other fields. The loaned field must stay empty.
    M& message()
    {
        return this->head_;
    }

    /// @brief The loaned field's bytes in the slot, capacity() of them. Valid until the
    ///        loan is published or destroyed. The bytes are not initialised.
    std::span<std::byte> data()
    {
        return {
            reinterpret_cast<std::byte*>(this->buffer_.data() + this->offset_), this->capacity_
        };
    }

    /// @brief The bytes loan() reserved for the loaned field.
    size_t capacity() const
    {
        return this->capacity_;
    }

  private:
    friend class backend::PublisherHandle<M>;

    Loan(M head, backend::LoanedBuffer buffer, size_t offset, size_t capacity, size_t headLength)
        : head_(std::move(head))
        , buffer_(std::move(buffer))
        , offset_(offset)
        , capacity_(capacity)
        , headLength_(headLength)
    {
    }

    M                     head_;
    backend::LoanedBuffer buffer_;
    size_t                offset_;
    size_t                capacity_;
    /// The encoded length of head_ with an empty loaned field.
    size_t headLength_;
};

}  // namespace hypha
