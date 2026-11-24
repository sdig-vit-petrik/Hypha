# A software PLC cycle: latch the inputs into a process image, run the logic
# against that image, write the outputs. The shape to copy when the logic must
# not see an input change under it, and the reference for a cycle node and its
# tests (README.md).
#
# Self-contained: this builds inside the hypha tree, and equally well on its
# own against an installed SDK.
#
#   cmake -S . -B build && cmake --build build && ctest --test-dir build

cmake_minimum_required(VERSION 3.20)
project(scan_cycle VERSION 0.2.0 LANGUAGES CXX)

include(GNUInstallDirs)

# Already defined when this builds as part of the hypha tree. Standalone, the
# installed package supplies it.
if(NOT TARGET hypha::sdk)
  find_package(hypha CONFIG REQUIRED)
endif()

add_executable(hypha_scan_cycle src/scan_cycle.cpp)
target_include_directories(hypha_scan_cycle PRIVATE inc)
target_link_libraries(hypha_scan_cycle PRIVATE hypha::sdk)
target_compile_features(hypha_scan_cycle PRIVATE cxx_std_23)
target_compile_options(hypha_scan_cycle PRIVATE -Wall -Wextra -Wpedantic)


# The SDK is a shared library, so an installed binary has to be able to find
# it. This installs only the example, never the framework it consumes.
if(TARGET hypha::ffi)
  get_target_property(HYPHA_FFI_LIBRARY hypha::ffi IMPORTED_LOCATION)
  if(HYPHA_FFI_LIBRARY)
    get_filename_component(HYPHA_FFI_LIBRARY_DIR "${HYPHA_FFI_LIBRARY}" DIRECTORY)
    set_target_properties(hypha_scan_cycle PROPERTIES
      INSTALL_RPATH "${HYPHA_FFI_LIBRARY_DIR}")
  endif()
endif()

install(TARGETS hypha_scan_cycle RUNTIME DESTINATION "${CMAKE_INSTALL_BINDIR}")

install(DIRECTORY config/ DESTINATION "${CMAKE_INSTALL_DATADIR}/hypha-examples/scan_cycle")

# Three levels, one file each (README.md): the control law alone, the step through
# hypha::testing::StepHarness, and the started cycle on the bus. Built with GoogleTest
# when it is found; inside the hypha tree, only when the tree builds its tests.
set(SCAN_CYCLE_TESTS ON)
if(DEFINED HYPHA_BUILD_TESTS AND NOT HYPHA_BUILD_TESTS)
  set(SCAN_CYCLE_TESTS OFF)
endif()
if(SCAN_CYCLE_TESTS)
  find_package(GTest QUIET)
endif()
if(SCAN_CYCLE_TESTS AND TARGET GTest::gtest_main)
  enable_testing()
  set(SCAN_CYCLE_TEST_CONFIG "${CMAKE_CURRENT_SOURCE_DIR}/test/scan_cycle_test.toml")

  # No bus and no shared memory: pure logic and the step harness.
  add_executable(scan_cycle_step_test
    test/test_command_law.cpp
    test/test_scan_cycle_step.cpp)
  # The started cycle with real transport, on simulated time.
  add_executable(scan_cycle_bus_test test/test_scan_cycle_bus.cpp)

  foreach(target scan_cycle_step_test scan_cycle_bus_test)
    target_include_directories(${target} PRIVATE inc)
    target_link_libraries(${target} PRIVATE hypha::sdk GTest::gtest_main)
    target_compile_features(${target} PRIVATE cxx_std_23)
    target_compile_options(${target} PRIVATE -Wall -Wextra -Wpedantic)
    target_compile_definitions(${target} PRIVATE
      SCAN_CYCLE_TEST_CONFIG="${SCAN_CYCLE_TEST_CONFIG}")
    hypha_add_test(${target})
  endforeach()
  set_tests_properties(scan_cycle_bus_test PROPERTIES LABELS "needs-iceoryx2" TIMEOUT 120)
endif()
