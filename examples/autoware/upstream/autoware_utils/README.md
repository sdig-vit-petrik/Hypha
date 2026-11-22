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
  the header-only diagnostics publisher of the clustering, on the shim's `rclcpp`.

The ROS-bound utilities the ported nodes use (`autoware_utils_debug`'s debug and
published-time publishers, `autoware_utils_tf`'s transform listener) are the shim's,
in `../../shim/include/`, on hypha endpoints and `hypha::TransformBuffer`; so is the one
function of `autoware_utils_geometry` the ground filter uses (`calc_distance3d`), whose
upstream header brings the planning messages, tf2 and Boost.Geometry.
