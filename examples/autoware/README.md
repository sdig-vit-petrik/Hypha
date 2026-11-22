# Autoware on hypha

Upstream Autoware nodes, built from their own sources on an rclcpp shim over the hypha
C++ SDK. This is the port of the AutoWare verification milestone: scope in #44, the port
in #47. Autoware 2.1.0: `autoware_core` 1.10.0, `autoware_universe` 0.53.2.

| Slice | Pipeline | State |
|---|---|---|
| A, detection (Leo Drive Isuzu bus) | crop box → concatenate → ground filter → voxel downsample → euclidean cluster | `autoware_crop_box_filter` and the voxel grid filter of `autoware_downsample_filters` ported |
| B, localization (Istanbul Open Dataset) | crop box → downsample → NDT → gyro odometer → EKF | not started |

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
  until hypha reads ROS 2 definitions (#45).
- `config/`: one TOML file per process, holding exactly the keys the node declares.
  A missing key or an unread one is refused with exit 78.

## The shim

| rclcpp | hypha |
|---|---|
| `rclcpp::Node` | one `hypha::Node`; several share the process session and its one parameter file. A CamelCase name becomes snake_case for hypha (`VoxelGridDownsampleFilter` is `voxel_grid_downsample_filter`); `get_name()` keeps the ROS name |
| `declare_parameter(name)`, `declare_parameter(name, default)` | `hypha::config().required` / `optional`, read once at start-up |
| `add_on_set_parameters_callback` | kept and never called: parameters do not change while the node runs |
| `create_publisher`, `create_subscription` on a relative name | a port: `output` is `[ports.outputs.output]`, `~/crop_box_polygon` is `[ports.outputs.crop_box_polygon]`, `a/b` is `[ports.outputs."a.b"]` |
| the same on an absolute name (`/tf`) | a literal topic |
| QoS depth | must fit the hypha policy of the message type (`hypha::QosOf<M>`, 32 for `default`); a larger or zero depth is refused |
| transient local | a latched publisher |
| reliable, best effort | both served by the shared-memory queue, which keeps the newest `maxDepth` samples |
| `publish` failure | logged with its status, counted in `failed_publish_count()` |
| `get_subscription_count` | `subscriberCount()`; a count the transport cannot give reads as 1 |
| `now()`, `get_clock()` | framework time, simulated under `HYPHA_USE_SIM_TIME` |
| `RCLCPP_*`, `RCLCPP_*_THROTTLE` | `HYPHA_LOG_*` with the logger name in front; throttled in host time |
| `rclcpp::spin(node)` | `node.run()` on the calling thread: a single-threaded executor |
| `RCLCPP_COMPONENTS_REGISTER_NODE` | registers the class with the component runner `shim/src/component_main.cpp`: one program per component, argument the TOML file, the exit codes of `hypha/application_node.h` |
| `sensor_msgs::msg::PointCloud2` and the other ROS 2 message paths | hypha's generated ROS 1 messages under a `msg` namespace name (`shim/include/hypha_rclcpp/message_namespace.hpp`); time stamps compare with `==` |
| `tl::expected` | `std::expected` |
| `autoware_utils_tf::TransformListener` | `hypha::TransformBuffer`; before spin, a lookup with a timeout runs the process's callbacks in 10 ms slices until the transform arrives or the timeout passes |
| `autoware_utils_debug::DebugPublisher` | optional ports `<node>.debug.<name>`: wired in the TOML file or disabled |
| `autoware_utils_debug::PublishedTimePublisher` | `<output topic>/debug/published_time`, sent while it has a subscriber |

Until #45 and #57, a message keeps its ROS 1 fields (`header.seq`, `stamp.secs`,
`stamp.nsecs`) and its `Ptr`/`ConstPtr` typedefs; code that names the ROS 2 fields or
`ConstSharedPtr` is patched.

## How to run

Against an installed SDK, with Eigen 3 and GoogleTest:

```bash
./build.sh && cmake --install build --prefix /tmp/hypha-sdk
cmake -S examples -B build/examples -DCMAKE_PREFIX_PATH=/tmp/hypha-sdk && cmake --build build/examples --parallel
./clean-shm.sh --force && ctest --test-dir build/examples/autoware --output-on-failure
build/examples/autoware/autoware_crop_box_filter examples/autoware/config/crop_box_filter.toml
```

`examples/check.sh` runs these tests; `./test.sh cpp` runs it against the installed SDK.

## Tests

| Test | What it checks |
|---|---|
| `autoware_crop_box_filter_upstream_test` | the upstream unit test (`test/test_crop_box_filter.cpp`): validation, cropping inside and outside the box, the pre- and post-transforms, the polygon |
| `autoware_crop_box_filter_node_test` | the upstream node on the bus: parameters from TOML, the lidar-to-base_link transform from `/tf_static` before spin, NaN points skipped, the cropped cloud with its other fields |
| `autoware_voxel_grid_downsample_filter_upstream_test` | the upstream unit test: invalid buffers and layouts refused, one centroid per voxel, separate voxels, non-finite points, the overflow fallback |
| `autoware_voxel_grid_downsample_filter_node_test` | the upstream node on the bus: parameters from TOML, an XYZIRC cloud in, one centroid per voxel with its mean intensity out |
| `autoware_rclcpp_shim_test` | the shim's names, time conversion and QoS check |

Upstream integration tests spin several nodes on an executor thread, with tf2_ros
broadcasters, which the shim does not offer; they are not built. The crop box's
(`test_crop_box_filter_integration.cpp`) is vendored as the reference case for #59; the
others are not copied.

## The gap so far

What the patches change, per package. It is the input to #56 and, for messages, #45/#57.

| Package | Edit | Cause |
|---|---|---|
| `autoware_crop_box_filter` | `ConstSharedPtr` → `std::shared_ptr<const T>` | hypha messages have `ConstPtr` only (#57) |
| `autoware_crop_box_filter` | `stamp.sec`/`nanosec` → `secs`/`nsecs` (node and test) | ROS 1 time fields (#45) |
| `autoware_downsample_filters` | `ConstSharedPtr` → `std::shared_ptr<const T>` (4 places) | as above (#57) |
| `autoware_downsample_filters` | PCL includes and `PCL_MAKE_ALIGNED_OPERATOR_NEW` → Eigen | the voxel grid filter uses only Eigen; the port does not depend on PCL for it |

The random downsample filter of the same package uses PCL (`pcl::RandomSample`) and is
not in the pipeline; it is not vendored.
