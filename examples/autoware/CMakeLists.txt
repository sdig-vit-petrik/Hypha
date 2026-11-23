# Autoware on hypha: upstream Autoware nodes on an rclcpp shim over the hypha C++ SDK.
# See README.md.
#
# Self-contained: builds against an installed SDK, Eigen 3 and GoogleTest.
#
#   cmake -S . -B build -DCMAKE_PREFIX_PATH=<sdk> && cmake --build build && ctest --test-dir build

cmake_minimum_required(VERSION 3.20)
project(autoware_on_hypha VERSION 0.1.0 LANGUAGES CXX)

if(NOT TARGET hypha::sdk)
  find_package(hypha CONFIG REQUIRED)
endif()
find_package(Eigen3 3.3 REQUIRED NO_MODULE)
find_package(GTest REQUIRED)
# The ground filter's point types and the clustering; Ubuntu's libpcl-dev.
find_package(PCL 1.12 CONFIG REQUIRED COMPONENTS common filters kdtree search segmentation)
find_program(AUTOWARE_PATCH patch REQUIRED)
enable_testing()

set(AUTOWARE_UPSTREAM_DIR "${CMAKE_CURRENT_SOURCE_DIR}/upstream")
set(AUTOWARE_MSG_OUT "${CMAKE_CURRENT_BINARY_DIR}/generated/include")

# Messages the ported nodes publish that hypha does not ship, in ROS 1 syntax until #45.
hypha_generate_messages(PACKAGE autoware_internal_debug_msgs
  MSG_DIR "${CMAKE_CURRENT_SOURCE_DIR}/msg/autoware_internal_debug_msgs/msg"
  OUT_DIR "${AUTOWARE_MSG_OUT}"
  DEPENDS std_msgs)
hypha_generate_messages(PACKAGE pcl_msgs
  MSG_DIR "${AUTOWARE_UPSTREAM_DIR}/pcl_msgs/msg"
  OUT_DIR "${AUTOWARE_MSG_OUT}"
  DEPENDS std_msgs sensor_msgs)
hypha_generate_messages(PACKAGE autoware_perception_msgs
  MSG_DIR "${AUTOWARE_UPSTREAM_DIR}/autoware_msgs/autoware_perception_msgs/msg"
  OUT_DIR "${AUTOWARE_MSG_OUT}"
  DEPENDS std_msgs geometry_msgs)
hypha_generate_messages(PACKAGE autoware_internal_msgs
  MSG_DIR "${CMAKE_CURRENT_SOURCE_DIR}/msg/autoware_internal_msgs/msg"
  OUT_DIR "${AUTOWARE_MSG_OUT}"
  DEPENDS std_msgs)
hypha_generate_messages(PACKAGE autoware_sensing_msgs
  MSG_DIR "${AUTOWARE_UPSTREAM_DIR}/autoware_msgs/autoware_sensing_msgs/msg"
  OUT_DIR "${AUTOWARE_MSG_OUT}"
  DEPENDS std_msgs geometry_msgs)
hypha_generate_messages(PACKAGE geographic_msgs
  MSG_DIR "${AUTOWARE_UPSTREAM_DIR}/geographic_info/geographic_msgs/msg"
  OUT_DIR "${AUTOWARE_MSG_OUT}")
hypha_generate_messages(PACKAGE autoware_map_msgs
  MSG_DIR "${CMAKE_CURRENT_SOURCE_DIR}/msg/autoware_map_msgs/msg"
  SRV_DIR "${CMAKE_CURRENT_SOURCE_DIR}/msg/autoware_map_msgs/srv"
  OUT_DIR "${AUTOWARE_MSG_OUT}"
  DEPENDS geographic_msgs std_msgs sensor_msgs)
# The shim's service envelope, and the services the ported nodes offer.
hypha_generate_messages(PACKAGE hypha_rclcpp_msgs
  MSG_DIR "${CMAKE_CURRENT_SOURCE_DIR}/msg/hypha_rclcpp_msgs/msg"
  OUT_DIR "${AUTOWARE_MSG_OUT}")
# In examples/, the MRS port generates std_srvs first, from the same SetBool definition;
# one target serves both.
if(NOT TARGET std_srvs)
  hypha_generate_messages(PACKAGE std_srvs
    SRV_DIR "${AUTOWARE_UPSTREAM_DIR}/common_interfaces/std_srvs/srv"
    OUT_DIR "${AUTOWARE_MSG_OUT}")
endif()
hypha_generate_messages(PACKAGE logging_demo
  SRV_DIR "${AUTOWARE_UPSTREAM_DIR}/demos/logging_demo/srv"
  OUT_DIR "${AUTOWARE_MSG_OUT}")
