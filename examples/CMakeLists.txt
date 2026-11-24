cmake_minimum_required(VERSION 3.20)
if(CMAKE_SOURCE_DIR STREQUAL CMAKE_CURRENT_SOURCE_DIR)
  project(hypha_examples LANGUAGES CXX)
endif()

# Example nodes. Not part of the shipped stack.
#
# Every example is self-contained: each folder holds its sources and a
# CMakeLists.txt that finds hypha when it is not already part of a larger
# build.

add_subdirectory(speed)
add_subdirectory(scan_cycle)
add_subdirectory(large_payload)
add_subdirectory(bench)
add_subdirectory(sim_twin)
add_subdirectory(sensor_fusion)
add_subdirectory(sensor_pipeline)

# The MRS and Autoware ports need Eigen (Autoware also GoogleTest) and build vendored
# upstream code, so they build only against an installed SDK (examples/check.sh), not
# inside the framework build.
if(CMAKE_SOURCE_DIR STREQUAL CMAKE_CURRENT_SOURCE_DIR)
  enable_testing()
  add_subdirectory(mrs)
  add_subdirectory(autoware)
endif()
