#pragma once

#include <hypha_msgs/ros_time.h>

#include <array>
#include <bit>
#include <cassert>
#include <cstdint>
#include <cstring>
#include <stdexcept>
#include <string>
#include <type_traits>
#include <vector>

namespace ros::serialization {

// Primitives are copied byte for byte and the ROS 1 wire format is little-endian.
static_assert(
    std::endian::native == std::endian::little,
    "The ROS 1 codec encodes primitives in target byte order and needs a "
    "little-endian target."
);

/// Base of every error this decoder raises.
class StreamException : public std::runtime_error
{
  public:
    explicit StreamException(const std::string& what) : std::runtime_error(what)
    {
    }
};

/// A read or write reached past the buffer.
class StreamOverrunException : public StreamException
{
  public:
    explicit StreamOverrunException(const std::string& what) : StreamException(what)
    {
    }
};

/// A field held a bit pattern its type does not permit.
class InvalidValueException : public StreamException
{
  public:
    explicit InvalidValueException(const std::string& what) : StreamException(what)
    {
    }
};

/// Out of line and cold, to keep the inlined write path small.
[[gnu::cold, gnu::noinline]] inline void throwStreamOverrun()
{
    throw StreamOverrunException("Buffer overrun during serialization/deserialization");
}

inline void throwInvalidValue(const std::string& what)
{
    throw InvalidValueException(what);
}

/// Maximum elements an array may declare when its element width is not fixed.
inline constexpr uint32_t MAX_UNBOUNDED_ARRAY_ELEMENTS = 1U << 20;

/// The fewest bytes an element of `T` occupies on the wire, or zero when no minimum is
/// known, as for a generated message.
template <typename T>
struct EncodedWidth
{
    static constexpr uint32_t minBytes =
        std::is_arithmetic_v<T> ? static_cast<uint32_t>(sizeof(T)) : 0U;
};

template <>
struct EncodedWidth<bool>
{
    static constexpr uint32_t minBytes = 1U;
};

template <>
struct EncodedWidth<hypha_msgs::RosTime>
{
    static constexpr uint32_t minBytes = 8U;
};

template <>
struct EncodedWidth<hypha_msgs::RosDuration>
{
    static constexpr uint32_t minBytes = 8U;
};

template <>
struct EncodedWidth<std::string>
{
    static constexpr uint32_t minBytes = 4U;
};

template <typename T>
struct EncodedWidth<std::vector<T>>
{
    static constexpr uint32_t minBytes = 4U;
};

template <typename T, size_t N>
struct EncodedWidth<std::array<T, N>>
{
    static constexpr uint32_t minBytes = static_cast<uint32_t>(N) * EncodedWidth<T>::minBytes;
};

/// Refuse a declared length the remaining bytes cannot hold. Compares counts, never a
/// pointer past the buffer.
template <typename Stream>
inline void requireRemaining(const Stream& stream, uint64_t needed)
{
    if (needed > static_cast<uint64_t>(stream.getLength())) {
        throwStreamOverrun();
    }
}

/// Refuse an element count before anything is reserved for it.
template <typename T, typename Stream>
inline void requireElementBudget(const Stream& stream, uint32_t count)
{
    const uint32_t minBytes = EncodedWidth<T>::minBytes;
    const bool     widthIsKnown = minBytes > 0U;
    if (widthIsKnown) {
        requireRemaining(stream, static_cast<uint64_t>(count) * static_cast<uint64_t>(minBytes));
        return;
    }
    if (count > MAX_UNBOUNDED_ARRAY_ELEMENTS) {
        throwInvalidValue("Array element count exceeds the decoder budget");
    }
}

class IStream
{
  public:
    IStream(uint8_t* data, uint32_t count) : data_(data), end_(data + count)
    {
    }

    void read(void* dest, uint32_t length)
    {
        if (static_cast<uint64_t>(length) > static_cast<uint64_t>(end_ - data_)) {
            throwStreamOverrun();
        }
        std::memcpy(dest, data_, length);
        data_ += length;
    }

    template <typename T>
    void next(T& val);

    uint8_t* getData() const
    {
        return data_;
    }

    uint32_t getLength() const
    {
        return static_cast<uint32_t>(end_ - data_);
    }

    /// Pass @p length bytes, as a view does that reads them in place.
    void skip(uint32_t length)
    {
        if (static_cast<uint64_t>(length) > static_cast<uint64_t>(end_ - data_)) {
            throwStreamOverrun();
        }
        data_ += length;
    }

  private:
    uint8_t* data_;
    uint8_t* end_;
};

/// Checked writer: an overrun throws StreamOverrunException.
class OStream
{
  public:
    OStream(uint8_t* data, uint32_t count) : data_(data), end_(data + count)
    {
    }

    [[gnu::always_inline]] void write(const void* src, uint32_t length)
    {
        if (static_cast<uint64_t>(length) > static_cast<uint64_t>(end_ - data_)) {
            throwStreamOverrun();
        }
        std::memcpy(data_, src, length);
        data_ += length;
    }

    template <typename T>
    [[gnu::always_inline]] void next(const T& val);

