#pragma once

/// @file
/// A message written in place: its large field lives in the shared-memory slot it is sent
/// from, and the application fills it there.

#include "hypha/backend/raw_publisher.h"

#include <ros/message_traits.h>
#include <ros/serialization.h>

#include <cassert>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <optional>
#include <span>
#include <type_traits>
#include <utility>
#include <vector>

namespace hypha {

/// @brief A message type with a field that a loan writes in place: its single top-level
///        array of numbers (hypha::message_traits::LoanedField, emitted by the generator).
template <typename M>
concept LoanableMessage = requires { hypha::message_traits::LoanedField<M>::member; };

namespace backend {
template <typename M>
class PublisherHandle;
}  // namespace backend

namespace detail {

/// @brief The loaned field's type, a std::vector of numbers; std::vector<std::byte> for a
///        type with none, so that only the static_assert of Loan reports it.
template <typename M>
struct LoanedFieldOf
{
    using type = std::vector<std::byte>;
};

template <LoanableMessage M>
struct LoanedFieldOf<M>
{
    using type =
        std::remove_cvref_t<decltype(std::declval<M&>().*message_traits::LoanedField<M>::member)>;
};

template <typename M>
using LoanedFieldType = typename LoanedFieldOf<M>::type;

/// @brief The ROS 1 encoding of a message whose loaned field is a gap: the field's length
///        prefix says @p count elements and @p gapBytes are left as they are, because they
///        already hold them. With a null output it only counts. It records where the gap
///        starts and where the `uint32` at @p seq lands, so a caller checks the shape and
///        sets `header.seq` after this one pass.
template <typename Field>
class GapStream
{
  public:
    GapStream(
        uint8_t*        out,
        size_t          size,
        const Field*    field,
        uint32_t        count,
        size_t          gapBytes,
        const uint32_t* seq = nullptr
    )
        : out_(out), size_(size), field_(field), seq_(seq), count_(count), gapBytes_(gapBytes)
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
        if constexpr (std::is_same_v<T, uint32_t>) {
            if (&value == this->seq_) {
                this->seqOffset_ = this->position_;
            }
        }
        ros::serialization::serialize(*this, value);
    }

    /// @brief True when the whole message fit and the gap was reached.
    bool complete() const
    {
        return !this->overrun_ && this->gapOffset_ != NONE;
    }

    size_t gapOffset() const
    {
        return this->gapOffset_;
    }

    /// @brief Where `header.seq` was written, or no value when the message has none.
    std::optional<size_t> seqOffset() const
    {
        return this->seqOffset_ == NONE ? std::nullopt : std::optional<size_t>(this->seqOffset_);
    }

    size_t length() const
    {
        return this->position_;
    }

  private:
    static constexpr size_t NONE = static_cast<size_t>(-1);

    uint8_t*        out_;
    size_t          size_;
    const Field*    field_;
    const uint32_t* seq_;
    uint32_t        count_;
    size_t          gapBytes_;
    size_t          position_{0};
    size_t          gapOffset_{NONE};
    size_t          seqOffset_{NONE};
    bool            overrun_{false};
};

}  // namespace detail

/// @brief One message of @p M being written in its shared-memory slot. Publisher<M>::loan()
///        makes it; publish() through the same publisher sends it. Dropped unsent, it
///        returns the slot. Moved from, it is empty: capacity() 0, data() empty.
///
/// message() holds the small fields. The lengths of its strings and arrays are fixed by
/// loan(); their contents and its numbers (a stamp, `width`, `is_dense`) may change until
/// the publish. data() is the loaned field's bytes inside the slot. They are not aligned
/// (the ROS 1 encoding has no padding): write them with std::memcpy or put(), never through
/// a cast pointer. A span taken from data() must not be used after the publish.
template <typename M>
class Loan
{
    static_assert(
        LoanableMessage<M>,
        "hypha::Loan: this message has no single top-level array of numbers to write in place; "
        "send it with publish(const M&)"
    );

  public:
    /// The number type of the loaned field.
    using Element = typename detail::LoanedFieldType<M>::value_type;

    Loan(Loan&& other) noexcept
        : head_(std::move(other.head_))
        , buffer_(std::move(other.buffer_))
        , owner_(std::exchange(other.owner_, nullptr))
        , offset_(std::exchange(other.offset_, 0))
        , capacity_(std::exchange(other.capacity_, 0))
        , headLength_(std::exchange(other.headLength_, 0))
    {
    }

    Loan& operator=(Loan&& other) noexcept
    {
        if (this != &other) {
            this->head_ = std::move(other.head_);
            this->buffer_ = std::move(other.buffer_);
            this->owner_ = std::exchange(other.owner_, nullptr);
            this->offset_ = std::exchange(other.offset_, 0);
            this->capacity_ = std::exchange(other.capacity_, 0);
            this->headLength_ = std::exchange(other.headLength_, 0);
        }
        return *this;
    }

    Loan(const Loan&) = delete;
    Loan& operator=(const Loan&) = delete;
    ~Loan() = default;

    /// @brief The message's other fields. The loaned field must stay empty.
    M& message()
    {
        return this->head_;
    }

    /// @brief The loaned field's bytes in the slot, capacity() of them, not initialised.
    ///        Valid until the loan is published, moved or destroyed.
    std::span<std::byte> data()
    {
        if (this->capacity_ == 0) {
            return {};
        }
        return {
            reinterpret_cast<std::byte*>(this->buffer_.data()) + this->offset_, this->capacity_
        };
    }

    /// @brief Write @p value as element @p index of an array of @p T in data(), at byte
    ///        `index * sizeof(T)`, with std::memcpy: a lidar point, a pixel, a number.
    ///        Debug builds assert that it fits.
    template <typename T>
        requires std::is_trivially_copyable_v<T>
    void put(size_t index, const T& value)
    {
        assert(index < this->capacity_ / sizeof(T) && "Loan::put past capacity()");
        std::memcpy(this->data().data() + index * sizeof(T), &value, sizeof(T));
    }

    /// @brief The bytes loan() reserved for the loaned field; 0 once moved from.
    size_t capacity() const
    {
        return this->capacity_;
    }

  private:
    friend class backend::PublisherHandle<M>;

    Loan(
        M                     head,
        backend::LoanedBuffer buffer,
        const void*           owner,
        size_t                offset,
        size_t                capacity,
        size_t                headLength
    )
        : head_(std::move(head))
        , buffer_(std::move(buffer))
        , owner_(owner)
        , offset_(offset)
        , capacity_(capacity)
        , headLength_(headLength)
    {
    }

    M                     head_;
    backend::LoanedBuffer buffer_;
    /// The publisher endpoint that lent it, which alone may send it.
    const void* owner_;
    size_t      offset_;
    size_t      capacity_;
    /// The encoded length of head_ with an empty loaned field.
    size_t headLength_;
};

}  // namespace hypha
