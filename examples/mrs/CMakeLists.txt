# The MRS UAV System on hypha: upstream MRS trackers, controllers and estimator
# inside native hypha CycleNode managers, a native multirotor simulator, and a
# mission that checks one takeoff and one goto. See README.md.
#
# Self-contained: builds against an installed SDK and Eigen 3.
#
#   cmake -S . -B build -DCMAKE_PREFIX_PATH=<sdk> && cmake --build build && ./run.sh build

cmake_minimum_required(VERSION 3.20)
project(mrs_on_hypha VERSION 0.2.0 LANGUAGES CXX)

if(NOT TARGET hypha::sdk)
  find_package(hypha CONFIG REQUIRED)
endif()
find_package(Eigen3 3.3 REQUIRED NO_MODULE)
enable_testing()

set(MRS_UPSTREAM_DIR "${CMAKE_CURRENT_SOURCE_DIR}/upstream")
set(MRS_MSG_OUT "${CMAKE_CURRENT_BINARY_DIR}/generated/include")

hypha_generate_messages(PACKAGE std_srvs
  SRV_DIR "${CMAKE_CURRENT_SOURCE_DIR}/msg/std_srvs/srv" OUT_DIR "${MRS_MSG_OUT}")
hypha_generate_messages(PACKAGE mrs_bridge
  MSG_DIR "${CMAKE_CURRENT_SOURCE_DIR}/msg/mrs_bridge/msg" OUT_DIR "${MRS_MSG_OUT}")
hypha_generate_messages(PACKAGE mrs_msgs
  MSG_DIR "${MRS_UPSTREAM_DIR}/mrs_msgs/msg" SRV_DIR "${MRS_UPSTREAM_DIR}/mrs_msgs/srv"
  OUT_DIR "${MRS_MSG_OUT}"
  DEPENDS std_msgs geometry_msgs sensor_msgs nav_msgs)

# Upstream code includes <mrs_msgs/UavState.h> and <geometry_msgs/Pose.h>: the package
# directories, generated and installed, go on the include path one level below hypha_msgs/.
get_target_property(MRS_SDK_INCLUDES hypha::sdk INTERFACE_INCLUDE_DIRECTORIES)
set(MRS_SDK_MSG_INCLUDES "")
foreach(dir ${MRS_SDK_INCLUDES})
  string(REGEX REPLACE "^\\$<BUILD_INTERFACE:(.*)>$" "\\1" dir "${dir}")
  string(REGEX REPLACE "^\\$<INSTALL_INTERFACE:.*>$" "" dir "${dir}")
  if(dir AND IS_DIRECTORY "${dir}/hypha_msgs")
    list(APPEND MRS_SDK_MSG_INCLUDES "${dir}/hypha_msgs")
  endif()
endforeach()
if(NOT MRS_SDK_MSG_INCLUDES)
  message(FATAL_ERROR "examples/mrs: no hypha_msgs directory on the hypha::sdk include path")
endif()
add_library(mrs_includes INTERFACE)
target_include_directories(mrs_includes INTERFACE "${MRS_MSG_OUT}/hypha_msgs" ${MRS_SDK_MSG_INCLUDES})
target_link_libraries(mrs_includes INTERFACE mrs_msgs mrs_bridge std_srvs hypha::sdk Eigen3::Eigen)
target_compile_features(mrs_includes INTERFACE cxx_std_23)

# One header that includes every mrs_msgs header, for the compile check.
file(GLOB MRS_MSG_DEFS "${MRS_UPSTREAM_DIR}/mrs_msgs/msg/*.msg" "${MRS_UPSTREAM_DIR}/mrs_msgs/srv/*.srv")
set(MRS_ALL_MSGS "#pragma once\n")
foreach(def ${MRS_MSG_DEFS})
  get_filename_component(name "${def}" NAME_WE)
  string(APPEND MRS_ALL_MSGS "#include <mrs_msgs/${name}.h>\n")
