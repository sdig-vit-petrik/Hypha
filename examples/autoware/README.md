# Autoware on hypha

Upstream Autoware nodes, built from their own sources on an rclcpp shim over the hypha
C++ SDK. This is the port of the AutoWare verification milestone: scope in #44, the port
in #47. Autoware 2.1.0: `autoware_core` 1.10.0, `autoware_universe` 0.53.2.

| Slice | Pipeline | State |
|---|---|---|
| A, detection (Leo Drive Isuzu bus) | crop box → concatenate → ground filter → voxel downsample → euclidean cluster | `autoware_crop_box_filter`, the voxel grid filter of `autoware_downsample_filters`, `autoware_ground_filter`, both nodes of `autoware_euclidean_cluster_object_detector` and the concatenate-and-time-sync node of `autoware_pointcloud_preprocessor` ported; every node on synthetic inputs, replay open |
| B, localization (Istanbul Open Dataset) | crop box → downsample → NDT → gyro odometer → EKF | `autoware_ekf_localizer` and `autoware_gyro_odometer` ported, on synthetic inputs; the rest open. For the Isuzu bags, `autoware_gnss_poser` and `autoware_map_projection_loader` ported too, with `autoware_geography_utils` on Lanelet2's projection and map reading; on synthetic inputs. On the Isuzu bags, which have no map, GNSS and the Applanix INS feed the EKF in place of NDT (#110) |

Each node first computes on synthetic inputs in tests; replay of the recorded data
follows #46.

## How it is built

- `upstream/<repository>/`: vendored sources, unchanged, with their `LICENSE` and the
  commit they come from (`README.md` in each).