hypha_generate_messages(PACKAGE visualization_msgs
  MSG_DIR "${CMAKE_CURRENT_SOURCE_DIR}/msg/visualization_msgs/msg"
  OUT_DIR "${AUTOWARE_MSG_OUT}"
  DEPENDS std_msgs geometry_msgs sensor_msgs)

# autoware_patched(<package> <upstream dir> <out var>): a copy of the upstream package in
# the build directory with upstream/patches/<package>.patch applied. The vendored files
# stay unchanged; the patch is the whole edit of the port.
function(autoware_patched package source_dir out_var)
  set(patched "${CMAKE_CURRENT_BINARY_DIR}/patched/${package}")
  set(patch_file "${AUTOWARE_UPSTREAM_DIR}/patches/${package}.patch")
  file(REMOVE_RECURSE "${patched}")
  file(COPY "${source_dir}/" DESTINATION "${patched}")
  if(EXISTS "${patch_file}")
    execute_process(
      COMMAND "${AUTOWARE_PATCH}" -p1 --forward --batch --quiet -d "${patched}" -i "${patch_file}"
      RESULT_VARIABLE patch_result OUTPUT_VARIABLE patch_output ERROR_VARIABLE patch_output)
    if(NOT patch_result EQUAL 0)
      message(FATAL_ERROR "examples/autoware: ${patch_file} does not apply:\n${patch_output}")
    endif()
  endif()
  file(GLOB_RECURSE upstream_files "${source_dir}/*")
  set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS ${upstream_files} "${patch_file}")
  set(${out_var} "${patched}" PARENT_SCOPE)
endfunction()

# The shim: rclcpp, rclcpp_components, the ROS 2 message paths and the Autoware utilities
# the nodes use, on hypha.
add_library(hypha_rclcpp STATIC shim/src/rclcpp.cpp)
target_include_directories(hypha_rclcpp PUBLIC
  shim/include
  "${AUTOWARE_MSG_OUT}/hypha_msgs"
  "${AUTOWARE_UPSTREAM_DIR}/common_interfaces/sensor_msgs/include"
  "${AUTOWARE_UPSTREAM_DIR}/autoware_utils/autoware_utils_system/include"
  "${AUTOWARE_UPSTREAM_DIR}/autoware_utils/autoware_utils_diagnostics/include"
  "${AUTOWARE_UPSTREAM_DIR}/autoware_utils/autoware_utils_geometry/include"
  "${AUTOWARE_UPSTREAM_DIR}/autoware_core/common/autoware_agnocast_wrapper/include"
  "${AUTOWARE_UPSTREAM_DIR}/geometry2/tf2/include")
target_link_libraries(hypha_rclcpp PUBLIC
  hypha::sdk autoware_internal_debug_msgs autoware_internal_msgs hypha_rclcpp_msgs std_srvs
  logging_demo visualization_msgs Eigen3::Eigen)
target_compile_features(hypha_rclcpp PUBLIC cxx_std_23)

add_library(hypha_rclcpp_component_main OBJECT shim/src/component_main.cpp)
target_link_libraries(hypha_rclcpp_component_main PUBLIC hypha_rclcpp)

# autoware_component(<program> SOURCES <file>...): the program of one registered node.
function(autoware_component program)
  cmake_parse_arguments(ARG "" "" "SOURCES;INCLUDES" ${ARGN})
  add_executable(${program} ${ARG_SOURCES} $<TARGET_OBJECTS:hypha_rclcpp_component_main>)
  target_include_directories(${program} PRIVATE ${ARG_INCLUDES})
  target_link_libraries(${program} PRIVATE hypha_rclcpp)
endfunction()

# autoware_test(<name> SOURCES <file>... [INCLUDES <dir>...] [LIBS <lib>...]
#               [ENV <VAR=value>...]): one GoogleTest program.
# Each test runs in a bus domain of its own, autoware_<n>, so tests that ctest runs in
# parallel share neither node names nor topics. A domain name has at most 32
# characters, so it is numbered, not named after the test.
function(autoware_test name)
  cmake_parse_arguments(ARG "" "" "SOURCES;INCLUDES;LIBS;ENV" ${ARGN})
  add_executable(${name} ${ARG_SOURCES})
  target_include_directories(${name} PRIVATE ${ARG_INCLUDES})
  target_link_libraries(${name} PRIVATE hypha_rclcpp GTest::gtest ${ARG_LIBS})
  add_test(NAME ${name} COMMAND ${name} WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}")
  get_property(count GLOBAL PROPERTY AUTOWARE_TEST_COUNT)
  if(NOT count)
    set(count 0)
  endif()
  math(EXPR count "${count} + 1")
  set_property(GLOBAL PROPERTY AUTOWARE_TEST_COUNT ${count})
  set_tests_properties(${name} PROPERTIES ENVIRONMENT "HYPHA_DOMAIN=autoware_${count};${ARG_ENV}")
