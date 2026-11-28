#pragma once

/// @file
/// A number at any byte address: what the sensor_msgs::PointCloud2Iterator hands out for
/// a field of a cloud, whose bytes may start anywhere (a received view's array follows the
/// ROS 1 encoding's variable-length strings and arrays; a field may sit at an odd offset
/// of a point). Every read and write copies the bytes with std::memcpy, which is defined
/// at any alignment and which the compiler turns into one load or store.

#include <cstddef>
#include <cstring>
#include <type_traits>

namespace hypha {

/// @brief The place of a `T` in a buffer, read and written by copy: a stand-in for the
///        `T&` that a cast pointer would give, which is undefined behaviour when the
///        address is not aligned for `T` (UBSan reports it). `float x = ref;`,
///        `ref = 1.0F;`, `ref += 2.0F;`, `++ref` and `ref == x` read as they do on a
///        `T&`; `float& r = ref` and `&ref` do not compile.
template <typename T>
class UnalignedRef
{
    static_assert(std::is_trivially_copyable_v<T> && !std::is_const_v<T>);

  public:
    explicit UnalignedRef(unsigned char* where) noexcept : where_(where)
    {
    }

    /// Not defaulted, so the proxy is not trivially copyable: passing it to printf's `...`
    /// (`printf("%f", *it)`) is a compile error instead of printing the proxy's bytes.
    UnalignedRef(const UnalignedRef& other) noexcept : where_(other.where_)
    {
    }

    /// The value in the buffer.
    operator T() const noexcept
    {
        T value;
        std::memcpy(&value, this->where_, sizeof(T));
        return value;
    }

    UnalignedRef& operator=(T value) noexcept
    {
        std::memcpy(this->where_, &value, sizeof(T));
        return *this;
    }

    /// Assigns the value in the buffer of @p other (as `a = b` does for `T&`), not the place.
    UnalignedRef& operator=(const UnalignedRef& other) noexcept
    {
        return *this = static_cast<T>(other);
    }

    template <typename V>
    UnalignedRef& operator+=(V value) noexcept
    {
        return *this = static_cast<T>(static_cast<T>(*this) + value);
    }

    template <typename V>
    UnalignedRef& operator-=(V value) noexcept
    {
        return *this = static_cast<T>(static_cast<T>(*this) - value);
    }

    template <typename V>
    UnalignedRef& operator*=(V value) noexcept
    {
        return *this = static_cast<T>(static_cast<T>(*this) * value);
    }

    template <typename V>
    UnalignedRef& operator/=(V value) noexcept
    {
        return *this = static_cast<T>(static_cast<T>(*this) / value);
    }

    template <typename V>
    UnalignedRef& operator%=(V value) noexcept
    {
        return *this = static_cast<T>(static_cast<T>(*this) % value);
    }

    template <typename V>
    UnalignedRef& operator&=(V value) noexcept
    {
        return *this = static_cast<T>(static_cast<T>(*this) & value);
    }

    template <typename V>
    UnalignedRef& operator|=(V value) noexcept
    {
        return *this = static_cast<T>(static_cast<T>(*this) | value);
    }

    template <typename V>
    UnalignedRef& operator^=(V value) noexcept
    {
        return *this = static_cast<T>(static_cast<T>(*this) ^ value);
    }

    template <typename V>
    UnalignedRef& operator<<=(V count) noexcept
    {
        return *this = static_cast<T>(static_cast<T>(*this) << count);
    }

    template <typename V>
    UnalignedRef& operator>>=(V count) noexcept
    {
        return *this = static_cast<T>(static_cast<T>(*this) >> count);
    }

    UnalignedRef& operator++() noexcept
    {
        return *this += 1;
    }

    UnalignedRef& operator--() noexcept
    {
        return *this -= 1;
    }

    /// The value before the increment, as `T&`'s postfix gives.
    T operator++(int) noexcept
    {
        const T before = *this;
        ++*this;
        return before;
    }

    T operator--(int) noexcept
    {
        const T before = *this;
        --*this;
        return before;
    }

  private:
    unsigned char* where_;
};

}  // namespace hypha
