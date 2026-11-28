#pragma once

/// The standard services of the installed SDK under their ROS names (nav_msgs/GetMap,
/// sensor_msgs/SetCameraInfo, with the MD5 of ROS), and a service and a message the
/// application added to sensor_msgs: true when each reads back.
bool standardServicesUsable();
