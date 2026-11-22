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
find_program(AUTOWARE_PATCH patch REQUIRED)
enable_testing()

set(AUTOWARE_UPSTREAM_DIR "${CMAKE_CURRENT_SOURCE_DIR}/upstream")
set(AUTOWARE_MSG_OUT "${CMAKE_CURRENT_BINARY_DIR}/generated/include")

# Messages the ported nodes publish that hypha does not ship, in ROS 1 syntax until #45.
hypha_generate_messages(PACKAGE autoware_internal_debug_msgs
  MSG_DIR "${CMAKE_CURRENT_SOURCE_DIR}/msg/autoware_internal_debug_msgs/msg"
  OUT_DIR "${AUTOWARE_MSG_OUT}")
hypha_generate_messages(PACKAGE autoware_internal_msgs
  MSG_DIR "${CMAKE_CURRENT_SOURCE_DIR}/msg/autoware_internal_msgs/msg"
  OUT_DIR "${AUTOWARE_MSG_OUT}"
  DEPENDS std_msgs)

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
  "${AUTOWARE_UPSTREAM_DIR}/autoware_utils/autoware_utils_system/include")
target_link_libraries(hypha_rclcpp PUBLIC
  hypha::sdk autoware_internal_debug_msgs autoware_internal_msgs Eigen3::Eigen)
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

# autoware_test(<name> SOURCES <file>... [INCLUDES <dir>...]): one GoogleTest program.
function(autoware_test name)
  cmake_parse_arguments(ARG "" "" "SOURCES;INCLUDES;LIBS" ${ARGN})
  add_executable(${name} ${ARG_SOURCES})
  target_include_directories(${name} PRIVATE ${ARG_INCLUDES})
  target_link_libraries(${name} PRIVATE hypha_rclcpp GTest::gtest ${ARG_LIBS})
  add_test(NAME ${name} COMMAND ${name} WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}")
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
