#pragma once

/// @file
/// autoware_agnocast_wrapper (autoware_core 1.10.0) as its build without Agnocast: the
/// macros are upstream's (upstream/autoware_core/common/autoware_agnocast_wrapper), and
/// the node, its timers and services are the shim's rclcpp ones.

#include <autoware/agnocast_wrapper/macros.hpp>
#include <autoware/agnocast_wrapper/node.hpp>