    uint8_t* getData() const
    {
        return data_;
    }

    uint32_t getLength() const
    {
        return static_cast<uint32_t>(end_ - data_);
    }

  private:
    uint8_t* data_;
    uint8_t* end_;
};

/// Unchecked writer for a buffer whose size the caller has already matched to the
/// message's encoded length. Debug builds assert each write.
class UncheckedOStream
{
  public:
    UncheckedOStream(uint8_t* data, uint32_t count) : data_(data), end_(data + count)
    {
    }

    [[gnu::always_inline]] void write(const void* src, uint32_t length)
    {
        assert(length <= getLength() && "unchecked write past the sized buffer");
        std::memcpy(data_, src, length);
        data_ += length;
    }

    template <typename T>
    [[gnu::always_inline]] void next(const T& val);

    uint8_t* getData() const
    {
        return data_;
    }

    uint32_t getLength() const
    {
        return static_cast<uint32_t>(end_ - data_);
    }

  private:
    uint8_t* data_;
    uint8_t* end_;
};

class LStream
{
  public:
    LStream() : count_(0)
    {
    }

    void write(const void* /*src*/, uint32_t length)
    {
        count_ += length;
    }

    template <typename T>
    void next(const T& val);

    uint32_t getLength() const
    {
        return count_;
    }

  private:
    uint32_t count_;
};

#define ROS_DECLARE_ALLINONE_SERIALIZER                                                            \
    template <typename Stream, typename T>                                                         \
    inline static void write(Stream& stream, const T& t)                                           \
    {                                                                                              \
        allInOne<Stream, const T&>(stream, t);                                                     \
    }                                                                                              \
                                                                                                   \
    template <typename Stream, typename T>                                                         \
    inline static void read(Stream& stream, T& t)                                                  \
    {                                                                                              \
        allInOne<Stream, T&>(stream, t);                                                           \
    }                                                                                              \
                                                                                                   \
    template <typename T>                                                                          \
    inline static uint32_t serializedLength(const T& t)                                            \
    {                                                                                              \
        ::ros::serialization::LStream stream;                                                      \
        allInOne<::ros::serialization::LStream, const T&>(stream, t);                              \
        return stream.getLength();                                                                 \
    }

template <typename T>
struct Serializer;

}  // namespace ros::serialization

namespace hypha {
template <typename M>
struct View;
}  // namespace hypha

namespace ros::serialization {

/// A view has its message's fields, in its message's order: it reads and writes with the
/// same field walk.
template <typename M>
struct Serializer<::hypha::View<M>> : Serializer<M>
{
};

template <typename Stream, typename T>
inline void serialize(Stream& stream, const T& t)
{
    Serializer<T>::write(stream, t);
}

template <typename T>
inline void deserialize(IStream& stream, T& t)
{
    Serializer<T>::read(stream, t);
}

template <typename T>
inline uint32_t serializationLength(const T& t)
{
    return Serializer<T>::serializedLength(t);
}

template <typename T>
inline void IStream::next(T& val)
{
    deserialize(*this, val);
}

template <typename T>
inline void OStream::next(const T& val)
{
    serialize(*this, val);
}

template <typename T>
inline void UncheckedOStream::next(const T& val)
{
    serialize(*this, val);
}

template <typename T>
inline void LStream::next(const T& val)
{
    count_ += serializationLength(val);
}

#define HYPHA_ROS_PRIMITIVE_SERIALIZER(Type)                                                       \
    template <>                                                                                    \
    struct Serializer<Type>                                                                        \
    {                                                                                              \
        template <typename Stream>                                                                 \
        inline static void write(Stream& stream, const Type& v)                                    \
        {                                                                                          \
            stream.write(&v, static_cast<uint32_t>(sizeof(Type)));                                 \
        }                                                                                          \
        template <typename Stream>                                                                 \
        inline static void read(Stream& stream, Type& v)                                           \
        {                                                                                          \
            stream.read(&v, static_cast<uint32_t>(sizeof(Type)));                                  \
        }                                                                                          \
        inline static uint32_t serializedLength(const Type&)                                       \
        {                                                                                          \
            return static_cast<uint32_t>(sizeof(Type));                                            \
        }                                                                                          \
    };

HYPHA_ROS_PRIMITIVE_SERIALIZER(uint8_t)
HYPHA_ROS_PRIMITIVE_SERIALIZER(int8_t)
HYPHA_ROS_PRIMITIVE_SERIALIZER(uint16_t)
HYPHA_ROS_PRIMITIVE_SERIALIZER(int16_t)
HYPHA_ROS_PRIMITIVE_SERIALIZER(uint32_t)
HYPHA_ROS_PRIMITIVE_SERIALIZER(int32_t)
HYPHA_ROS_PRIMITIVE_SERIALIZER(uint64_t)
HYPHA_ROS_PRIMITIVE_SERIALIZER(int64_t)
HYPHA_ROS_PRIMITIVE_SERIALIZER(float)
HYPHA_ROS_PRIMITIVE_SERIALIZER(double)
#undef HYPHA_ROS_PRIMITIVE_SERIALIZER

}  // namespace ros::serialization
