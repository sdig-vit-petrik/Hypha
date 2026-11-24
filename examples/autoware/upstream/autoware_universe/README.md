# autoware_universe (vendored)

From https://github.com/autowarefoundation/autoware_universe, tag `0.53.2`, commit
`62a5739d4d5a34f27f6aa15702c8ff59fa4d862c` (62a5739), the version that Autoware 2.1.0
pins in `repositories/autoware.repos`.
Licence: `LICENSE` in this directory (Apache 2.0), with its `NOTICE`, which a copy must
carry.

Only the files the hypha port compiles are copied, at their upstream paths:

- `sensing/autoware_pointcloud_preprocessor`: the concatenate-and-time-sync node, that
  is `include/autoware/pointcloud_preprocessor/concatenate_data/` and
  `src/concatenate_data/` without the older `concatenate_pointclouds` node, the
  `utility/memory` layout checks and `diagnostics/format_utils.hpp` they use, three
  unit tests (`test_concatenation_info.cpp`, `test_concatenation_diagnostics.cpp`,
  `test_concatenate_node_unit.cpp`), `config/concatenate_and_time_sync_node.param.yaml`,
  `docs/concatenate-data.md` and `package.xml`.

- `perception/autoware_euclidean_cluster`: the voxel-grid based clustering of the
  rule-based detection, which keeps the cluster points: `include/` without the label-based
  variant, `lib/` (`confusable_cluster_merger`, `utils`, `voxel_grid_based_euclidean_cluster`),
  its node in `src/`, its unit test, `config/`, `package.xml`, `README.md`.
- `perception/autoware_shape_estimation`: `include/` and `lib/` without the TensorRT
  estimator (upstream's CMake leaves it out without CUDA), the node in `src/`, the unit
  tests `test_shape_estimator.cpp` and `test_corrector_utils.cpp` (the node test needs
  rclcpp executors), `config/`, `package.xml`, `README.md`.
- `perception/autoware_detected_object_feature_remover`: `include/`, `lib/`, `src/`, its
  unit test, `package.xml`, `README.md`.
- `perception/autoware_multi_object_tracker`: `include/`, `lib/`, `src/`, `config/`, and the
  unit tests `test_vehicle_tracker.cpp` and `test_uuid_generator.cpp`; the others read
  rosbag2 bags through their bench helpers, which are not ported.

The files are unchanged. The edits the port needs are in `../patches/`, one file per
package; the build applies them to a copy in the build directory.
