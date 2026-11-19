# geometry2 tf2 LinearMath (vendored)

From https://github.com/ros/geometry2, branch `noetic-devel`, commit `8fef92a6b56a26e2b5be58d4d5c84dc4de5ee696` (8fef92a):
only `tf2/include/tf2/LinearMath/`, the header-only math that `mrs_lib::AttitudeConverter`
and the estimation manager use (`tf2::Quaternion`, `Matrix3x3`, `Vector3`, `Transform`).
Licence: `LICENSE` in this directory (the Bullet zlib licence each header carries; derived
from the Bullet Physics library).

The files are unchanged. Not vendored: the TF buffer (`tf2_ros`), which in the port is
hypha's (`hypha::Transforms` under `mrs_lib::Transformer`), and `tf2_geometry_msgs`,
whose noetic header needs KDL; the shim's `tf2_geometry_msgs/tf2_geometry_msgs.h` carries
the conversions the port uses.