endforeach()
file(CONFIGURE OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/test_include/all_mrs_msgs.h" CONTENT "${MRS_ALL_MSGS}")

# mrs_check(<name> SOURCES <file>... [LIBS <target>...]): one check executable, one CTest test.
function(mrs_check name)
  cmake_parse_arguments(ARG "" "" "SOURCES;LIBS" ${ARGN})
  add_executable(mrs_${name} ${ARG_SOURCES})
  target_include_directories(mrs_${name} PRIVATE test "${CMAKE_CURRENT_BINARY_DIR}/test_include")
  target_link_libraries(mrs_${name} PRIVATE mrs_includes ${ARG_LIBS})
  add_test(NAME mrs_${name} COMMAND mrs_${name})
endfunction()

mrs_check(messages_compile SOURCES test/messages_compile.cpp)

add_library(mrs_ros_shim STATIC shim/src/ros_shim.cpp)
target_include_directories(mrs_ros_shim PUBLIC shim/include)
# tf2's LinearMath (upstream/geometry2), for the shim's tf2_geometry_msgs.
target_include_directories(mrs_ros_shim PUBLIC "${MRS_UPSTREAM_DIR}/geometry2/tf2/include")
target_compile_definitions(mrs_ros_shim PUBLIC MRS_UPSTREAM_DIR="${MRS_UPSTREAM_DIR}")
target_link_libraries(mrs_ros_shim PUBLIC mrs_includes)

mrs_check(shim SOURCES test/test_shim.cpp LIBS mrs_ros_shim)

set(MRS_LIB "${MRS_UPSTREAM_DIR}/mrs_lib")
add_library(mrs_lib STATIC
  "${MRS_LIB}/src/attitude_converter/attitude_converter.cpp"
  "${MRS_LIB}/src/geometry/misc.cpp"
  "${MRS_LIB}/src/math/math.cpp"
  "${MRS_LIB}/src/utils/utils.cpp"
  "${MRS_LIB}/src/scope_timer/scope_timer.cpp"
  "${MRS_LIB}/src/safety_zone/safety_zone.cpp"
  "${MRS_LIB}/src/safety_zone/line_operations.cpp"
  "${MRS_LIB}/src/safety_zone/polygon/polygon.cpp")
target_include_directories(mrs_lib PUBLIC "${MRS_LIB}/include")
target_link_libraries(mrs_lib PUBLIC mrs_ros_shim)

mrs_check(mrs_lib_utils SOURCES test/test_mrs_lib_utils.cpp LIBS mrs_lib)

add_library(mrs_host STATIC host/src/plugin_host.cpp)
target_include_directories(mrs_host PUBLIC host/include)
target_link_libraries(mrs_host PUBLIC mrs_lib)

mrs_check(plugin_host SOURCES test/test_plugin_host.cpp LIBS mrs_host)
mrs_check(transformer SOURCES test/test_transformer.cpp LIBS mrs_host)

add_library(mrs_sim_model STATIC simulator/multirotor_model.cpp simulator/rate_controller.cpp simulator/simulator_core.cpp)
target_include_directories(mrs_sim_model PUBLIC simulator)
target_link_libraries(mrs_sim_model PUBLIC Eigen3::Eigen)
target_compile_features(mrs_sim_model PUBLIC cxx_std_23)

mrs_check(multirotor_model SOURCES test/test_multirotor_model.cpp LIBS mrs_sim_model)
mrs_check(simulator_node SOURCES test/test_simulator_node.cpp LIBS mrs_sim_model)

add_executable(mrs_simulator simulator/mrs_simulator.cpp)
target_link_libraries(mrs_simulator PRIVATE mrs_sim_model mrs_host)

add_executable(mrs_mission managers/mission.cpp)
target_link_libraries(mrs_mission PRIVATE mrs_host)

add_executable(mrs_teleop managers/teleop.cpp)
target_link_libraries(mrs_teleop PRIVATE mrs_host)

set(MRS_MANAGERS "${MRS_UPSTREAM_DIR}/mrs_uav_managers")
add_library(mrs_upstream_includes INTERFACE)
target_include_directories(mrs_upstream_includes INTERFACE
  "${MRS_MANAGERS}/include"
  "${MRS_UPSTREAM_DIR}/mrs_uav_state_estimators/include"
  "${MRS_UPSTREAM_DIR}/mrs_uav_controllers/include")
target_link_libraries(mrs_upstream_includes INTERFACE mrs_host)

add_executable(mrs_estimation_manager managers/estimation_manager.cpp
  "${MRS_MANAGERS}/src/estimation_manager/estimator.cpp"
  "${MRS_MANAGERS}/src/estimation_manager/estimators/state_estimator.cpp"
  "${MRS_UPSTREAM_DIR}/mrs_uav_state_estimators/src/estimators/state/ground_truth.cpp")
target_link_libraries(mrs_estimation_manager PRIVATE mrs_upstream_includes)

# The tracker and controller plugins. An object library, so every PLUGINLIB_EXPORT_CLASS
# registration is linked in.
add_library(mrs_control_plugins OBJECT
  "${MRS_MANAGERS}/src/null_tracker.cpp"
  "${MRS_MANAGERS}/src/control_manager/common/common.cpp"
  "${MRS_UPSTREAM_DIR}/mrs_uav_trackers/src/landoff_tracker/landoff_tracker.cpp"
  "${MRS_UPSTREAM_DIR}/mrs_uav_trackers/src/line_tracker/line_tracker.cpp"
  "${MRS_UPSTREAM_DIR}/mrs_uav_controllers/src/common.cpp"
  "${MRS_UPSTREAM_DIR}/mrs_uav_controllers/src/se3_controller.cpp"
  "${MRS_UPSTREAM_DIR}/mrs_uav_controllers/src/failsafe_controller.cpp")
target_link_libraries(mrs_control_plugins PUBLIC mrs_upstream_includes)

add_executable(mrs_control_manager managers/control_manager.cpp $<TARGET_OBJECTS:mrs_control_plugins>)
target_link_libraries(mrs_control_manager PRIVATE mrs_upstream_includes)

add_executable(mrs_transform_manager managers/transform_manager.cpp)
target_link_libraries(mrs_transform_manager PRIVATE mrs_upstream_includes)

add_executable(mrs_uav_manager managers/uav_manager.cpp)
target_link_libraries(mrs_uav_manager PRIVATE mrs_upstream_includes)
