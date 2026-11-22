#pragma once

/// @file
/// ROS 2 names for hypha's generated messages: `sensor_msgs::msg::PointCloud2` is
/// `sensor_msgs::PointCloud2`. Until the generator emits ROS 2 style headers (#57) and
/// reads ROS 2 definitions (#45), a message keeps its ROS 1 fields (`header.seq`,
/// `stamp.secs`/`stamp.nsecs`) and its `Ptr`/`ConstPtr` typedefs.

/// Declares `<package>::msg` as a name of `<package>`.
#define HYPHA_RCLCPP_MSG_NAMESPACE(package)                                                        \
    namespace package {                                                                            \
    namespace msg = ::package;                                                                     \
    }

/// Declares `<package>::<Name>_<Allocator>`, the allocator template ROS generated C++ puts
/// under every message, as `<package>::<Name>`: hypha messages use the standard allocator.
#define HYPHA_RCLCPP_MSG_ALLOCATOR_TEMPLATE(package, Name)                                         \
    namespace package {                                                                            \
    template <class Allocator>                                                                     \
    using Name##_ = ::package::Name;                                                               \
    }
