#pragma once

#include <hypha_msgs/ros_time.h>

#include <ros/serialization_core.h>

#include <array>
#include <cstdint>
#include <span>
#include <string>
#include <type_traits>
#include <vector>

namespace ros::serialization {

/// A bool is one wire byte: written as 0 or 1, read with every non-zero byte as true.
/// The byte is never copied into the `bool`, which would be undefined behaviour.
template <>
struct Serializer<bool>
{
    template <typename Stream>
    inline static void write(Stream& stream, const bool& v)
    {
        const uint8_t encoded = v ? static_cast<uint8_t>(1) : static_cast<uint8_t>(0);
        stream.write(&encoded, 1);
    }

    template <typename Stream>
    inline static void read(Stream& stream, bool& v)
    {
        uint8_t encoded = 0;
        stream.read(&encoded, 1);
        v = encoded != 0U;
    }

    inline static uint32_t serializedLength(const bool&)
    {
        return 1;
    }
};

template <>
struct Serializer<hypha_msgs::RosTime>
{
    template <typename Stream>
    inline static void write(Stream& stream, const hypha_msgs::RosTime& v)
    {
        stream.write(&v.secs, 4);
        stream.write(&v.nsecs, 4);
    }

    template <typename Stream>
    inline static void read(Stream& stream, hypha_msgs::RosTime& v)
    {
        stream.read(&v.secs, 4);
        stream.read(&v.nsecs, 4);
    }

    inline static uint32_t serializedLength(const hypha_msgs::RosTime&)
    {
        return 8;
    }
};

template <>
struct Serializer<hypha_msgs::RosDuration>
{
    template <typename Stream>
    inline static void write(Stream& stream, const hypha_msgs::RosDuration& v)
    {
        stream.write(&v.secs, 4);
        stream.write(&v.nsecs, 4);
    }

    template <typename Stream>
    inline static void read(Stream& stream, hypha_msgs::RosDuration& v)
    {
        stream.read(&v.secs, 4);
        stream.read(&v.nsecs, 4);
    }

    inline static uint32_t serializedLength(const hypha_msgs::RosDuration&)
    {
        return 8;
    }
};

template <>
struct Serializer<std::string>
{
    template <typename Stream>
    inline static void write(Stream& stream, const std::string& v)
    {
        const uint32_t size = static_cast<uint32_t>(v.size());
        stream.write(&size, 4);
        if (size > 0) {
            stream.write(v.data(), size);
        }
    }

    template <typename Stream>
    inline static void read(Stream& stream, std::string& v)
    {
        uint32_t size = 0;
        stream.read(&size, 4);
        // The declared length is untrusted: check it before reserving for it.
        requireRemaining(stream, static_cast<uint64_t>(size));
        v.resize(size);
        if (size > 0) {
            stream.read(v.data(), size);
        }
    }

    inline static uint32_t serializedLength(const std::string& v)
    {
        return 4 + static_cast<uint32_t>(v.size());
    }
};

/// A run of numeric elements is written and read with one copy. bool is excluded: it is
/// normalised element by element.
template <typename T>
inline constexpr bool kNumericIsByteCopy = std::is_arithmetic_v<T> && !std::is_same_v<T, bool>;

template <typename T>
struct Serializer<std::vector<T>>
{
    template <typename Stream>
    inline static void write(Stream& stream, const std::vector<T>& v)
    {
        const uint32_t size = static_cast<uint32_t>(v.size());
        stream.write(&size, 4);
        if constexpr (kNumericIsByteCopy<T>) {
            writeNumeric(stream, v, size);
        } else {
            for (const auto& elem : v) {
                stream.next(elem);
            }
        }
    }

    template <typename Stream>
    inline static void read(Stream& stream, std::vector<T>& v)
    {
        uint32_t size = 0;
        stream.read(&size, 4);
        requireElementBudget<T>(stream, size);
        if constexpr (kNumericIsByteCopy<T>) {
            readNumeric(stream, v, size);
        } else if constexpr (EncodedWidth<T>::minBytes > 0U) {
            readKnownWidth(stream, v, size);
        } else {
            readUnknownWidth(stream, v, size);
        }
    }

    template <typename Stream>
    inline static void writeNumeric(Stream& stream, const std::vector<T>& v, uint32_t size)
    {
        if (size > 0U) {
            stream.write(v.data(), static_cast<uint32_t>(sizeof(T) * size));
        }
    }

