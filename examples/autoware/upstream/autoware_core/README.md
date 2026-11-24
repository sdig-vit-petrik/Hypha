# autoware_core (vendored)

From https://github.com/autowarefoundation/autoware_core, tag `1.10.0`, commit
`384bcd47e6d2fd21e5954a915153aa5303b34a53` (384bcd4), the version that Autoware 2.1.0
pins in `repositories/autoware.repos`.
Licence: `LICENSE` in this directory (Apache 2.0, the Autoware Foundation and TIER IV), with
its `NOTICE`, which a copy must carry.

Only the packages the hypha port compiles are copied, at their upstream paths:

- `sensing/autoware_crop_box_filter`: `src/`, `test/`, `config/`, `package.xml`, `README.md`.
- `sensing/autoware_downsample_filters`: the voxel grid filter only:
  `src/voxel_grid_downsample_filter/`, its unit test, its `config/` file, `package.xml`,
  `README.md`.
- `perception/autoware_ground_filter`: `src/`, its two unit tests (`test_ground_filter.cpp`,
  `test_sanity_check.cpp`), `config/ground_filter.param.yaml`, `package.xml`, `README.md`.
- `perception/autoware_euclidean_cluster_object_detector`: `src/`, `lib/`, its three unit
  tests (`test_euclidean_cluster.cpp`, `test_voxel_grid_based_euclidean_cluster.cpp`,
  `test_ros_conversions.cpp`), `config/`, `package.xml`, `README.md`.
- `common/autoware_point_types`: `include/autoware/point_types/types.hpp`.
- `common/autoware_agnocast_wrapper`: `include/autoware/agnocast_wrapper/macros.hpp` and
  `package.xml`; the rest of the wrapper, as it builds without Agnocast, is the shim's
  (`../../shim/include/autoware/agnocast_wrapper/`).
- `common/autoware_geography_utils`: `include/`, `src/`, `test/`, `package.xml`, `README.md`;
  its projectors are Lanelet2's and `autoware_lanelet2_extension`'s (`../lanelet2/`,
  `../autoware_lanelet2_extension/`).
- `common/autoware_component_interface_specs`: `include/autoware/component_interface_specs/`
  `map.hpp`, `utils.hpp`, `version.hpp`, the map interface; `package.xml`, `README.md`.
- `map/autoware_map_projection_loader`: `include/`, `src/`, `test/` (its C++ tests and their
  data; the Python launch tests are copied, not run), `config/`, `package.xml`, `README.md`.
- `sensing/autoware_gnss_poser`: `src/`, `test/`, `config/`, `package.xml`, `README.md`.
- `common/autoware_kalman_filter`: `include/`, `src/`, `test/`, `package.xml`, `README.md`.
- `localization/autoware_localization_util`: the covariance ellipse the EKF uses
  (`covariance_ellipse.hpp`, `.cpp` and its unit test), `package.xml`, `README.md`.
- `localization/autoware_gyro_odometer`: `src/`, `test/`, `config/`, `package.xml`,
  `README.md`.
- `localization/autoware_ekf_localizer`: `src/`, `test/` (its C++ tests; the launch test
  `test/utils/test_ekf_localizer_launch.py` is not copied), `config/`, `package.xml`,
  `README.md`.

The files are unchanged. The edits the port needs are in `../patches/`, one file per
package; the build applies them to a copy in the build directory.
