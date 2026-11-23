# autoware_internal_debug_msgs

The messages of https://github.com/autowarefoundation/autoware_internal_msgs, tag `1.16.0`
(commit `4144effa727d194554ad78d343683201bbbbc427`), the version Autoware 2.1.0 pins, that the
ported nodes publish. Licence: `LICENSE` (Apache 2.0).

- `ProcessingTimeNode.msg`, `ProcessingTimeTree.msg`: unchanged; their ROS 2 definitions are
  valid ROS 1 syntax.
- `BoolStamped.msg`, `Float64Stamped.msg`, `Float64MultiArrayStamped.msg`,
  `Int32Stamped.msg`, `StringStamped.msg`: rewritten in ROS 1 syntax
  (`time` for `builtin_interfaces/Time`) until hypha reads ROS 2 definitions (#45); the
  upstream definition is in each one's comment. The concatenate node includes the two
  last and publishes neither; the EKF publishes `Float64Stamped` and `Float64MultiArrayStamped`, the GNSS poser
  `BoolStamped`.