    template <typename Stream>
    inline static void readNumeric(Stream& stream, std::vector<T>& v, uint32_t size)
    {
        v.resize(size);
        if (size > 0U) {
            stream.read(v.data(), static_cast<uint32_t>(sizeof(T) * size));
        }
    }

    template <typename Stream>
    inline static void readKnownWidth(Stream& stream, std::vector<T>& v, uint32_t size)
    {
        v.resize(size);
        for (uint32_t index = 0; index < size; ++index) {
            stream.next(v[index]);
        }
    }

    /// Elements of no known width: each is made only as its bytes are read.
    template <typename Stream>
    inline static void readUnknownWidth(Stream& stream, std::vector<T>& v, uint32_t size)
    {
        v.clear();
        for (uint32_t index = 0; index < size; ++index) {
            v.emplace_back();
            stream.next(v.back());
        }
    }

    inline static uint32_t serializedLength(const std::vector<T>& v)
    {
        uint32_t len = 4;
        for (const auto& elem : v) {
            len += serializationLength(elem);
        }
        return len;
    }
};

/// std::vector<bool> holds bit proxies; each element goes through Serializer<bool>.
template <>
struct Serializer<std::vector<bool>>
{
    template <typename Stream>
    inline static void write(Stream& stream, const std::vector<bool>& v)
    {
        const uint32_t size = static_cast<uint32_t>(v.size());
        stream.write(&size, 4);
        for (const bool elem : v) {
            stream.next(elem);
        }
    }

    template <typename Stream>
    inline static void read(Stream& stream, std::vector<bool>& v)
    {
        uint32_t size = 0;
        stream.read(&size, 4);
        requireElementBudget<bool>(stream, size);
        v.resize(size);
        for (size_t i = 0; i < v.size(); ++i) {
            bool elem = false;
            stream.next(elem);
            v[i] = elem;
        }
    }

    inline static uint32_t serializedLength(const std::vector<bool>& v)
    {
        return 4 + static_cast<uint32_t>(v.size());
    }
};

template <typename T, size_t N>
struct Serializer<std::array<T, N>>
{
    template <typename Stream>
    inline static void write(Stream& stream, const std::array<T, N>& v)
    {
        if constexpr (kNumericIsByteCopy<T>) {
            if constexpr (N > 0) {
                static_assert(
                    sizeof(std::array<T, N>) == N * sizeof(T),
                    "a fixed numeric array must be exactly its elements"
                );
                stream.write(v.data(), static_cast<uint32_t>(N * sizeof(T)));
            }
        } else {
            for (size_t i = 0; i < N; ++i) {
                stream.next(v[i]);
            }
        }
    }

    template <typename Stream>
    inline static void read(Stream& stream, std::array<T, N>& v)
    {
        for (size_t i = 0; i < N; ++i) {
            stream.next(v[i]);
        }
    }

    inline static uint32_t serializedLength(const std::array<T, N>& v)
    {
        if constexpr (kNumericIsByteCopy<T>) {
            static_cast<void>(v);
            return static_cast<uint32_t>(N * sizeof(T));
        } else {
            uint32_t len = 0;
            for (size_t i = 0; i < N; ++i) {
                len += serializationLength(v[i]);
            }
            return len;
        }
    }
};

/// The bytes of a view's array (hypha::View), read in place: the span points into the
/// buffer being read, which must outlive it. Only one-byte elements, whose bytes need no
/// alignment.
template <typename T>
    requires(sizeof(T) == 1 && std::is_arithmetic_v<T>)
struct Serializer<std::span<const T>>
{
    template <typename Stream>
    inline static void write(Stream& stream, const std::span<const T>& v)
    {
        const uint32_t size = static_cast<uint32_t>(v.size());
        stream.write(&size, 4);
        if (size > 0U) {
            stream.write(v.data(), size);
        }
    }

    inline static void read(IStream& stream, std::span<const T>& v)
    {
        uint32_t size = 0;
        stream.read(&size, 4);
        requireRemaining(stream, size);
        v = std::span<const T>(reinterpret_cast<const T*>(stream.getData()), size);
        stream.skip(size);
    }

    inline static uint32_t serializedLength(const std::span<const T>& v)
    {
        return 4U + static_cast<uint32_t>(v.size());
    }
};

}  // namespace ros::serialization