endfunction()

# --- Slice A: autoware_crop_box_filter (autoware_core 1.10.0) --------------------------

autoware_patched(autoware_crop_box_filter
  "${AUTOWARE_UPSTREAM_DIR}/autoware_core/sensing/autoware_crop_box_filter" CROP_BOX_DIR)

autoware_component(autoware_crop_box_filter
  SOURCES "${CROP_BOX_DIR}/src/crop_box_filter.cpp" "${CROP_BOX_DIR}/src/crop_box_filter_node.cpp"
  INCLUDES "${CROP_BOX_DIR}/src")

# The upstream unit test, unchanged but for the patch: the filter on synthetic clouds.
autoware_test(autoware_crop_box_filter_upstream_test
  SOURCES "${CROP_BOX_DIR}/test/test_crop_box_filter.cpp" "${CROP_BOX_DIR}/src/crop_box_filter.cpp"
  INCLUDES "${CROP_BOX_DIR}/src"
  LIBS GTest::gtest_main)

# The node on the bus: a cloud in, the cropped cloud out, through hypha topics.
autoware_test(autoware_crop_box_filter_node_test
  SOURCES test/test_crop_box_filter_node.cpp
    "${CROP_BOX_DIR}/src/crop_box_filter.cpp" "${CROP_BOX_DIR}/src/crop_box_filter_node.cpp"
  INCLUDES "${CROP_BOX_DIR}/src")

# The shim itself.
autoware_test(autoware_rclcpp_shim_test SOURCES test/test_rclcpp_shim.cpp LIBS GTest::gtest_main)

# --- Slice A: autoware_downsample_filters, the voxel grid filter (autoware_core 1.10.0) --

autoware_patched(autoware_downsample_filters
  "${AUTOWARE_UPSTREAM_DIR}/autoware_core/sensing/autoware_downsample_filters" DOWNSAMPLE_DIR)
set(VOXEL_GRID_SOURCES
  "${DOWNSAMPLE_DIR}/src/voxel_grid_downsample_filter/voxel_grid_downsample_filter.cpp"
  "${DOWNSAMPLE_DIR}/src/voxel_grid_downsample_filter/faster_voxel_grid_downsample_filter.cpp")

autoware_component(autoware_voxel_grid_downsample_filter
  SOURCES ${VOXEL_GRID_SOURCES}
    "${DOWNSAMPLE_DIR}/src/voxel_grid_downsample_filter/voxel_grid_downsample_filter_node.cpp"
  INCLUDES "${DOWNSAMPLE_DIR}/src")

autoware_test(autoware_voxel_grid_downsample_filter_upstream_test
  SOURCES "${DOWNSAMPLE_DIR}/test/test_voxel_grid_downsample_filter.cpp" ${VOXEL_GRID_SOURCES}
  INCLUDES "${DOWNSAMPLE_DIR}/src"
  LIBS GTest::gtest_main)

autoware_test(autoware_voxel_grid_downsample_filter_node_test
  SOURCES test/test_voxel_grid_downsample_filter_node.cpp ${VOXEL_GRID_SOURCES}
    "${DOWNSAMPLE_DIR}/src/voxel_grid_downsample_filter/voxel_grid_downsample_filter_node.cpp"
  INCLUDES "${DOWNSAMPLE_DIR}/src")

# --- PCL and the Autoware helpers that use it ----------------------------------------------

# pcl_conversions (perception_pcl, jazzy): sensor_msgs <-> PCL, header-only.
autoware_patched(pcl_conversions "${AUTOWARE_UPSTREAM_DIR}/perception_pcl/pcl_conversions"
  PCL_CONVERSIONS_DIR)
add_library(autoware_pcl INTERFACE)
target_include_directories(autoware_pcl SYSTEM INTERFACE ${PCL_INCLUDE_DIRS})
target_include_directories(autoware_pcl INTERFACE "${PCL_CONVERSIONS_DIR}/include")
target_link_libraries(autoware_pcl INTERFACE hypha_rclcpp pcl_msgs ${PCL_LIBRARIES})
target_compile_definitions(autoware_pcl INTERFACE ${PCL_DEFINITIONS})

