# Generates C++ headers for the supported ROS 1 message definitions from the
# canonical .msg files in msg/. No ROS installation is used.

cmake_minimum_required(VERSION 3.16)
project(hypha_msgs VERSION 0.2.0 LANGUAGES CXX)

if(NOT COMMAND hypha_generate_messages)
  message(STATUS "hypha_msgs: hypha_generate_messages unavailable; skipping C++ header generation")
  return()
endif()

# The headers land at <package>/<Name>.h in the package directories that
# crates/hypha-cxx owns and installs: build tree only, at the location hypha-cxx
# reads.
set(HYPHA_MSGS_OUT_DIR "${CMAKE_BINARY_DIR}/generated/include")
set(HYPHA_MSGS_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/msg")

# One call per package; DEPENDS names the packages a package's fields refer to,
# so each is declared after the ones it needs.
hypha_generate_messages(PACKAGE std_msgs
  MSG_DIR "${HYPHA_MSGS_ROOT}/std_msgs" OUT_DIR "${HYPHA_MSGS_OUT_DIR}")
hypha_generate_messages(PACKAGE geometry_msgs
  MSG_DIR "${HYPHA_MSGS_ROOT}/geometry_msgs" OUT_DIR "${HYPHA_MSGS_OUT_DIR}"
  DEPENDS std_msgs)
hypha_generate_messages(PACKAGE diagnostic_msgs
  MSG_DIR "${HYPHA_MSGS_ROOT}/diagnostic_msgs" OUT_DIR "${HYPHA_MSGS_OUT_DIR}"
  DEPENDS std_msgs)
hypha_generate_messages(PACKAGE rosgraph_msgs
  MSG_DIR "${HYPHA_MSGS_ROOT}/rosgraph_msgs" OUT_DIR "${HYPHA_MSGS_OUT_DIR}"
  DEPENDS std_msgs)
hypha_generate_messages(PACKAGE nav_msgs
  MSG_DIR "${HYPHA_MSGS_ROOT}/nav_msgs" OUT_DIR "${HYPHA_MSGS_OUT_DIR}"
  DEPENDS std_msgs geometry_msgs)
hypha_generate_messages(PACKAGE sensor_msgs
  MSG_DIR "${HYPHA_MSGS_ROOT}/sensor_msgs" OUT_DIR "${HYPHA_MSGS_OUT_DIR}"
  DEPENDS std_msgs geometry_msgs)
hypha_generate_messages(PACKAGE tf2_msgs
  MSG_DIR "${HYPHA_MSGS_ROOT}/tf2_msgs" OUT_DIR "${HYPHA_MSGS_OUT_DIR}"
  DEPENDS geometry_msgs)

hypha_generate_messages(PACKAGE hypha_msgs
  MSG_DIR "${HYPHA_MSGS_ROOT}/hypha_msgs" OUT_DIR "${HYPHA_MSGS_OUT_DIR}")

# Every standard package. The name differs from the package hypha_msgs above,
# which holds hypha's own messages.
add_library(hypha_standard_messages INTERFACE)
target_link_libraries(hypha_standard_messages INTERFACE
  std_msgs geometry_msgs diagnostic_msgs rosgraph_msgs nav_msgs sensor_msgs tf2_msgs
  hypha_msgs)
