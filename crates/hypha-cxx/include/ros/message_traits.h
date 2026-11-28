#pragma once

#include <cstdint>
#include <string>
#include <type_traits>

namespace ros::message_traits {

struct TrueType
{
    static constexpr bool value = true;
    using type = std::true_type;
    using value_type = bool;
};

struct FalseType
{
    static constexpr bool value = false;
    using type = std::false_type;
    using value_type = bool;
};

template <typename M>
struct IsMessage : FalseType
{
};

template <typename M>
struct IsFixedSize : FalseType
{
};

template <typename M>
struct HasHeader : FalseType
{
};

template <typename M>
struct DataType
{
    static const char* value()
    {
        return "";
    }

    static const char* value(const M&)
    {
        return value();
    }
};

template <typename M>
struct MD5Sum
{
    static const char* value()
    {
        return "";
    }

    static const char* value(const M&)
    {
        return value();
    }
};

template <typename M>
struct Definition
{
    static const char* value()
    {
        return "";
    }

    static const char* value(const M&)
    {
        return value();
    }
};

template <typename M>
inline const char* value()
{
    return DataType<M>::value();
}

template <typename M>
inline const char* value(const M& m)
{
    return DataType<M>::value(m);
}

template <typename M>
inline const char* dataType()
{
    return DataType<M>::value();
}

template <typename M>
inline const char* dataType(const M& m)
{
    return DataType<M>::value(m);
}

template <typename M>
inline const char* md5(const M& m)
{
    return MD5Sum<M>::value(m);
}

template <typename M>
inline const char* definition(const M& m)
{
    return Definition<M>::value(m);
}

}  // namespace ros::message_traits

namespace hypha {

/// @brief A received @p M read in place (hypha/view.h): the generator specialises it, for a
///        message whose loaned field is an array of bytes, with @p M's fields, that one a
///        std::span of the bytes it was received in.
template <typename M>
struct View;

}  // namespace hypha

namespace ros::message_traits {

// A view has the type, the hash and the definition of its message.
template <typename M>
struct IsMessage<::hypha::View<M>> : IsMessage<M>
{
};

template <typename M>
struct HasHeader<::hypha::View<M>> : HasHeader<M>
{
};

template <typename M>
struct DataType<::hypha::View<M>> : DataType<M>
{
};

template <typename M>
struct MD5Sum<::hypha::View<M>> : MD5Sum<M>
{
};

template <typename M>
struct Definition<::hypha::View<M>> : Definition<M>
{
};

}  // namespace ros::message_traits

namespace hypha::message_traits {

/// @brief The field of @p M that `Publisher<M>::loan()` writes in place, in the slot:
///        `member` points to it. The generator specialises it for a message with a single
///        top-level array of numbers that only fixed-size numbers follow (`Image::data`,
///        `PointCloud2::data`, the `*MultiArray` types: 15 standard types); others have none.
template <typename M>
struct LoanedField
{
};

}  // namespace hypha::message_traits