# autoware_point_types (autoware_core) on point_cloud_msg_wrapper, both header-only.
autoware_patched(point_cloud_msg_wrapper "${AUTOWARE_UPSTREAM_DIR}/point_cloud_msg_wrapper"
  POINT_CLOUD_MSG_WRAPPER_DIR)
add_library(autoware_point_types INTERFACE)
target_include_directories(autoware_point_types INTERFACE
  "${AUTOWARE_UPSTREAM_DIR}/autoware_core/common/autoware_point_types/include"
  "${POINT_CLOUD_MSG_WRAPPER_DIR}/include")
target_link_libraries(autoware_point_types INTERFACE autoware_pcl)

# autoware_utils_math (header-only) and autoware_utils_debug's TimeKeeper.
add_library(autoware_utils_extra STATIC
  "${AUTOWARE_UPSTREAM_DIR}/autoware_utils/autoware_utils_debug/src/time_keeper.cpp")
target_include_directories(autoware_utils_extra PUBLIC
  "${AUTOWARE_UPSTREAM_DIR}/autoware_utils/autoware_utils_math/include"
  "${AUTOWARE_UPSTREAM_DIR}/autoware_utils/autoware_utils_debug/include")
target_link_libraries(autoware_utils_extra PUBLIC hypha_rclcpp)

# --- Slice A: autoware_ground_filter (autoware_core 1.10.0) --------------------------------

autoware_patched(autoware_ground_filter
  "${AUTOWARE_UPSTREAM_DIR}/autoware_core/perception/autoware_ground_filter" GROUND_FILTER_DIR)
add_library(autoware_ground_filter_core STATIC
  "${GROUND_FILTER_DIR}/src/ground_filter.cpp" "${GROUND_FILTER_DIR}/src/sanity_check.cpp")
target_include_directories(autoware_ground_filter_core PUBLIC "${GROUND_FILTER_DIR}/src")
target_link_libraries(autoware_ground_filter_core PUBLIC
  autoware_point_types autoware_utils_extra autoware_pcl)
# grid.hpp uses glibc's float constants of <math.h>; a C library without them (macOS) gets
# their glibc values.
include(CheckSymbolExists)
check_symbol_exists(M_PIf "math.h" AUTOWARE_HAVE_M_PIF)
if(NOT AUTOWARE_HAVE_M_PIF)
  target_compile_definitions(autoware_ground_filter_core PUBLIC
    "M_PIf=3.14159265358979323846f" "M_PI_2f=1.57079632679489661923f"
    "M_PI_4f=0.785398163397448309616f")
endif()

autoware_component(autoware_ground_filter
  SOURCES "${GROUND_FILTER_DIR}/src/ground_filter_node.cpp")
target_link_libraries(autoware_ground_filter PRIVATE autoware_ground_filter_core)

autoware_test(autoware_ground_filter_upstream_test
  SOURCES "${GROUND_FILTER_DIR}/test/test_ground_filter.cpp" LIBS autoware_ground_filter_core)
autoware_test(autoware_ground_filter_sanity_check_upstream_test
  SOURCES "${GROUND_FILTER_DIR}/test/test_sanity_check.cpp"
  LIBS autoware_ground_filter_core GTest::gtest_main)

autoware_test(autoware_ground_filter_node_test
  SOURCES test/test_ground_filter_node.cpp "${GROUND_FILTER_DIR}/src/ground_filter_node.cpp"
  LIBS autoware_ground_filter_core)

# --- Slice A: autoware_euclidean_cluster_object_detector (autoware_core 1.10.0) -----------

autoware_patched(autoware_euclidean_cluster_object_detector
  "${AUTOWARE_UPSTREAM_DIR}/autoware_core/perception/autoware_euclidean_cluster_object_detector"
  EUCLIDEAN_CLUSTER_DIR)
add_library(autoware_euclidean_cluster_core STATIC
  "${EUCLIDEAN_CLUSTER_DIR}/lib/ros_conversions.cpp"
  "${EUCLIDEAN_CLUSTER_DIR}/src/euclidean_cluster_object_detector.cpp")
target_include_directories(autoware_euclidean_cluster_core PUBLIC "${EUCLIDEAN_CLUSTER_DIR}/src")
target_link_libraries(autoware_euclidean_cluster_core PUBLIC
  autoware_point_types autoware_pcl autoware_perception_msgs)

autoware_component(autoware_euclidean_cluster
  SOURCES "${EUCLIDEAN_CLUSTER_DIR}/src/euclidean_cluster_node.cpp")
target_link_libraries(autoware_euclidean_cluster PRIVATE autoware_euclidean_cluster_core)
autoware_component(autoware_voxel_grid_based_euclidean_cluster
  SOURCES "${EUCLIDEAN_CLUSTER_DIR}/src/voxel_grid_based_euclidean_cluster_node.cpp")
