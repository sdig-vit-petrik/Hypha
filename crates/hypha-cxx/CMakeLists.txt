cmake_minimum_required(VERSION 3.20)
project(hypha VERSION 0.3.0 LANGUAGES C CXX)

include(GNUInstallDirs)
include(CMakePackageConfigHelpers)

# libhypha_ffi.so and its generated header. crates/hypha_ffi builds the library
# with cargo; this package wraps it as the installable hypha::ffi.
set(HYPHA_FFI_HEADER_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../hypha_ffi/include")
if(NOT TARGET hypha_ffi OR NOT TARGET hypha_standard_messages OR NOT HYPHA_MSGGEN_BIN)
  message(FATAL_ERROR
    "crates/hypha-cxx is built from the hypha root CMakeLists.txt, which builds "
    "libhypha_ffi, hypha_msggen and the message headers with cargo.")
endif()
get_target_property(HYPHA_FFI_LINK_FILE hypha_ffi IMPORTED_LOCATION)
get_target_property(HYPHA_FFI_LIB_FILE hypha_ffi HYPHA_RAW_LOCATION)

get_target_property(HYPHA_FFI_SONAME hypha_ffi IMPORTED_SONAME)

add_library(hypha::ffi SHARED IMPORTED GLOBAL)
set_target_properties(hypha::ffi PROPERTIES IMPORTED_LOCATION "${HYPHA_FFI_LINK_FILE}")
# Without the soname, a clean configure (library not built yet) links every test
# with no build rpath. See crates/hypha_ffi/CMakeLists.txt.
if(HYPHA_FFI_SONAME)
  set_target_properties(hypha::ffi PROPERTIES IMPORTED_SONAME "${HYPHA_FFI_SONAME}")
endif()
add_dependencies(hypha::ffi hypha_ffi_cargo)
target_include_directories(hypha::ffi INTERFACE "${HYPHA_FFI_HEADER_DIR}")
find_package(Threads REQUIRED)
target_link_libraries(hypha::ffi INTERFACE Threads::Threads ${CMAKE_DL_LIBS} m)

add_library(hypha INTERFACE)
add_library(hypha::sdk ALIAS hypha)
set_target_properties(hypha PROPERTIES EXPORT_NAME sdk)

target_include_directories(hypha INTERFACE
  $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
  $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/log/include>
  $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/base/include>
  $<INSTALL_INTERFACE:${CMAKE_INSTALL_INCLUDEDIR}>
)
target_compile_features(hypha INTERFACE cxx_std_23)

if(TARGET hypha_strict_warnings)
  target_link_libraries(hypha INTERFACE $<BUILD_INTERFACE:hypha_strict_warnings>)
endif()

# shm_open() is in librt rather than libc before glibc 2.34.
include(CheckLibraryExists)
check_library_exists(rt shm_open "" HYPHA_HAS_LIBRT)
if(HYPHA_HAS_LIBRT)
  target_link_libraries(hypha INTERFACE rt)
endif()

target_link_libraries(hypha INTERFACE hypha::ffi)

# ROS 1 wire-compatible message headers, at their ROS 1 path <package>/<Name>.h.
# This package owns the directory of each standard package: the headers
# crates/hypha_msgs generates into the build tree, and in hypha_msgs/ also
# include/hypha_msgs/ros_time.h by hand.
set(HYPHA_MSGS_INCLUDE_DIR "${CMAKE_BINARY_DIR}/generated/include")
# The headers appear during the build, so the link is for ordering only.
target_link_libraries(hypha INTERFACE $<BUILD_INTERFACE:hypha_standard_messages>)
target_include_directories(hypha INTERFACE
  $<BUILD_INTERFACE:${HYPHA_MSGS_INCLUDE_DIR}>
)

# The companion headers of sensor_msgs that ROS 1 ships beside the messages
# (point_cloud2_iterator.h, point_field_conversion.h, image_encodings.h,
# distortion_models.h): ros/common_msgs unchanged, in upstream/common_msgs, which the
# SDK installs into the include directory of sensor_msgs. The ROS 2 names of the same
# headers (<sensor_msgs/point_cloud2_iterator.hpp>, ...) are include/sensor_msgs/*.hpp.
set(HYPHA_COMMON_MSGS_INCLUDE_DIR
  "${CMAKE_CURRENT_SOURCE_DIR}/upstream/common_msgs/sensor_msgs/include")
target_include_directories(hypha SYSTEM INTERFACE
  $<BUILD_INTERFACE:${HYPHA_COMMON_MSGS_INCLUDE_DIR}>
)

# hypha::testing_mcap: the static MCAP reader (crate hypha_testing_mcap) under
# hypha/testing/mcap_replay.h (and its mcap_*.h parts), with HYPHA_BUILD_TESTING_MCAP.
if(HYPHA_BUILD_TESTING_MCAP)
  set(HYPHA_TESTING_MCAP_HEADER_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../hypha_testing_mcap/include")
  get_target_property(HYPHA_TESTING_MCAP_ARCHIVE hypha_testing_mcap_archive IMPORTED_LOCATION)
  get_filename_component(HYPHA_TESTING_MCAP_ARCHIVE_NAME "${HYPHA_TESTING_MCAP_ARCHIVE}" NAME)
  add_library(hypha_testing_mcap INTERFACE)
  add_library(hypha::testing_mcap ALIAS hypha_testing_mcap)
  set_target_properties(hypha_testing_mcap PROPERTIES EXPORT_NAME testing_mcap)
  target_include_directories(hypha_testing_mcap INTERFACE
    $<BUILD_INTERFACE:${HYPHA_TESTING_MCAP_HEADER_DIR}>
    $<INSTALL_INTERFACE:${CMAKE_INSTALL_INCLUDEDIR}>
  )
  # The native libraries a Rust static library needs, as `cargo rustc -p
  # hypha_testing_mcap --lib --crate-type staticlib -- --print native-static-libs`
  # prints them on Linux. The macOS list is rustc's usual one, not checked here.
  if(APPLE)
    set(HYPHA_TESTING_MCAP_NATIVE_LIBS iconv System c m)
  else()
    set(HYPHA_TESTING_MCAP_NATIVE_LIBS gcc_s util rt pthread m dl c)
  endif()
  target_link_libraries(hypha_testing_mcap INTERFACE
    $<BUILD_INTERFACE:hypha_testing_mcap_archive>
    $<INSTALL_INTERFACE:$<INSTALL_PREFIX>/${CMAKE_INSTALL_LIBDIR}/${HYPHA_TESTING_MCAP_ARCHIVE_NAME}>
    ${HYPHA_TESTING_MCAP_NATIVE_LIBS}
  )
endif()

install(TARGETS hypha EXPORT hyphaTargets)
if(HYPHA_BUILD_TESTING_MCAP)
  install(DIRECTORY include/ DESTINATION ${CMAKE_INSTALL_INCLUDEDIR})
  install(TARGETS hypha_testing_mcap EXPORT hyphaTargets)
  install(FILES "${HYPHA_TESTING_MCAP_ARCHIVE}" DESTINATION ${CMAKE_INSTALL_LIBDIR})
  install(FILES "${HYPHA_TESTING_MCAP_HEADER_DIR}/hypha/testing/mcap_reader.h"
    DESTINATION ${CMAKE_INSTALL_INCLUDEDIR}/hypha/testing)
else()
  install(DIRECTORY include/ DESTINATION ${CMAKE_INSTALL_INCLUDEDIR}
    PATTERN "mcap_*.h" EXCLUDE)
endif()
install(DIRECTORY "${HYPHA_COMMON_MSGS_INCLUDE_DIR}/" DESTINATION ${CMAKE_INSTALL_INCLUDEDIR})
install(DIRECTORY log/include/ DESTINATION ${CMAKE_INSTALL_INCLUDEDIR})
install(DIRECTORY base/include/ DESTINATION ${CMAKE_INSTALL_INCLUDEDIR})

# The library does not exist at configure time, so it is resolved at install time:
# install(FILES) of a symlink would install the symlink, not the library.
install(CODE "
  file(REAL_PATH \"${HYPHA_FFI_LIB_FILE}\" hypha_ffi_real)
  file(INSTALL DESTINATION \"\${CMAKE_INSTALL_PREFIX}/${CMAKE_INSTALL_LIBDIR}\"
       TYPE FILE RENAME \"${HYPHA_FFI_VERSIONED_NAME}\" FILES \"\${hypha_ffi_real}\")
")
install(FILES "${HYPHA_FFI_HEADER_DIR}/hypha/hypha_ffi.h"
  DESTINATION ${CMAKE_INSTALL_INCLUDEDIR}/hypha)

# The .msg sources the headers above were generated from, and the CMake
# function that generates an application's own messages against them. An
# application message that names std_msgs/Header needs the definition, not the
# header.
set(HYPHA_STANDARD_MSG_SOURCE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../hypha_msgs/msg")
set(HYPHA_MESSAGES_MODULE "${CMAKE_CURRENT_SOURCE_DIR}/../hypha_msggen/cmake/hyphaMessages.cmake")
if(NOT IS_DIRECTORY "${HYPHA_STANDARD_MSG_SOURCE_DIR}")
  message(FATAL_ERROR
    "hypha-cxx: no .msg definitions at ${HYPHA_STANDARD_MSG_SOURCE_DIR}. They are "
    "installed with the SDK so that an application can generate its own messages "
    "against them.")
endif()
install(DIRECTORY "${HYPHA_STANDARD_MSG_SOURCE_DIR}/"
  DESTINATION "${CMAKE_INSTALL_DATADIR}/hypha/msg")

# The headers this build generated for each standard package, by name: never the
# directory, which can hold files an older build left (#161), nor the rest of the
# build tree, where an application that builds hypha as a subproject generates
# its own packages. hypha_msgs/ merges with ros_time.h, which
# install(DIRECTORY include/) placed at the same destination.
file(GLOB HYPHA_STANDARD_PACKAGE_DIRS LIST_DIRECTORIES true RELATIVE
  "${HYPHA_STANDARD_MSG_SOURCE_DIR}" "${HYPHA_STANDARD_MSG_SOURCE_DIR}/*")
foreach(package ${HYPHA_STANDARD_PACKAGE_DIRS})
  if(IS_DIRECTORY "${HYPHA_STANDARD_MSG_SOURCE_DIR}/${package}")
    get_target_property(package_headers ${package} HYPHA_GENERATED_HEADERS)
    install(FILES ${package_headers} DESTINATION ${CMAKE_INSTALL_INCLUDEDIR}/${package})
  endif()
endforeach()
install(FILES "${HYPHA_MESSAGES_MODULE}"
  DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/hypha)

# hypha_add_test(), which runs an application's test in a bus domain of its own.
include("${CMAKE_CURRENT_SOURCE_DIR}/cmake/hyphaTesting.cmake")
install(FILES "${CMAKE_CURRENT_SOURCE_DIR}/cmake/hyphaTesting.cmake"
  DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/hypha)

# The generator, so that hypha_generate_messages() works from an install prefix.
install(PROGRAMS "${HYPHA_MSGGEN_BIN}" DESTINATION ${CMAKE_INSTALL_BINDIR})

install(EXPORT hyphaTargets
  NAMESPACE hypha::
  DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/hypha
)

write_basic_package_version_file(
  "${CMAKE_CURRENT_BINARY_DIR}/hyphaConfigVersion.cmake"
  COMPATIBILITY ExactVersion
)
configure_package_config_file(
  "${CMAKE_CURRENT_SOURCE_DIR}/cmake/hyphaConfig.cmake.in"
  "${CMAKE_CURRENT_BINARY_DIR}/hyphaConfig.cmake"
  INSTALL_DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/hypha
)
install(FILES
  "${CMAKE_CURRENT_BINARY_DIR}/hyphaConfig.cmake"
  "${CMAKE_CURRENT_BINARY_DIR}/hyphaConfigVersion.cmake"
  DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/hypha
)

# Every test runs against the Rust core, so a run needs a reachable shared
# memory segment. That is the only environment they need.

if(HYPHA_BUILD_TESTS)
  find_package(GTest REQUIRED)
  find_package(Python3 COMPONENTS Interpreter REQUIRED)
  enable_testing()

  # A gtest program writes its results to gtest-xml/<ctest name>.xml in the build
  # tree; scripts/traceability.py reads them for the qualification report. Call it
  # after set_tests_properties, which replaces the environment of a test.
  function(hypha_gtest_report test_name)
    set_property(TEST ${test_name} APPEND PROPERTY
      ENVIRONMENT "GTEST_OUTPUT=xml:${CMAKE_BINARY_DIR}/gtest-xml/${test_name}.xml")
  endfunction()

  add_executable(test_hypha_c test/test_c_abi.c)
  target_link_libraries(test_hypha_c PRIVATE hypha::ffi)
  hypha_add_test(test_hypha_c)

  add_executable(test_hypha_base
    base/test/test_main.cpp
    base/test/test_time.cpp
    base/test/test_clock.cpp
    base/test/test_duration_saturation.cpp
    base/test/test_time_saturation.cpp
    base/test/test_seconds_precision.cpp
  )
  target_link_libraries(test_hypha_base PRIVATE hypha GTest::gtest)
  # Simulated time joins the bus, and base/test/test_main.cpp sets no domain.
  hypha_add_test(test_hypha_base)
  hypha_gtest_report(test_hypha_base)

  # The C++ peer of the framework-stamp round trip. The Rust test in
  # crates/hypha/tests runs it against the Rust SDK, so it is a plain program.
  add_executable(framework_stamp_peer test/framework_stamp_peer.cpp)
  target_link_libraries(framework_stamp_peer PRIVATE hypha)

  # A replay participant that a backward seek restarts. The Rust test
  # apps/hypha_tools/tests/play_restart.rs runs it under hypha play.
  add_executable(replay_restart_peer test/replay_restart_peer.cpp)
  target_link_libraries(replay_restart_peer PRIVATE hypha)

  # A triggered cycle participant; two of them form a loop with no delay element. The Rust
  # test apps/hypha_tools/tests/play_lockstep_waits.rs runs them under hypha_play.
  add_executable(replay_loop_peer test/replay_loop_peer.cpp)
  target_link_libraries(replay_loop_peer PRIVATE hypha)

  # The exit code of a real runNode process: 0 clean, 78 configuration, 70 runtime.
  add_executable(run_node_exit_code_node test/run_node_exit_code_node.cpp)
  target_link_libraries(run_node_exit_code_node PRIVATE hypha)
  foreach(exit_case IN ITEMS "missing;78" "unread;78" "throw;70" "clean;0")
    list(GET exit_case 0 case_name)
    list(GET exit_case 1 case_code)
    hypha_add_test(run_node_exit_code_${case_name}
      COMMAND ${CMAKE_COMMAND}
        -DNODE=$<TARGET_FILE:run_node_exit_code_node>
        -DCASE=${case_name}
        -DEXPECTED=${case_code}
        -DWORK_DIR=${CMAKE_CURRENT_BINARY_DIR}
        -P ${CMAKE_CURRENT_SOURCE_DIR}/test/run_node_exit_code.cmake)
    set_tests_properties(run_node_exit_code_${case_name} PROPERTIES LABELS "needs-iceoryx2")
  endforeach()

  # A library with another ABI revision is refused before any startup call.
  add_executable(test_hypha_abi_mismatch
    test/test_main.cpp
    test/abi_revision_mismatch_is_refused.cpp
  )
  target_link_libraries(test_hypha_abi_mismatch PRIVATE hypha GTest::gtest)
  target_compile_definitions(test_hypha_abi_mismatch PRIVATE HYPHA_NO_LOGGING)
  add_test(NAME test_hypha_abi_mismatch COMMAND test_hypha_abi_mismatch)
  hypha_gtest_report(test_hypha_abi_mismatch)

  set(HYPHA_CYCLE_CONTRACT_FLAGS)
  if(CMAKE_OSX_SYSROOT)
    list(APPEND HYPHA_CYCLE_CONTRACT_FLAGS "--flag=-isysroot"
         "--flag=${CMAKE_OSX_SYSROOT}")
  endif()
  if(CMAKE_OSX_DEPLOYMENT_TARGET)
    list(APPEND HYPHA_CYCLE_CONTRACT_FLAGS
         "--flag=-mmacosx-version-min=${CMAKE_OSX_DEPLOYMENT_TARGET}")
  endif()

  add_test(NAME test_cycle_compile_contract
    COMMAND ${Python3_EXECUTABLE}
      ${CMAKE_CURRENT_SOURCE_DIR}/test/cycle_compile_contract.py
      --compiler "${CMAKE_CXX_COMPILER}"
      --include "${CMAKE_CURRENT_SOURCE_DIR}/include"
      --include "${CMAKE_CURRENT_SOURCE_DIR}/log/include"
      --include "${HYPHA_MSGS_INCLUDE_DIR}"
      --include "${CMAKE_CURRENT_SOURCE_DIR}/base/include"
      --include "${CMAKE_CURRENT_SOURCE_DIR}/../hypha_ffi/include"
      ${HYPHA_CYCLE_CONTRACT_FLAGS}
  )

  add_executable(test_hypha_log
    test/test_main.cpp
    test/test_log_throttle.cpp
    test/test_log_transition.cpp
    test/test_log_lazy_arguments.cpp
    test/test_log_module_name.cpp
    test/test_log_module_default.cpp
    test/test_log_module_without_calls.cpp
    test/test_publish_or_warn_log.cpp
  )
  target_link_libraries(test_hypha_log PRIVATE hypha GTest::gtest)
  add_test(NAME test_hypha_log COMMAND test_hypha_log)
  hypha_gtest_report(test_hypha_log)

  add_executable(test_log_module_overflow_uses_default
    test/test_main.cpp
    test/test_log_module_overflow_uses_default.cpp
  )
  target_link_libraries(test_log_module_overflow_uses_default PRIVATE hypha GTest::gtest)
  add_test(NAME test_log_module_overflow_uses_default COMMAND test_log_module_overflow_uses_default)
  hypha_gtest_report(test_log_module_overflow_uses_default)

  # An invalid module name must stop the build of its translation unit.
  add_library(log_module_name_invalid OBJECT EXCLUDE_FROM_ALL test/log_module_name_invalid.cpp)
  target_link_libraries(log_module_name_invalid PRIVATE hypha)
  add_test(NAME log_module_name_invalid_is_refused
    COMMAND ${CMAKE_COMMAND} --build ${CMAKE_BINARY_DIR} --target log_module_name_invalid)
  set_tests_properties(log_module_name_invalid_is_refused PROPERTIES WILL_FAIL TRUE)

  # A loan of a message with no field to loan must stop the build, with the rule in words.
  add_library(typed_loan_not_loanable OBJECT EXCLUDE_FROM_ALL test/typed_loan_not_loanable.cpp)
  target_link_libraries(typed_loan_not_loanable PRIVATE hypha)
  add_test(NAME typed_loan_of_a_message_without_an_array_is_refused
    COMMAND ${CMAKE_COMMAND} --build ${CMAKE_BINARY_DIR} --target typed_loan_not_loanable)
  set_tests_properties(typed_loan_of_a_message_without_an_array_is_refused PROPERTIES
    PASS_REGULAR_EXPRESSION "this message has no single top-level array of numbers to write in place")

  add_executable(test_hypha
    test/test_main.cpp
    test/wall_clock_listener.cpp
    test/test_publisher.cpp
    test/test_typed_loan.cpp
    test/test_subscriber.cpp
    test/subscriber_latest_pair_is_consistent.cpp
    test/test_parameters.cpp
    test/test_parameter_lists.cpp
    test/test_config_views.cpp
    test/test_config_walk.cpp
    test/test_node.cpp
    test/test_node_creation_race.cpp
    test/test_schema.cpp
    test/test_pipeline.cpp
    test/test_header_seq.cpp
    test/test_run_node_lifecycle.cpp
    test/test_startup_configuration.cpp
    test/test_handle_lifetime.cpp
    test/test_ros_serialization_bounds.cpp
    test/test_ros1_codec_encode.cpp
    test/test_typed_schema_identity.cpp
    test/test_publish_receive_status.cpp
    test/test_result_names.cpp
    test/test_callback_failure.cpp
    test/test_node_health.cpp
    test/test_metrics.cpp
    test/test_cycle_step_faults.cpp
    test/test_cycle_shutdown.cpp
    test/test_cycle_node.cpp
    test/test_triggered_cycle.cpp
    test/test_cycle_timer.cpp
    test/test_twist_mux_shape.cpp
    test/test_newest_only.cpp
    test/test_transforms_port.cpp
    test/test_transform_buffer.cpp
    test/test_subscriber_diagnostics.cpp
    test/test_cycle_sample_stamps.cpp
    test/test_clock_driver.cpp
    test/cycle_arrivals_merge.cpp
    test/cycle_output_send_order.cpp
    test/cycle_node_config_tables.cpp
    test/test_input_supervision.cpp
    test/cycle_supervision_from_table.cpp
    test/cycle_step_exception_is_logged.cpp
    test/test_application_doc_examples.cpp
    test/test_cycle_doc_examples.cpp
    test/test_quick_start_doc_examples.cpp
    test/test_cycle_verdict_mapping.cpp
    test/test_node_session_ownership.cpp
    test/test_cycle_registry_lifetime.cpp
    test/test_message_stamp.cpp
    test/test_time_jump.cpp
    test/test_node_identity.cpp
    test/test_node_introspection.cpp
    test/test_unique_handle.cpp
    test/test_stop_signal.cpp
    test/test_fail_run.cpp
    test/test_worker_stop.cpp
    test/test_handover.cpp
    test/test_application_skeleton.cpp
    test/test_step_harness.cpp
    test/test_diagnostic_updater.cpp
    test/test_raw_endpoints.cpp
    test/test_qos.cpp
    test/test_topic_overloading.cpp
    test/test_sensor_msgs_companions.cpp
    test/test_camera_info.cpp
    test/test_sensor_msgs_ros2_names.cpp
  )
  # The ROS 2 names of sensor_msgs, for test_sensor_msgs_ros2_names.cpp.
  hypha_generate_ros2_headers(test_hypha_ros2_headers PACKAGES sensor_msgs)
  target_link_libraries(test_hypha PRIVATE test_hypha_ros2_headers)
  # Every public header compiles on its own: one generated translation unit each.
  file(GLOB_RECURSE HYPHA_PUBLIC_HEADERS RELATIVE "${CMAKE_CURRENT_SOURCE_DIR}/include"
       CONFIGURE_DEPENDS "${CMAKE_CURRENT_SOURCE_DIR}/include/*.h")
  if(NOT HYPHA_BUILD_TESTING_MCAP)
    list(REMOVE_ITEM HYPHA_PUBLIC_HEADERS
      "hypha/testing/mcap_replay.h"
      "hypha/testing/mcap_replay_impl.h"
      "hypha/testing/mcap_file.h")
  endif()
  set(HYPHA_HEADER_UNITS)
  foreach(header IN LISTS HYPHA_PUBLIC_HEADERS)
    string(MAKE_C_IDENTIFIER "${header}" unit)
    set(unit "${CMAKE_CURRENT_BINARY_DIR}/header_contract/${unit}.cpp")
    file(CONFIGURE OUTPUT "${unit}" CONTENT "#include \"${header}\"\n")
    list(APPEND HYPHA_HEADER_UNITS "${unit}")
  endforeach()
  add_library(test_hypha_header_contract OBJECT ${HYPHA_HEADER_UNITS})
  target_link_libraries(test_hypha_header_contract PRIVATE hypha)
  if(HYPHA_BUILD_TESTING_MCAP)
    target_link_libraries(test_hypha_header_contract PRIVATE hypha::testing_mcap)
  endif()
  target_compile_definitions(test_hypha_header_contract PRIVATE HYPHA_NO_LOGGING)
  add_dependencies(test_hypha test_hypha_header_contract)

  # The companion headers of sensor_msgs compile on their own, first in their unit,
  # under both names. Upstream's point_field_conversion.h does not include PointField.h
  # (in ROS 1 neither); its unit does, which the ROS 2 name does itself.
  set(HYPHA_COMPANION_HEADERS
    sensor_msgs/point_cloud2_iterator.h sensor_msgs/point_field_conversion.h
    sensor_msgs/image_encodings.h sensor_msgs/distortion_models.h
    sensor_msgs/point_cloud2_iterator.hpp sensor_msgs/point_field_conversion.hpp
    sensor_msgs/image_encodings.hpp sensor_msgs/distortion_models.hpp)
  set(HYPHA_COMPANION_UNITS)
  foreach(header IN LISTS HYPHA_COMPANION_HEADERS)
    string(MAKE_C_IDENTIFIER "${header}" unit)
    set(unit "${CMAKE_CURRENT_BINARY_DIR}/header_contract/companion_${unit}.cpp")
    set(prelude "")
    if(header STREQUAL "sensor_msgs/point_field_conversion.h")
      set(prelude "#include <sensor_msgs/PointField.h>\n")
    endif()
    file(CONFIGURE OUTPUT "${unit}" CONTENT "${prelude}#include <${header}>\n")
    list(APPEND HYPHA_COMPANION_UNITS "${unit}")
  endforeach()
  add_library(test_hypha_companion_headers OBJECT ${HYPHA_COMPANION_UNITS})
  target_link_libraries(test_hypha_companion_headers PRIVATE hypha)
  add_dependencies(test_hypha test_hypha_companion_headers)

  add_library(test_hypha_log_header_order OBJECT
    test/headers/log_status_first.cpp
    test/headers/log_header_first.cpp
  )
  target_link_libraries(test_hypha_log_header_order PRIVATE hypha)
  add_dependencies(test_hypha test_hypha_log_header_order)

  target_link_libraries(test_hypha PRIVATE hypha GTest::gtest)
  target_compile_definitions(test_hypha PRIVATE HYPHA_NO_LOGGING)
  # One ctest entry per gtest shard, one per core, so `ctest -j` runs the shards in
  # parallel. test_main.cpp gives each process a bus domain and a TMPDIR of its own.
  cmake_host_system_information(RESULT HYPHA_TEST_SHARDS QUERY NUMBER_OF_LOGICAL_CORES)
  math(EXPR HYPHA_LAST_TEST_SHARD "${HYPHA_TEST_SHARDS} - 1")
  # A test that raises SIGINT or SIGTERM stops its whole process for good, so it is
  # left out of the shards and runs alone: the `*StopSignalProcess` suites.
  # A test that counts periods in a window of wall time measures the host as well: it
  # is left out too and runs with no other test beside it (RUN_SERIAL below).
  set(HYPHA_WALL_TIME_TESTS ClockDriverFixture.PacesToTheRealTimeFactor)
  string(REPLACE ";" ":" HYPHA_WALL_TIME_FILTER "${HYPHA_WALL_TIME_TESTS}")
  set(HYPHA_SHARD_FILTER "-*StopSignalProcess.*:${HYPHA_WALL_TIME_FILTER}")
  foreach(shard RANGE ${HYPHA_LAST_TEST_SHARD})
    add_test(NAME test_hypha_${shard} COMMAND test_hypha --gtest_filter=${HYPHA_SHARD_FILTER})
    set_tests_properties(test_hypha_${shard} PROPERTIES LABELS "needs-iceoryx2"
      ENVIRONMENT "GTEST_TOTAL_SHARDS=${HYPHA_TEST_SHARDS};GTEST_SHARD_INDEX=${shard}")
    hypha_gtest_report(test_hypha_${shard})
  endforeach()
  # The typed loan's contract over every loanable standard type, in its own program: it
  # publishes std_msgs/UInt8MultiArray under the default policy, which test_qos.cpp maps to
  # a policy of its own (QosOf), and one program must see one QosOf per type.
  add_executable(test_typed_loan_contract test/test_main.cpp test/test_typed_loan_contract.cpp)
  target_link_libraries(test_typed_loan_contract PRIVATE hypha GTest::gtest)
  target_compile_definitions(test_typed_loan_contract PRIVATE HYPHA_NO_LOGGING)
  add_test(NAME test_typed_loan_contract COMMAND test_typed_loan_contract)
  set_tests_properties(test_typed_loan_contract PROPERTIES LABELS "needs-iceoryx2")
  hypha_gtest_report(test_typed_loan_contract)
  # The MCAP replay of the testing component, in its own program: test_hypha links
  # nothing of hypha::testing_mcap, as an application that does not replay links nothing.
  if(HYPHA_BUILD_TESTING_MCAP)
    add_executable(test_hypha_mcap_replay
      test/test_main.cpp
      test/test_mcap_replay.cpp
    )
    target_link_libraries(test_hypha_mcap_replay PRIVATE hypha hypha::testing_mcap GTest::gtest)
    target_compile_definitions(test_hypha_mcap_replay PRIVATE
      HYPHA_NO_LOGGING
      HYPHA_TEST_DATA_DIR="${CMAKE_CURRENT_SOURCE_DIR}/test/data"
    )
    add_test(NAME test_hypha_mcap_replay COMMAND test_hypha_mcap_replay)
    set_tests_properties(test_hypha_mcap_replay PROPERTIES LABELS "needs-iceoryx2")
    hypha_gtest_report(test_hypha_mcap_replay)
    # `hypha record` stores the bytes of a typed loan shrunk at the publish.
    if(HYPHA_BUILD_APPS)
      add_executable(test_typed_loan_record test/test_main.cpp test/test_typed_loan_record.cpp)
      target_link_libraries(test_typed_loan_record PRIVATE hypha hypha::testing_mcap GTest::gtest)
      target_compile_definitions(test_typed_loan_record PRIVATE
        HYPHA_NO_LOGGING HYPHA_TEST_HYPHA_TOOL="$<TARGET_FILE:hypha::hypha>")
      add_test(NAME test_typed_loan_record COMMAND test_typed_loan_record)
      set_tests_properties(test_typed_loan_record PROPERTIES LABELS "needs-iceoryx2" TIMEOUT 120)
      hypha_gtest_report(test_typed_loan_record)
    endif()
  endif()
  foreach(alone
      NodeStopSignalProcess.StopSignalEndsTheRun
      NodeStopSignalProcess.ANewNodeAfterAStopSignalIsStoppedFromTheStart
      CycleShutdownStopSignalProcess.NodeDestructionReleasesCyclesBeforeTheSession)
    add_test(NAME test_hypha_${alone} COMMAND test_hypha --gtest_filter=${alone})
    set_tests_properties(test_hypha_${alone} PROPERTIES LABELS "needs-iceoryx2")
    hypha_gtest_report(test_hypha_${alone})
  endforeach()
  foreach(wall_time ${HYPHA_WALL_TIME_TESTS})
    add_test(NAME test_hypha_${wall_time} COMMAND test_hypha --gtest_filter=${wall_time})
    set_tests_properties(test_hypha_${wall_time} PROPERTIES LABELS "needs-iceoryx2;wall-time"
      RUN_SERIAL TRUE)
    hypha_gtest_report(test_hypha_${wall_time})
  endforeach()
endif()

# The typed loan's tests under AddressSanitizer and UBSan, on request: a moved-from loan,
# a loan that outlives its publisher or its node, a span past the publish.
option(HYPHA_TYPED_LOAN_SANITIZE "Build test_typed_loan_sanitized with ASan and UBSan" OFF)
if(HYPHA_BUILD_TESTS AND HYPHA_TYPED_LOAN_SANITIZE)
  add_executable(test_typed_loan_sanitized
    test/test_main.cpp test/test_typed_loan.cpp test/test_typed_loan_contract.cpp)
  target_link_libraries(test_typed_loan_sanitized PRIVATE hypha GTest::gtest)
  target_compile_definitions(test_typed_loan_sanitized PRIVATE HYPHA_NO_LOGGING)
  target_compile_options(test_typed_loan_sanitized PRIVATE
    -fsanitize=address,undefined -fno-sanitize-recover=undefined -fno-omit-frame-pointer -g)
  target_link_options(test_typed_loan_sanitized PRIVATE -fsanitize=address,undefined)
endif()
