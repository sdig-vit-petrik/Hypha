#pragma once

/// @file
/// rmw's QoS policy kinds (rmw/types.h, ROS 2 jazzy), which rclcpp::QoS takes as well as its
/// own. The shim's QoS holds reliable or best effort, volatile or transient local; it
/// refuses the kinds that leave the choice to the middleware.

enum rmw_qos_reliability_policy_e {
    RMW_QOS_POLICY_RELIABILITY_SYSTEM_DEFAULT,
    RMW_QOS_POLICY_RELIABILITY_RELIABLE,
    RMW_QOS_POLICY_RELIABILITY_BEST_EFFORT,
    RMW_QOS_POLICY_RELIABILITY_UNKNOWN,
    RMW_QOS_POLICY_RELIABILITY_BEST_AVAILABLE
};

using rmw_qos_reliability_policy_t = rmw_qos_reliability_policy_e;

enum rmw_qos_durability_policy_e {
    RMW_QOS_POLICY_DURABILITY_SYSTEM_DEFAULT,
    RMW_QOS_POLICY_DURABILITY_TRANSIENT_LOCAL,
    RMW_QOS_POLICY_DURABILITY_VOLATILE,
    RMW_QOS_POLICY_DURABILITY_UNKNOWN,
    RMW_QOS_POLICY_DURABILITY_BEST_AVAILABLE
};

using rmw_qos_durability_policy_t = rmw_qos_durability_policy_e;
