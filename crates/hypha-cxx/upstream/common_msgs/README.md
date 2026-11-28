# common_msgs sensor_msgs companion headers (vendored)

From https://github.com/ros/common_msgs, tag `1.13.2` (the last ROS 1 Noetic release of
`sensor_msgs`), commit `1230f39a7068d1d73d1039eb0eb970c922b6bcf7`:

- `sensor_msgs/include/sensor_msgs/point_cloud2_iterator.h` and its
  `impl/point_cloud2_iterator.h`: `PointCloud2Modifier`, `PointCloud2Iterator`,
  `PointCloud2ConstIterator`;
- `point_field_conversion.h`: `readPointCloud2BufferValue`, `writePointCloud2BufferValue` and
  the datatype to type maps;
- `image_encodings.h`, `distortion_models.h`: the constants and small functions every ROS 1
  camera node includes.

They are the header-only files that ROS 1 installs beside the `sensor_msgs` messages. The
SDK installs them into the include directory of `sensor_msgs` (`crates/hypha-cxx/CMakeLists.txt`),
beside the headers generated from `crates/hypha_msgs/msg/sensor_msgs`, and
`crates/hypha-cxx/include/sensor_msgs/*.hpp` are their ROS 2 names. Issues #242 and #265.

Licence: BSD 3-Clause, in the header of each file (Willow Garage, Inc.; Open Source
Robotics Foundation, Inc.; listed in `NOTICE`). Upstream has no separate licence file.

The files are unchanged but for one edit, marked `hypha port:`: `point_cloud2_iterator.h` and
its `impl/` take a `hypha::View<sensor_msgs::PointCloud2>` too, a received cloud read in place
(`hypha/view.h`, #255), in a constructor of `PointCloud2ConstIterator` and its base, and
`set_field` reads the fields of either. A fresh `port.py vendor` drops the edit: apply it again.
`vendor.toml` holds their SHA-256, and `python3 -I
examples/autoware/tools/port.py verify` (run by `./check.sh`) checks them. To fetch them
again: `python3 -I examples/autoware/tools/port.py vendor common_msgs`.

Not vendored: `fill_image.h` (no port uses it) and `point_cloud_conversion.h` (it needs
`sensor_msgs/PointCloud`, which the SDK does not ship).
