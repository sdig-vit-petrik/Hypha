# Sensor pipeline: a synthetic IMU and LiDAR source, an event-driven processing node on a
# plain hypha::Node with one worker thread, and a replay check that compares two runs of
# the node by message stamp.
#
#   cmake -S . -B build && cmake --build build

cmake_minimum_required(VERSION 3.20)
project(sensor_pipeline VERSION 0.3.0 LANGUAGES CXX)

include(GNUInstallDirs)

if(NOT TARGET hypha::sdk)
  find_package(hypha CONFIG REQUIRED)
endif()

set(SENSOR_PIPELINE_TARGETS hypha_sensor_source hypha_cloud_processor)

add_executable(hypha_sensor_source src/sensor_source.cpp)
add_executable(hypha_cloud_processor src/cloud_processor.cpp)

# The comparison reads MCAP files with the test-only reader, which an SDK built without
# HYPHA_BUILD_TESTING_MCAP does not have.
if(TARGET hypha::testing_mcap)
  add_executable(sensor_pipeline_compare src/compare_by_stamp.cpp)
  target_link_libraries(sensor_pipeline_compare PRIVATE hypha::testing_mcap)
  list(APPEND SENSOR_PIPELINE_TARGETS sensor_pipeline_compare)
endif()

foreach(target IN LISTS SENSOR_PIPELINE_TARGETS)
  target_include_directories(${target} PRIVATE inc)
  target_link_libraries(${target} PRIVATE hypha::sdk)
  target_compile_features(${target} PRIVATE cxx_std_23)
  target_compile_options(${target} PRIVATE -Wall -Wextra -Wpedantic)
endforeach()

get_target_property(HYPHA_FFI_LIBRARY hypha::ffi IMPORTED_LOCATION)
get_filename_component(HYPHA_FFI_LIBRARY_DIR "${HYPHA_FFI_LIBRARY}" DIRECTORY)
foreach(target IN LISTS SENSOR_PIPELINE_TARGETS)
  set_target_properties(${target} PROPERTIES INSTALL_RPATH "${HYPHA_FFI_LIBRARY_DIR}")
endforeach()

install(TARGETS ${SENSOR_PIPELINE_TARGETS} RUNTIME DESTINATION "${CMAKE_INSTALL_BINDIR}")

# The replay check (replay_check.sh) runs the processor twice on the recorded fixture,
# with hypha play and hypha record, compares the two outputs by stamp, and runs the slow
# mode once. It needs the tools and the comparison; it is not run when the hypha tree is
# built without tests.
set(SENSOR_PIPELINE_REPLAY_TEST ON)
if(DEFINED HYPHA_BUILD_TESTS AND NOT HYPHA_BUILD_TESTS)
  set(SENSOR_PIPELINE_REPLAY_TEST OFF)
endif()
if(SENSOR_PIPELINE_REPLAY_TEST AND TARGET hypha::hypha AND TARGET sensor_pipeline_compare)
  hypha_add_test(sensor_pipeline_replay_check
    COMMAND bash "${CMAKE_CURRENT_SOURCE_DIR}/replay_check.sh"
      --hypha "$<TARGET_FILE:hypha::hypha>"
      --processor "$<TARGET_FILE:hypha_cloud_processor>"
      --compare "$<TARGET_FILE:sensor_pipeline_compare>"
      --work "${CMAKE_CURRENT_BINARY_DIR}/replay_check")
  set_tests_properties(sensor_pipeline_replay_check PROPERTIES
    LABELS "needs-iceoryx2"
    TIMEOUT 120)
endif()