target_link_libraries(autoware_voxel_grid_based_euclidean_cluster PRIVATE
  autoware_euclidean_cluster_core)

# The upstream unit tests; each has its own main().
foreach(upstream_test euclidean_cluster voxel_grid_based_euclidean_cluster ros_conversions)
  autoware_test(autoware_${upstream_test}_upstream_test
    SOURCES "${EUCLIDEAN_CLUSTER_DIR}/test/test_${upstream_test}.cpp"
    LIBS autoware_euclidean_cluster_core)
endforeach()

# Each node on the bus, one per process: the nodes of a process share its parameter file.
foreach(node euclidean_cluster voxel_grid_based_euclidean_cluster)
  autoware_test(autoware_${node}_node_test
    SOURCES test/test_${node}_node.cpp "${EUCLIDEAN_CLUSTER_DIR}/src/${node}_node.cpp"
    LIBS autoware_euclidean_cluster_core)
endforeach()

# --- Slice A: concatenate-and-time-sync (autoware_universe 0.53.2) -------------------------

autoware_patched(autoware_pointcloud_preprocessor
  "${AUTOWARE_UPSTREAM_DIR}/autoware_universe/sensing/autoware_pointcloud_preprocessor"
  POINTCLOUD_PREPROCESSOR_DIR)
set(CONCATENATE_DIR "${POINTCLOUD_PREPROCESSOR_DIR}/src/concatenate_data")
add_library(autoware_concatenate_core STATIC
  "${CONCATENATE_DIR}/cloud_collector.cpp"
  "${CONCATENATE_DIR}/collector_matcher.cpp"
  "${CONCATENATE_DIR}/combine_cloud_handler.cpp"
  "${CONCATENATE_DIR}/combine_cloud_handler_base.cpp"
  "${CONCATENATE_DIR}/concatenation_diagnostics.cpp"
  "${CONCATENATE_DIR}/matching_policy.cpp"
  "${POINTCLOUD_PREPROCESSOR_DIR}/src/utility/memory.cpp")
target_include_directories(autoware_concatenate_core PUBLIC
  "${POINTCLOUD_PREPROCESSOR_DIR}/include" "${CONCATENATE_DIR}"
  "${AUTOWARE_UPSTREAM_DIR}/autoware_utils/autoware_utils/include")
target_link_libraries(autoware_concatenate_core PUBLIC
  autoware_point_types autoware_pcl autoware_sensing_msgs)

autoware_component(autoware_concatenate_and_time_sync
  SOURCES "${CONCATENATE_DIR}/concatenate_and_time_sync_node.cpp")
target_link_libraries(autoware_concatenate_and_time_sync PRIVATE autoware_concatenate_core)

autoware_test(autoware_concatenation_info_upstream_test
  SOURCES "${POINTCLOUD_PREPROCESSOR_DIR}/test/test_concatenation_info.cpp"
  LIBS autoware_concatenate_core GTest::gtest_main)
autoware_test(autoware_concatenation_diagnostics_upstream_test
  SOURCES "${POINTCLOUD_PREPROCESSOR_DIR}/test/test_concatenation_diagnostics.cpp"
  LIBS autoware_concatenate_core)
# The upstream node unit test (its own main()): a node per case, from a TOML file in place of
# parameter_overrides.
autoware_test(autoware_concatenate_node_unit_upstream_test
  SOURCES "${POINTCLOUD_PREPROCESSOR_DIR}/test/test_concatenate_node_unit.cpp"
    "${CONCATENATE_DIR}/concatenate_and_time_sync_node.cpp"
  LIBS autoware_concatenate_core)

# The node on the bus: two lidars, their mounts on /tf_static and the vehicle's twist.
autoware_test(autoware_concatenate_and_time_sync_node_test
  SOURCES test/test_concatenate_and_time_sync_node.cpp
    "${CONCATENATE_DIR}/concatenate_and_time_sync_node.cpp"
  LIBS autoware_concatenate_core)

# --- Localization: autoware_ekf_localizer (autoware_core 1.10.0) -------------------------

set(KALMAN_FILTER_DIR "${AUTOWARE_UPSTREAM_DIR}/autoware_core/common/autoware_kalman_filter")
add_library(autoware_kalman_filter STATIC
  "${KALMAN_FILTER_DIR}/src/kalman_filter.cpp"
  "${KALMAN_FILTER_DIR}/src/time_delay_kalman_filter.cpp")
