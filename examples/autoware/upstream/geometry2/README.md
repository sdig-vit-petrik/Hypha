# geometry2 tf2 (vendored)

From https://github.com/ros2/geometry2, branch `jazzy`, commit
`f702874b1c8535d6a038230ab2cda0ba5d521ebd` (f702874): the header-only parts of `tf2`
the EKF uses, at their upstream paths under `tf2/include/tf2/`:

- `LinearMath/`: `Scalar.hpp`, `MinMax.hpp`, `QuadWord.hpp`, `Vector3.hpp`,
  `Quaternion.hpp`, `Matrix3x3.hpp`, from Bullet;
- `time.hpp` (`tf2::TimePoint`, `TimePointZero`; the functions it declares are not
  built), `exceptions.hpp` (`tf2::TransformException` and its kinds), and
  `visibility_control.h`, which they include.

Licence: `LICENSE` in this directory (BSD 3-Clause, the repository's). The `LinearMath`
files carry Bullet's zlib licence in their own header comment.

The files are unchanged. `tf2/utils.hpp`, the buffer and the broadcaster are the shim's
(`../../shim/include/tf2/`, `../../shim/include/tf2_ros/`): upstream's `utils.hpp` needs
`tf2_geometry_msgs` and KDL, and the buffer core is replaced by `hypha::TransformBuffer`.