- `upstream/patches/<package>.patch`: the whole edit of the port to one package. The
  build copies the package into the build directory and applies its patch
  (`patch -p1`); every edit is also marked `// hypha port:` with its reason. A patch
  holds what the shim does not cover. The patches together are the measured gap between
  Autoware and the rclcpp subset (#56).
- `shim/`: the rclcpp subset the nodes use, on the public C++ SDK. It is the seed of
  `hypha_rclcpp` (#59) and moves into its own package once #56 decides where it lives.
- `msg/`: the messages the nodes publish that hypha does not ship, in ROS 1 syntax
  until hypha reads ROS 2 definitions (#45). `pcl_msgs` is vendored as its ROS 1
  definitions (`upstream/pcl_msgs/`); the `autoware_perception_msgs` the clustering
  publishes and the `autoware_sensing_msgs` of the concatenate node are ROS 2
  definitions that are valid ROS 1 syntax, vendored unchanged (`upstream/autoware_msgs/`).
- `config/`: one TOML file per process, holding exactly the keys the node declares.
  A missing key or an unread one is refused with exit 78.

## The shim

| rclcpp | hypha |
|---|---|
| `get_parameter(name)` | the value a `declare_parameter` call read; an undeclared name is refused |
| `get_namespace()`, `get_fully_qualified_name()` | `/` and `/<name>`: every node is in the root namespace |
| `rclcpp::Node` | one `hypha::Node`; several share the process session and its one parameter file. A CamelCase name becomes snake_case for hypha (`VoxelGridDownsampleFilter` is `voxel_grid_downsample_filter`); `get_name()` keeps the ROS name |
| `declare_parameter(name)`, `declare_parameter(name, default)` | `hypha::config().required` / `optional`, read once at start-up |
| `add_on_set_parameters_callback` | kept and never called: parameters do not change while the node runs |
| `create_publisher`, `create_subscription` on a relative name | a port: `output` is `[ports.outputs.output]`, `~/crop_box_polygon` is `[ports.outputs.crop_box_polygon]`, `a/b` is `[ports.outputs."a.b"]` |
| the same on an absolute name (`/tf`) | a literal topic |
| QoS depth | must fit the hypha policy of the message type (`hypha::QosOf<M>`, 32 for `default`); a larger or zero depth is refused. `TwistWithCovarianceStamped` and `Odometry` have the port's `twist` policy, depth 100 (`shim/include/hypha_rclcpp/qos_policies.hpp`), which their include paths include |
| transient local | a latched publisher |
| reliable, best effort | both served by the shared-memory queue, which keeps the newest `maxDepth` samples |
| `publish` failure | logged with its status, counted in `failed_publish_count()` |
| `get_subscription_count` | `subscriberCount()`; a count the transport cannot give reads as 1 |
| `now()`, `get_clock()` | framework time, simulated under `HYPHA_USE_SIM_TIME` |
| `RCLCPP_*`, `RCLCPP_*_STREAM`, `RCLCPP_*_THROTTLE`, `RCLCPP_*_STREAM_THROTTLE` | `HYPHA_LOG_*` with the logger name in front; throttled in host time |
| `create_service`, `create_client` | two topics of `hypha_rclcpp_msgs/ServiceCall`, `<name>/request` and `<name>/response` (hypha has no RPC, [cpp-api.md](../../docs/cpp-api.md) §8): a request carries its client's identity and a sequence number, the server answers in its callback on the spin thread, and a client takes only its own responses. A name resolves as ROS resolves it in the root namespace: absolute as it is, `~/x` as `/<node>/x`, a relative `x` as the parameter `services.x` when the file sets it, else `/x`. A request that does not decode is answered unhandled, and the client's future holds an exception |
| `Client::async_send_request` (with or without a callback), `service_is_ready`, `prune_pending_requests` | the future is set on the spin thread when the response arrives; ready means a server reads the request topic. `wait_for_service` and a blocking call are not offered |
| `create_callback_group`, `CallbackGroup` | kept, without effect: every callback runs on the one spin thread |
| `QoS::reliability`, `QoS::durability` with rmw's kinds (`rmw/qos_profiles.h`) | reliable or best effort, volatile or transient local; a kind that leaves the choice to the middleware is refused |
| `rclcpp::spin(node)` | `node.run()` on the calling thread: a single-threaded executor |
| `rclcpp::spin_some(node)` | `node.runFor()` with the shortest limit: one dispatch turn, the due timers and the queued samples, never a wait; a failed run throws |
| `rcl_clock_type_t`, `RCL_ROS_TIME` | rcl's enum at global scope; every kind means framework time |
| `rclcpp::create_timer(node, clock, period, callback)`, `TimerBase::cancel`/`reset`/`is_canceled`/`time_until_trigger` | a `hypha::Timer` of the node on framework time, whatever the clock passed; created and controlled from a callback too (#90), refused inside a callback on a simulated clock the node does not own; a refused call throws. `create_wall_timer` is not offered |
| `RCLCPP_COMPONENTS_REGISTER_NODE` | registers the class with the component runner `shim/src/component_main.cpp`: one program per component, argument the TOML file, the exit codes of `hypha/application_node.h` |
| `sensor_msgs::msg::PointCloud2` and the other ROS 2 message paths | hypha's generated ROS 1 messages under a `msg` namespace name (`shim/include/hypha_rclcpp/message_namespace.hpp`); time stamps compare with `==` |
| `tl::expected`, `rcpputils`' copy of it | `std::expected` |
| `fmt::format` with a literal format | `std::format` |
| `PointCloud2_<Allocator>`, `PointField_<Allocator>` | the message itself: hypha messages use the standard allocator |
| `autoware_vehicle_info_utils` | the ten vehicle parameters, required, and upstream's derived values and checks; the footprint functions (Boost.Geometry) are not offered |
| `autoware_utils_geometry`: `calc_distance2d/3d`, `get_rpy`, `create_quaternion_from_rpy/yaw`, `create_point`, `pose2transform` | upstream's code on the vendored tf2 types; the distances for any point with `x`, `y`, `z`. `msg/covariance.hpp` is upstream's |
| `tf2::Quaternion`, `Vector3`, `Matrix3x3`, `Transform`, `tf2::Stamped`, `tf2::TimePointZero`, the tf2 exceptions | geometry2's headers, vendored (`upstream/geometry2`) |
| `tf2::getYaw`, `tf2::getEulerYPR`, `tf2::toMsg`/`fromMsg` of a quaternion, vector, transform or pose, `tf2::doTransform` of a `Vector3` or `Vector3Stamped` (KDL's rotation of a quaternion) | tf2's formulas in the shim (`shim/include/tf2/utils.hpp`, `tf2_geometry_msgs/`); upstream's headers need tf2's conversion machinery and KDL |
| `tf2::BufferCore`, `tf2_ros::Buffer` (a BufferCore with a clock), `tf2_ros::TransformListener` | `hypha::TransformBuffer` on `/tf` and `/tf_static`; `lookupTransform` at `TimePointZero` is the latest common time, never waits, and throws the tf2 exception of the failure's kind |
| `tf2_ros::TransformBroadcaster` | a `tf2_msgs/TFMessage` publisher on `/tf` with the `tf` policy |
| `autoware_agnocast_wrapper` | its build without Agnocast: upstream's `macros.hpp`; `Node`, `Service`, `Client`, `create_timer` and the tf2 types are the shim's |
| `autoware_utils_logging::LoggerLevelConfigure` | the `~/config_logger` service, which refuses every request: hypha sets log levels from `[log]` in the parameter file only |
| `managed_transform_buffer::ManagedTransformBuffer` | `hypha::TransformBuffer`; `getTransform` never waits (#91) and ignores its timeout. A static transform holds for all time; one not in the buffer yet is an empty result with a throttled warning, and the concatenate node drops that source and reports it, where upstream waits up to the timeout |
| `autoware_utils_tf::TransformListenerT`, `TransformListener` | `hypha::TransformBuffer`, whichever buffer and listener types the template names; before spin, a lookup with a timeout runs the process's callbacks in 10 ms slices until the transform arrives or the timeout passes |
| `autoware_utils_debug::DebugPublisher` | optional ports `<node>.debug.<name>`: wired in the TOML file or disabled |
| `autoware_utils_debug::PublishedTimePublisher` | `<output topic>/debug/published_time`, sent while it has a subscriber |
| `autoware_utils_diagnostics::DiagnosticsInterface` | upstream's, unchanged, on a shim publisher of the literal `/diagnostics`; the topic has hypha's `default` policy, at most 8 publishers |

A generated message carries the members ROS generated C++ gives: `Ptr`/`ConstPtr`,
`SharedPtr`/`ConstSharedPtr`/`UniquePtr`, `_<field>_type` and `set__<field>()`
([cpp-api.md](../../docs/cpp-api.md) §1.1). Until #45 it keeps its ROS 1 fields
(`header.seq`, `stamp.secs`, `stamp.nsecs`); code that names the ROS 2 fields is patched.

## How to run

Against an installed SDK, with Eigen 3, GoogleTest, PCL 1.12 or newer (Ubuntu's
`libpcl-dev`), GeographicLib, pugixml, yaml-cpp and Boost's filesystem and serialization
libraries (`libgeographiclib-dev`, `libpugixml-dev`, `libyaml-cpp-dev`,
`libboost-filesystem-dev`, `libboost-serialization-dev`); `docker/Dockerfile` installs them:

```bash
./build.sh && cmake --install build --prefix /tmp/hypha-sdk
cmake -S examples -B build/examples -DCMAKE_PREFIX_PATH=/tmp/hypha-sdk && cmake --build build/examples --parallel
./clean-shm.sh --force && ctest --test-dir build/examples/autoware --output-on-failure
build/examples/autoware/autoware_crop_box_filter examples/autoware/config/crop_box_filter.toml
```

`examples/check.sh` runs these tests; `./test.sh cpp` runs it against the installed SDK.

On macOS the port builds natively with Apple clang and Homebrew's `pcl`, `googletest`,
`eigen`, `geographiclib`, `pugixml` and `yaml-cpp` (`brew install pcl googletest eigen
geographiclib pugixml yaml-cpp`; Homebrew's `boost`, which `pcl` brings, holds the compiled
Boost libraries); CMake finds them through `PATH`.
`examples/autoware` also configures on its own: `cmake -S examples/autoware -B
build/autoware -DCMAKE_PREFIX_PATH=/tmp/hypha-sdk`. The ground filter's `grid.hpp` uses
glibc's float constants `M_PIf`, `M_PI_2f` and `M_PI_4f`; where `<math.h>` lacks them, the
CMake file defines them with glibc's values, so the patches stay the same on both systems.

## Tests

| Test | What it checks |
|---|---|
| `autoware_crop_box_filter_upstream_test` | the upstream unit test (`test/test_crop_box_filter.cpp`): validation, cropping inside and outside the box, the pre- and post-transforms, the polygon |
| `autoware_crop_box_filter_node_test` | the upstream node on the bus: parameters from TOML, the lidar-to-base_link transform from `/tf_static` before spin, NaN points skipped, the cropped cloud with its other fields |
| `autoware_voxel_grid_downsample_filter_upstream_test` | the upstream unit test: invalid buffers and layouts refused, one centroid per voxel, separate voxels, non-finite points, the overflow fallback |
| `autoware_voxel_grid_downsample_filter_node_test` | the upstream node on the bus: parameters from TOML, an XYZIRC cloud in, one centroid per voxel with its mean intensity out |
| `autoware_ground_filter_upstream_test` | the upstream unit test (14 cases): the grid and radial modes, the centroids, the slope classification, the parameter ranges, the point layouts |
| `autoware_ground_filter_sanity_check_upstream_test` | the upstream layout check (5 cases): XYZI, XYZIRC, XYZIRADRT, XYZIRCAEDT, an empty cloud |
| `autoware_ground_filter_node_test` | the upstream node on the bus: its parameters and the vehicle's from TOML, a flat ground with a pole in, every pole point and no ground point out |
| `autoware_euclidean_cluster_upstream_test` | the upstream unit test (4 cases): clusters with and without height, the size limits |
| `autoware_voxel_grid_based_euclidean_cluster_upstream_test` | the upstream unit test (9 cases): the size limits, separate groups, the mean position, points on voxel boundaries, an empty cloud |
| `autoware_ros_conversions_upstream_test` | the upstream unit test (4 cases): centroids, the objects message, the coloured debug cloud |
| `autoware_euclidean_cluster_node_test` | the upstream EuclideanClusterNode on the bus: two clusters as objects at their centroids, a cluster above `max_cluster_size` and lone points dropped, the debug cloud, an OK diagnostic |
| `autoware_voxel_grid_based_euclidean_cluster_node_test` | the upstream VoxelGridBasedEuclideanClusterNode on the bus: the same scene, and a WARN diagnostic that names the skipped cluster |
| `autoware_concatenation_info_upstream_test` | the upstream unit test (17 cases): the concatenation info message, source segments, strategies, the advanced configuration bytes |
| `autoware_concatenation_diagnostics_upstream_test` | the upstream unit test (11 cases): the diagnostic status of a concatenation: all topics, a missing topic or transform, an empty cloud, a late cloud, the advanced reference window |
| `autoware_concatenate_node_unit_upstream_test` | the upstream node unit test (9 cases): the twist and odometry queues, motion compensation, the collector's info, the concatenation with non-finite points, a cloud that times out, three clouds at once; a node per case, its parameters from a TOML file |
| `autoware_concatenate_and_time_sync_node_test` | the upstream node on the bus: two lidars' mounts from `/tf_static`, the vehicle's twist on the deeper `twist` policy, two clouds into one in `base_link` with the newer cloud motion-compensated 0.5 m ahead, the info and the synchronized clouds, an OK diagnostic; and a missing lidar after `timeout_sec`, through the collector's timer created in a callback (#90): its source `TIMEOUT` and an ERROR diagnostic |
| `autoware_kalman_filter_upstream_test` | the upstream unit tests of `autoware_kalman_filter`: the Kalman filter and the time-delay Kalman filter |
| `autoware_covariance_ellipse_upstream_test` | the upstream unit test of `autoware_localization_util`'s covariance ellipse |
| `autoware_ekf_localizer_utils_upstream_test` | the upstream unit tests of the EKF's utilities: the aged queue, covariance, Mahalanobis distance, measurement, numeric checks, state transition, strings, warning messages |
| `autoware_ekf_localizer_upstream_test` | the upstream unit test of the EKF itself: the delay table, the 1-D filters, pose and twist delay compensation |
| `autoware_ekf_localizer_node_test` | the upstream EKFLocalizerNode on the bus, on simulated time the test drives, with the deployment configuration: no estimate before the `trigger_node` service call and an initial pose, then odometry and `map → base_link` at that pose; WARN after 50 ticks without a pose, ERROR after 100; poses and twist at 1 m/s move the estimate (upstream's two integration scenarios and one of fusion) |
| `autoware_gyro_odometer_upstream_test` | the upstream unit tests of `autoware_gyro_odometer`: the fusion of a vehicle twist and the IMU, and its diagnostics decision |
| `autoware_gyro_odometer_node_test` | the upstream GyroOdometerNode on the bus, on simulated time the test drives, with the deployment topics: a completed pair on all four twist topics with its covariances, an IMU frame that cannot be resolved (no output, ERROR), an IMU mounted a quarter turn rotated into `base_link`, OK after a fusion (upstream's node scenarios) |
| `autoware_geography_utils_upstream_test` | the upstream unit tests of `autoware_geography_utils` (21 cases): the MGRS, local Cartesian UTM, local Cartesian and transverse Mercator projectors forward and back, on the vendored Lanelet2 and extension projectors, and the vertical datums; the two cases that need the EGM2008 geoid dataset skip without it, as upstream's do |
| `autoware_gnss_poser_node_test` | the upstream GNSSPoser on the bus, with the deployment configuration: a fixed fix projected about a local Cartesian map origin (x and y as GeographicLib's, z the altitude above the origin's), with the INS heading, the fix's variances and the INS errors squared, and `map → gnss_base_link`; a fix without a fix flagged and dropped; an antenna mounted 1 m ahead of `base_link` (upstream's node scenarios, with exact values) |
| `autoware_map_projection_loader_yaml_upstream_test` | the upstream unit test: the MGRS, local, local Cartesian UTM and transverse Mercator projections read from YAML, and the files it refuses |
| `autoware_map_projection_loader_lanelet2_upstream_test` | the upstream unit test: the MGRS grid and the local projection read from a Lanelet2 map, through Lanelet2's OSM reader and Autoware's OSM parser |
| `autoware_map_projection_loader_node_test` | the upstream MapProjectionLoader on the bus: the projection read from upstream's local Cartesian UTM file, latched on `/map/map_projector_info`, received by a subscriber created after the node (upstream's node tests are Python launch tests) |
| `autoware_rclcpp_shim_test` | the shim's names, time conversion, QoS check, timers and services: a call through a callback, two clients that each take only their own response, a request that does not decode, the service names |

Upstream integration tests spin several nodes on an executor thread, with tf2_ros
broadcasters and parameter overrides, which the shim does not offer; they are not built.
The crop box's (`test_crop_box_filter_integration.cpp`) is vendored as the reference case
for #59, and so are the EKF's (`test_ekf_localizer_node_integration.cpp`), the gyro
odometer's (`test_gyro_odometer_node.cpp`) and the GNSS poser's two
(`test_gnss_poser_node.cpp`, `test_gnss_poser_node_characterization.cpp`), whose
scenarios the node tests run on the bus; the others, the euclidean cluster's `test_nodes.cpp` and the concatenate node's
`test_concatenate_node_characterization.cpp` among them, are not copied.

The two cluster nodes differ on a cluster above `max_cluster_size`, as upstream does:
PCL's extraction drops it before EuclideanClusterNode counts it, so that node reports
OK, while VoxelGridBasedEuclideanClusterNode counts it and reports WARN. The node tests
check both.

## The gap so far

What the patches change, per package. It is the input to #56 and, for messages, #45/#57.

| Package | Edit | Cause |
|---|---|---|
| `autoware_crop_box_filter` | `stamp.sec`/`nanosec` → `secs`/`nsecs` (node and test) | ROS 1 time fields (#45) |
| `autoware_downsample_filters` | PCL includes and `PCL_MAKE_ALIGNED_OPERATOR_NEW` → Eigen | the voxel grid filter uses only Eigen; the port does not depend on PCL for it |
| `autoware_ground_filter` | `message_filters` removed; `use_indices = true` refused at start-up | the input/indices synchronizer is `message_filters`, which the shim does not offer (#56) |
| `autoware_ground_filter` | `abs()` → `std::abs()` on the eight float heights of `ground_filter.cpp` | an upstream bug: with libstdc++ and PCL 1.15 the unqualified call is C's `int abs(int)`, which truncates the heights; the ground filter node test fails without it ([report](../../docs/autoware-ground-filter-abs.md)) |
| `autoware_euclidean_cluster_object_detector` | `stamp.sec`/`nanosec` → `secs`/`nsecs` (two unit tests) | ROS 1 time fields (#45) |
| `autoware_pointcloud_preprocessor` | `sec`/`nanosec` → `secs`/`nsecs`: 66 uses in `time_utils.hpp`, `combine_cloud_handler`, `test_concatenation_info.cpp` | ROS 1 time fields (#45) |
| `autoware_pointcloud_preprocessor` | the twist and odometry queues drop a twist by its age, `stamp - front < 1 s`, in place of comparing with `stamp - 1 s` | a hypha stamp is unsigned (ROS 1 `time`), so `stamp - 1 s` does not fit below 1 s, as under simulated time from 0; the test is the same |
| `autoware_pointcloud_preprocessor` | five unused `message_filters` includes removed; `test_concatenate_node_unit.cpp` reads its parameters from a TOML file, not `parameter_overrides` | the shim offers neither; its parameters come from the parameter file |
| `autoware_ekf_localizer` | `stamp.sec`/`nanosec` → `secs`/`nsecs` in `ekf_localizer.cpp` | ROS 1 time fields (#45) |
| `autoware_gyro_odometer` | `stamp.sec`/`nanosec` → `secs`/`nsecs` in the unit test | ROS 1 time fields (#45) |
| `autoware_gnss_poser` | `stamp.sec`/`nanosec` → `secs`/`nsecs` in the static transform lookup | ROS 1 time fields (#45) |
| `autoware_gnss_poser` | `#include <numeric>` added | an upstream omission: `std::reduce` comes through other headers only with libstdc++ |
| `autoware_localization_util` | `stamp.sec`/`nanosec` → `secs`/`nsecs` in the covariance ellipse test | ROS 1 time fields (#45) |
| `pcl_conversions` | two unused `message_filters` includes removed | as above |
| `point_cloud_msg_wrapper` | `#include <algorithm>` added | an upstream omission |

The `ConstSharedPtr` edits of the first two packages went away with the generator's ROS 2
typedefs (#57).

The random downsample filter of the same package uses PCL (`pcl::RandomSample`) and is
not in the pipeline; it is not vendored.
