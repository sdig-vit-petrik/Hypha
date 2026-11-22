# autoware_internal_debug_msgs

The messages of https://github.com/autowarefoundation/autoware_internal_msgs, tag `1.16.0`
(commit `4144effa727d194554ad78d343683201bbbbc427`), the version Autoware 2.1.0 pins, that the
ported nodes publish. Licence: `LICENSE` (Apache 2.0).

- `ProcessingTimeNode.msg`, `ProcessingTimeTree.msg`: unchanged; their ROS 2 definitions are
  valid ROS 1 syntax.
- `Float64Stamped.msg`: rewritten in ROS 1 syntax (`time` for `builtin_interfaces/Time`)
  until hypha reads ROS 2 definitions (#45); the upstream definition is in its comment.