target_include_directories(autoware_kalman_filter PUBLIC "${KALMAN_FILTER_DIR}/include")
target_link_libraries(autoware_kalman_filter PUBLIC Eigen3::Eigen)
target_compile_features(autoware_kalman_filter PUBLIC cxx_std_23)
autoware_test(autoware_kalman_filter_upstream_test
  SOURCES "${KALMAN_FILTER_DIR}/test/test_kalman_filter.cpp"
    "${KALMAN_FILTER_DIR}/test/test_time_delay_kalman_filter.cpp"
  LIBS autoware_kalman_filter GTest::gtest_main)

# autoware_localization_util: the covariance ellipse, the one part the EKF uses.
autoware_patched(autoware_localization_util
  "${AUTOWARE_UPSTREAM_DIR}/autoware_core/localization/autoware_localization_util"
  LOCALIZATION_UTIL_DIR)
add_library(autoware_localization_util STATIC
  "${LOCALIZATION_UTIL_DIR}/src/covariance_ellipse.cpp")
target_include_directories(autoware_localization_util PUBLIC "${LOCALIZATION_UTIL_DIR}/include")
target_link_libraries(autoware_localization_util PUBLIC hypha_rclcpp)
autoware_test(autoware_covariance_ellipse_upstream_test
  SOURCES "${LOCALIZATION_UTIL_DIR}/test/test_covariance_ellipse.cpp"
  LIBS autoware_localization_util GTest::gtest_main)

autoware_patched(autoware_ekf_localizer
  "${AUTOWARE_UPSTREAM_DIR}/autoware_core/localization/autoware_ekf_localizer" EKF_DIR)
add_library(autoware_ekf_localizer_core STATIC
  "${EKF_DIR}/src/ekf_localizer.cpp"
  "${EKF_DIR}/src/utils/hyper_parameters.cpp"
  "${EKF_DIR}/src/utils/covariance.cpp"
  "${EKF_DIR}/src/utils/diagnostics.cpp"
  "${EKF_DIR}/src/utils/mahalanobis.cpp"
  "${EKF_DIR}/src/utils/measurement.cpp"
  "${EKF_DIR}/src/utils/state_transition.cpp"
  "${EKF_DIR}/src/utils/warning_message.cpp")
target_include_directories(autoware_ekf_localizer_core PUBLIC "${EKF_DIR}/src" "${EKF_DIR}")
target_link_libraries(autoware_ekf_localizer_core PUBLIC
  autoware_kalman_filter autoware_localization_util hypha_rclcpp)

autoware_component(autoware_ekf_localizer SOURCES "${EKF_DIR}/src/ekf_localizer_node.cpp")
target_link_libraries(autoware_ekf_localizer PRIVATE autoware_ekf_localizer_core)

file(GLOB EKF_UPSTREAM_UTIL_TESTS "${EKF_DIR}/test/utils/*.cpp")
autoware_test(autoware_ekf_localizer_utils_upstream_test
  SOURCES ${EKF_UPSTREAM_UTIL_TESTS} LIBS autoware_ekf_localizer_core GTest::gtest_main)
autoware_test(autoware_ekf_localizer_upstream_test
  SOURCES "${EKF_DIR}/test/test_ekf_localizer.cpp"
  LIBS autoware_ekf_localizer_core GTest::gtest_main)

# The node on the bus, on simulated time the test drives: trigger, initial pose, pose and
# twist in; odometry, /tf and diagnostics out.
autoware_test(autoware_ekf_localizer_node_test
  SOURCES test/test_ekf_localizer_node.cpp "${EKF_DIR}/src/ekf_localizer_node.cpp"
  LIBS autoware_ekf_localizer_core
  ENV HYPHA_USE_SIM_TIME=1)

# --- Localization: autoware_gyro_odometer (autoware_core 1.10.0) -------------------------

autoware_patched(autoware_gyro_odometer
  "${AUTOWARE_UPSTREAM_DIR}/autoware_core/localization/autoware_gyro_odometer" GYRO_ODOMETER_DIR)
add_library(autoware_gyro_odometer_core STATIC
  "${GYRO_ODOMETER_DIR}/src/gyro_odometer.cpp"
  "${GYRO_ODOMETER_DIR}/src/gyro_odometer_diagnostics.cpp")
target_include_directories(autoware_gyro_odometer_core PUBLIC "${GYRO_ODOMETER_DIR}/src")
target_link_libraries(autoware_gyro_odometer_core PUBLIC hypha_rclcpp)

autoware_component(autoware_gyro_odometer
  SOURCES "${GYRO_ODOMETER_DIR}/src/gyro_odometer_node.cpp")
