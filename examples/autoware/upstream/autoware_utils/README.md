# autoware_utils (vendored)

From https://github.com/autowarefoundation/autoware_utils, tag `1.11.0`, commit
`9cb6f102ab4fce01a1ba1b847c9a57e822184268` (9cb6f10), the version that Autoware 2.1.0
pins. Licence: `LICENSE` in this directory (Apache 2.0).

Only `autoware_utils_system/include/autoware_utils_system/stop_watch.hpp`, unchanged.
The ROS-bound utilities the ported nodes use (`autoware_utils_debug`'s debug and
published-time publishers, `autoware_utils_tf`'s transform listener) are the shim's,
in `../../shim/include/`, on hypha endpoints and `hypha::TransformBuffer`.
