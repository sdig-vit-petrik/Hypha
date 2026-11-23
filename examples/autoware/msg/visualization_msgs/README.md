# visualization_msgs

`Marker`, `MarkerArray`, `UVCoordinate` and `MeshFile` of https://github.com/ros2/common_interfaces,
branch `jazzy`, commit `a941f14bb318d8d904505ed935ccbb97f24a70a4` (a941f14), the definitions
`autoware_localization_util`'s covariance ellipse marker uses. hypha does not ship
`visualization_msgs`. Licence: `LICENSE` (Apache 2.0).

- `MarkerArray.msg`, `UVCoordinate.msg`, `MeshFile.msg`: unchanged; their ROS 2 definitions
  are valid ROS 1 syntax.
- `Marker.msg`: `duration` for `builtin_interfaces/Duration` until hypha reads ROS 2
  definitions (#45); otherwise unchanged.