target_link_libraries(autoware_gyro_odometer PRIVATE autoware_gyro_odometer_core)

autoware_test(autoware_gyro_odometer_upstream_test
  SOURCES "${GYRO_ODOMETER_DIR}/test/test_gyro_odometer.cpp"
    "${GYRO_ODOMETER_DIR}/test/test_gyro_odometer_diagnostics.cpp"
  LIBS autoware_gyro_odometer_core GTest::gtest_main)

# The node on the bus, on simulated time the test drives: the vehicle twist and the IMU
# in, the fused twist and the diagnostics out.
autoware_test(autoware_gyro_odometer_node_test
  SOURCES test/test_gyro_odometer_node.cpp "${GYRO_ODOMETER_DIR}/src/gyro_odometer_node.cpp"
  LIBS autoware_gyro_odometer_core
  ENV HYPHA_USE_SIM_TIME=1)

# --- GNSS: Lanelet2's projection, autoware_lanelet2_extension's projectors --------------

# GeographicLib does the projections; Lanelet2 wraps it.
# Debian and Ubuntu ship its find module there, Homebrew a package configuration; both set
# GeographicLib_LIBRARIES and GeographicLib_INCLUDE_DIRS.
list(APPEND CMAKE_MODULE_PATH /usr/share/cmake/geographiclib)
find_package(GeographicLib REQUIRED)

# Lanelet2 1.2.3: the map (lanelet2_core), its reading and writing (lanelet2_io: OSM on
# pugixml, binary on Boost.Serialization) and the UTM and local Cartesian projectors.
# Shared libraries, as upstream builds them: a reader registers itself from a static
# object of its own file, which a static archive would leave out of the program.
find_package(pugixml REQUIRED)
find_package(Boost REQUIRED COMPONENTS filesystem serialization)
set(LANELET2_DIR "${AUTOWARE_UPSTREAM_DIR}/lanelet2")
file(GLOB LANELET2_CORE_SOURCES "${LANELET2_DIR}/lanelet2_core/src/*.cpp")
add_library(autoware_lanelet2_core SHARED ${LANELET2_CORE_SOURCES})
target_include_directories(autoware_lanelet2_core SYSTEM PUBLIC "${LANELET2_DIR}/lanelet2_core/include")
target_link_libraries(autoware_lanelet2_core PUBLIC Boost::headers Eigen3::Eigen)
target_compile_features(autoware_lanelet2_core PUBLIC cxx_std_23)
file(GLOB LANELET2_IO_SOURCES "${LANELET2_DIR}/lanelet2_io/src/*.cpp")
add_library(autoware_lanelet2_io SHARED ${LANELET2_IO_SOURCES})
target_include_directories(autoware_lanelet2_io SYSTEM PUBLIC "${LANELET2_DIR}/lanelet2_io/include")
target_link_libraries(autoware_lanelet2_io PUBLIC
  autoware_lanelet2_core pugixml::pugixml Boost::filesystem Boost::serialization)
add_library(autoware_lanelet2_projection SHARED
  "${LANELET2_DIR}/lanelet2_projection/src/UTM.cpp"
  "${LANELET2_DIR}/lanelet2_projection/src/LocalCartesian.cpp")
target_include_directories(autoware_lanelet2_projection SYSTEM PUBLIC
  "${LANELET2_DIR}/lanelet2_projection/include" ${GeographicLib_INCLUDE_DIRS})
target_link_libraries(autoware_lanelet2_projection PUBLIC
  autoware_lanelet2_io ${GeographicLib_LIBRARIES})

# autoware_lanelet2_extension 1.3.0: the MGRS and transverse Mercator projectors, and the
# OSM parser that reads Autoware's map format.
set(LANELET2_EXTENSION_DIR
  "${AUTOWARE_UPSTREAM_DIR}/autoware_lanelet2_extension/autoware_lanelet2_extension")
add_library(autoware_lanelet2_extension_projection SHARED
  "${LANELET2_EXTENSION_DIR}/lib/mgrs_projector.cpp"
  "${LANELET2_EXTENSION_DIR}/lib/transverse_mercator_projector.cpp"
  "${LANELET2_EXTENSION_DIR}/lib/autoware_osm_parser.cpp")
target_include_directories(autoware_lanelet2_extension_projection PUBLIC
  "${LANELET2_EXTENSION_DIR}/include")
target_link_libraries(autoware_lanelet2_extension_projection PUBLIC autoware_lanelet2_projection)

