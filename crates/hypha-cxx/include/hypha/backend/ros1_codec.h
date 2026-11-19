#pragma once

/// @file
/// Turns a ROS 1 message into bytes and back, and reports its type identity.

#include <ros/message_traits.h>
#include <ros/serialization.h>

#include <cstdint>
#include <cstring>
#include <string>
#include <type_traits>
#include <vector>

namespace hypha::codec {

/// @brief The message's ROS type name, e.g. "geometry_msgs/PoseStamped".
template <typename M>
const char* dataType()
{
    return ros::message_traits::DataType<M>::value();
}

/// @brief The message definition hash.
template <typename M>
const char* md5Sum()
{
    return ros::message_traits::MD5Sum<M>::value();
}

/// @brief The full .msg text.
template <typename M>
const char* definition()
{
    return ros::message_traits::Definition<M>::value();
}

/// @brief Identity a receiver checks before decoding: "<type>/<md5>".
template <typename M>
std::string typeTag()
{
    return std::string(dataType<M>()) + "/" + md5Sum<M>();
}

/// @brief Serialise into @p out, resizing it to the exact length.
template <typename M>
void encode(const M& message, std::vector<uint8_t>& out)
{
    const uint32_t length = ros::serialization::serializationLength(message);
    out.resize(length);
    if (length > 0) {
        ros::serialization::OStream stream(out.data(), length);
        ros::serialization::serialize(stream, message);
    }
}

/// @brief Return the exact ROS 1 serialized length.
template <typename M>
size_t encodedLength(const M& message)
{
    return ros::serialization::serializationLength(message);
}

namespace detail {

/// @brief Serialize into caller-owned storage of exactly @p length bytes, where
///        @p length must be encodedLength(message) for this same message. Unchecked.
///        A smaller @p length writes past @p out.
/// @return False for a null buffer, a length beyond 32 bits, or a length the message
///         does not fill exactly.
template <typename M>
bool encodeUnchecked(const M& message, uint8_t* out, size_t length)
{
    const bool refused = length > UINT32_MAX || (length > 0 && out == nullptr);
    if (refused) {
        return false;
    }
    if (length == 0) {
        return ros::serialization::serializationLength(message) == 0U;
    }
    ros::serialization::UncheckedOStream stream(out, static_cast<uint32_t>(length));
    ros::serialization::serialize(stream, message);
    return stream.getLength() == 0U;
}

/// @brief Finds where one uint32 field of a message lands in its ROS 1 encoding.
class FieldOffsetStream
{
  public:
    explicit FieldOffsetStream(const uint32_t* target)
        : target_(reinterpret_cast<std::uintptr_t>(target))
    {
    }

    template <typename T>
    void next(const T& field)
    {
        if (this->found_) {
            return;
        }
        const std::uintptr_t begin = reinterpret_cast<std::uintptr_t>(&field);
        if constexpr (std::is_same_v<T, uint32_t>) {
            if (begin == this->target_) {
                this->found_ = true;
                return;
            }
        } else if constexpr (requires(FieldOffsetStream& stream, const T& value) {
                                 ros::serialization::Serializer<T>::
                                     template allInOne<FieldOffsetStream, const T&>(stream, value);
                             }) {
            const bool holdsTarget = this->target_ >= begin && this->target_ < begin + sizeof(T);
            if (holdsTarget) {
                ros::serialization::Serializer<T>::template allInOne<FieldOffsetStream, const T&>(
                    *this, field
                );
                return;
            }
        }
        this->offset_ += ros::serialization::serializationLength(field);
    }

    bool found() const
    {
        return this->found_;
    }

    size_t offset() const
    {
        return this->offset_;
    }

  private:
    std::uintptr_t target_;
    size_t         offset_{0};
    bool           found_{false};
};

/// @brief encodeUnchecked(), then write @p seq over `header.seq` in the encoded bytes
///        when M has one.
template <typename M>
bool encodeWithSeq(const M& message, uint8_t* out, size_t length, uint32_t seq)
{
    if (!encodeUnchecked(message, out, length)) {
        return false;
    }
    if constexpr (requires { message.header.seq; }) {
        static_assert(
            std::is_same_v<std::remove_cvref_t<decltype(message.header.seq)>, uint32_t>,
            "header.seq must be uint32"
        );
        FieldOffsetStream stream(&message.header.seq);
        ros::serialization::Serializer<M>::template allInOne<FieldOffsetStream, const M&>(
            stream, message
        );
        const bool noRoom = !stream.found() || stream.offset() + sizeof(seq) > length;
        if (noRoom) {
            return false;
        }
        // The serializer writes integers in host order; so does this.
        std::memcpy(out + stream.offset(), &seq, sizeof(seq));
    }
    return true;
}

}  // namespace detail

/// @brief Serialize into caller-owned storage, checking @p capacity against the length.
/// @return False when @p capacity is not exactly the encoded length.
template <typename M>
bool encodeInto(const M& message, uint8_t* out, size_t capacity)
{
    const uint32_t length = ros::serialization::serializationLength(message);
    if (capacity != length) {
        return false;
    }
    return detail::encodeUnchecked(message, out, length);
}

template <typename M>
std::vector<uint8_t> encode(const M& message)
{
    std::vector<uint8_t> out;
    encode(message, out);
    return out;
}

/// @brief Deserialise from @p bytes into @p message.
/// @return True on success; false on truncation, corrupt fields, or trailing bytes.
template <typename M>
bool decode(const uint8_t* bytes, size_t length, M& message)
{
    if (length > UINT32_MAX) {
        return false;
    }
    try {
        ros::serialization::IStream stream(
            const_cast<uint8_t*>(bytes), static_cast<uint32_t>(length)
        );
        ros::serialization::deserialize(stream, message);
        const bool consumedEverything = stream.getLength() == 0U;
        return consumedEverything;
    } catch (const ros::serialization::StreamException&) {
        return false;
    }
}

template <typename M>
bool decode(const std::vector<uint8_t>& bytes, M& out)
{
    return decode(bytes.data(), bytes.size(), out);
}

}  // namespace hypha::codec
