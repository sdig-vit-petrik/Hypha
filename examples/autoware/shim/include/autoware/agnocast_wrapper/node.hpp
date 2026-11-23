#pragma once

/// @file
/// autoware::agnocast_wrapper::Node without Agnocast: upstream forwards every call to an
/// rclcpp::Node, so the shim's rclcpp::Node is the node. Its endpoints, timers and
/// services are those of the shim.

#include <autoware/agnocast_wrapper/macros.hpp>
#include <rclcpp/rclcpp.hpp>

#include <utility>

namespace autoware::agnocast_wrapper {

using Node = rclcpp::Node;

template <class S>
using Service = rclcpp::Service<S>;

template <class S>
using Client = rclcpp::Client<S>;

/// Upstream's free function for a timer on @p clock; the group changes nothing.
template <class Callback>
rclcpp::TimerBase::SharedPtr create_timer(
    Node*                                  node,
    const rclcpp::Clock::SharedPtr&        clock,
    const rclcpp::Duration&                period,
    Callback&&                             callback,
    const rclcpp::CallbackGroup::SharedPtr& /*group*/ = nullptr
)
{
    return rclcpp::create_timer(node, clock, period, std::forward<Callback>(callback));
}

}  // namespace autoware::agnocast_wrapper