# autoware_geography_utils (autoware_core 1.10.0): projection and height on the projectors.
set(GEOGRAPHY_UTILS_DIR "${AUTOWARE_UPSTREAM_DIR}/autoware_core/common/autoware_geography_utils")
add_library(autoware_geography_utils STATIC
  "${GEOGRAPHY_UTILS_DIR}/src/height.cpp"
  "${GEOGRAPHY_UTILS_DIR}/src/lanelet2_projector.cpp"
  "${GEOGRAPHY_UTILS_DIR}/src/projection.cpp")
target_include_directories(autoware_geography_utils PUBLIC "${GEOGRAPHY_UTILS_DIR}/include")
target_link_libraries(autoware_geography_utils PUBLIC
  autoware_lanelet2_extension_projection autoware_map_msgs geographic_msgs hypha_rclcpp)
autoware_test(autoware_geography_utils_upstream_test
  SOURCES "${GEOGRAPHY_UTILS_DIR}/test/test_geography_utils.cpp"
    "${GEOGRAPHY_UTILS_DIR}/test/test_height.cpp"
    "${GEOGRAPHY_UTILS_DIR}/test/test_lanelet2_projector.cpp"
    "${GEOGRAPHY_UTILS_DIR}/test/test_projection.cpp"
  LIBS autoware_geography_utils)

# --- GNSS: autoware_gnss_poser (autoware_core 1.10.0) ----------------------------------

autoware_patched(autoware_gnss_poser
  "${AUTOWARE_UPSTREAM_DIR}/autoware_core/sensing/autoware_gnss_poser" GNSS_POSER_DIR)
autoware_component(autoware_gnss_poser SOURCES "${GNSS_POSER_DIR}/src/gnss_poser_node.cpp")
target_include_directories(autoware_gnss_poser PRIVATE "${GNSS_POSER_DIR}/src")
target_link_libraries(autoware_gnss_poser PRIVATE autoware_geography_utils autoware_sensing_msgs)

# The node on the bus: the map projection, a fix and the INS orientation in; the pose, its
# covariance, the fixed flag and map -> gnss_base_link out.
autoware_test(autoware_gnss_poser_node_test
  SOURCES test/test_gnss_poser_node.cpp "${GNSS_POSER_DIR}/src/gnss_poser_node.cpp"
  INCLUDES "${GNSS_POSER_DIR}/src"
  LIBS autoware_geography_utils autoware_sensing_msgs)

# --- Map: autoware_map_projection_loader (autoware_core 1.10.0) --------------------------

find_package(yaml-cpp REQUIRED)
set(COMPONENT_INTERFACE_SPECS_DIR
  "${AUTOWARE_UPSTREAM_DIR}/autoware_core/common/autoware_component_interface_specs")
set(MAP_PROJECTION_LOADER_DIR
  "${AUTOWARE_UPSTREAM_DIR}/autoware_core/map/autoware_map_projection_loader")
add_library(autoware_map_projection_loader_core STATIC
  "${MAP_PROJECTION_LOADER_DIR}/src/map_projection_loader.cpp"
  "${MAP_PROJECTION_LOADER_DIR}/src/load_info_from_lanelet2_map.cpp")
target_include_directories(autoware_map_projection_loader_core PUBLIC
  "${MAP_PROJECTION_LOADER_DIR}/include" "${COMPONENT_INTERFACE_SPECS_DIR}/include")
target_link_libraries(autoware_map_projection_loader_core PUBLIC
  autoware_lanelet2_extension_projection autoware_map_msgs hypha_rclcpp yaml-cpp::yaml-cpp)

autoware_component(autoware_map_projection_loader
  SOURCES "${MAP_PROJECTION_LOADER_DIR}/src/map_projection_loader_node.cpp")
target_link_libraries(autoware_map_projection_loader PRIVATE autoware_map_projection_loader_core)

autoware_test(autoware_map_projection_loader_yaml_upstream_test
  SOURCES "${MAP_PROJECTION_LOADER_DIR}/test/test_load_info_from_yaml.cpp"
  LIBS autoware_map_projection_loader_core GTest::gmock)
autoware_test(autoware_map_projection_loader_lanelet2_upstream_test
  SOURCES "${MAP_PROJECTION_LOADER_DIR}/test/test_load_info_from_lanelet2_map.cpp"
  LIBS autoware_map_projection_loader_core GTest::gmock)

# The node on the bus: the projection read from YAML, latched on /map/map_projector_info.
autoware_test(autoware_map_projection_loader_node_test
  SOURCES test/test_map_projection_loader_node.cpp
    "${MAP_PROJECTION_LOADER_DIR}/src/map_projection_loader_node.cpp"
  INCLUDES "${MAP_PROJECTION_LOADER_DIR}/src"
  LIBS autoware_map_projection_loader_core)
