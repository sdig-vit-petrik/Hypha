#pragma once

/// Uses the companion headers of sensor_msgs under their ROS 1 names; true when every value reads
/// back.
bool sensorCompanionsUsable();

/// The same through the ROS 2 names of the headers and of the messages.
bool sensorCompanionsRos2Usable();
