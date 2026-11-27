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

namespace hypha::message_traits {

/// @brief The field of @p M that `Publisher<M>::loan()` writes in place, in the slot:
///        `member` points to it. The generator specialises it for a message whose last
///        variable-length field holds numbers and is followed by fixed-size numbers only
///        (`sensor_msgs/Image::data`, `sensor_msgs/PointCloud2::data`); others have none.
template <typename M>
struct LoanedField
{
};

}  // namespace hypha::message_traits
