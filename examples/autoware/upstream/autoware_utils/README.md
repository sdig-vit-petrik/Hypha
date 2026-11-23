# autoware_utils (vendored)

From https://github.com/autowarefoundation/autoware_utils, tag `1.11.0`, commit
`9cb6f102ab4fce01a1ba1b847c9a57e822184268` (9cb6f10), the version that Autoware 2.1.0
pins. Licence: `LICENSE` in this directory (Apache 2.0).

Only these files, unchanged:

- `autoware_utils_system/include/autoware_utils_system/stop_watch.hpp`;
- `autoware_utils_math/include/autoware_utils_math/`: `constants.hpp`, `normalization.hpp`,
  `unit_conversion.hpp`;
- `autoware_utils_debug/include/autoware_utils_debug/time_keeper.hpp` and
  `autoware_utils_debug/src/time_keeper.cpp`, the processing-time tree;
- `autoware_utils_diagnostics/include/autoware_utils_diagnostics/diagnostics_interface.hpp`,
  the header-only diagnostics publisher of the clustering, on the shim's `rclcpp`;
- `autoware_utils_geometry/include/autoware_utils_geometry/msg/covariance.hpp`, the
  covariance indices of the EKF;
- of the `autoware_utils` umbrella package, the forwarding headers the ported nodes
  include: `ros/debug_publisher.hpp`, `ros/diagnostics_interface.hpp`,
  `ros/published_time_publisher.hpp`, `system/stop_watch.hpp`, `geometry/geometry.hpp` and
  `math/unit_conversion.hpp`, each a `using namespace` of the split package;
- `autoware_utils_geometry`'s Boost.Geometry types and polygon utilities
  (`boost_geometry.hpp`, `boost_polygon_utils.hpp`, `src/geometry/boost_polygon_utils.cpp`),
  for the tracker; the patch removes the `PredictedObject` overloads, whose message holds a
  `builtin_interfaces/Duration` (#45);

The ROS-bound utilities the ported nodes use (`autoware_utils_debug`'s debug and
published-time publishers, `autoware_utils_tf`'s transform listener) are the shim's,
in `../../shim/include/`, on hypha endpoints and `hypha::TransformBuffer`; so are the
functions of `autoware_utils_geometry/geometry.hpp` the ported nodes use (the distances,
`get_rpy`, `create_quaternion_from_rpy`, `create_point`, `pose2transform`), copied from
upstream's code, whose header brings the planning messages and Boost.Geometry; the
logger level service of `autoware_utils_logging` is the shim's too, as hypha sets log
levels from the parameter file only.
