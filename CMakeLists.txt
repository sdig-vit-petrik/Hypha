cmake_minimum_required(VERSION 3.20)
project(hypha_workspace VERSION 0.3.0 LANGUAGES CXX)

# An installed binary finds libhypha_ffi beside it. The build tree uses the
# directories of the linked libraries, which CMake adds by itself.
if(APPLE)
  set(CMAKE_INSTALL_RPATH "@loader_path/../lib")
else()
  set(CMAKE_INSTALL_RPATH "$ORIGIN/../lib")
endif()

get_property(HYPHA_MULTI_CONFIG GLOBAL PROPERTY GENERATOR_IS_MULTI_CONFIG)
if(NOT HYPHA_MULTI_CONFIG AND NOT CMAKE_BUILD_TYPE)
  message(FATAL_ERROR
    "CMAKE_BUILD_TYPE is empty. Configure with -DCMAKE_BUILD_TYPE=Release or Debug.")
endif()

# cargo builds for the host it runs on; arm64 builds natively on an arm64 host.
if(CMAKE_CROSSCOMPILING)
  message(FATAL_ERROR
    "hypha does not cross-compile. Build natively on the target architecture, "
    "e.g. in hypha-builder for that platform.")
endif()
if(DEFINED ENV{CARGO_BUILD_TARGET})
  message(FATAL_ERROR
    "CARGO_BUILD_TARGET is set in the environment. It moves cargo's output to a "
    "directory this project does not look in.")
endif()

option(HYPHA_BUILD_TESTS "Build tests" ON)
option(HYPHA_BUILD_APPS "Build and install the command line tools" ON)
option(HYPHA_BUILD_EXAMPLES "Build and install the C++ examples" ON)
option(HYPHA_BUILD_TESTING_MCAP
  "Build and install hypha::testing_mcap: the MCAP replay of the C++ testing component" ON)

# Strict diagnostics for hypha's own C++ targets. A parent project that adds
# hypha with FetchContent or add_subdirectory keeps its own warning policy.
if(CMAKE_SOURCE_DIR STREQUAL CMAKE_CURRENT_SOURCE_DIR)
  add_library(hypha_strict_warnings INTERFACE)
  target_compile_options(hypha_strict_warnings INTERFACE
    -Wall
    -Wextra
    -Wshadow
    -Wnon-virtual-dtor
    -Wold-style-cast
    -Wcast-align
    -Wunused
    -Woverloaded-virtual
    -Wdouble-promotion
    -Wformat=2
    -Werror
  )
endif()

set(GTEST_USE_STATIC_LIBS ON)

enable_testing()

# Everything cargo builds, in one call (crates/hypha_ffi).
set(HYPHA_CARGO_PACKAGE_FLAGS -p hypha_ffi -p hypha_msggen)
if(HYPHA_BUILD_APPS)
  list(APPEND HYPHA_CARGO_PACKAGE_FLAGS -p hypha_tools -p hypha_ros_bridge)
endif()
if(HYPHA_BUILD_TESTING_MCAP)
  list(APPEND HYPHA_CARGO_PACKAGE_FLAGS -p hypha_testing_mcap)
endif()

add_subdirectory(crates/hypha_ffi)
add_subdirectory(crates/hypha_msggen)
add_subdirectory(crates/hypha_msgs)

add_subdirectory(crates/hypha-cxx)

if(HYPHA_BUILD_APPS)
  add_subdirectory(apps)
endif()

if(HYPHA_BUILD_EXAMPLES)
  add_subdirectory(examples)
endif()
