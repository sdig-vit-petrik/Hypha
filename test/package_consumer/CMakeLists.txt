cmake_minimum_required(VERSION 3.20)
project(hypha_package_consumer LANGUAGES CXX)

# Two ways an application gets hypha: an installed prefix, or the source tree
# built inside its own project (FetchContent or add_subdirectory).
if(HYPHA_SOURCE_DIR)
  set(HYPHA_BUILD_TESTS OFF)
  set(HYPHA_BUILD_APPS OFF)
  set(HYPHA_BUILD_EXAMPLES OFF)
  add_subdirectory("${HYPHA_SOURCE_DIR}" hypha)
else()
  find_package(hypha CONFIG REQUIRED)
endif()

# An application's own message package, generated from the install prefix and
# nothing else. Nothing here names a hypha source checkout, which is the
# property under test.
hypha_generate_messages(PACKAGE consumer_msgs
  MSG_DIR "${CMAKE_CURRENT_SOURCE_DIR}/msgs/consumer_msgs"
  SRV_DIR "${CMAKE_CURRENT_SOURCE_DIR}/srvs/consumer_msgs"
  DEPENDS std_msgs)

# ROS 2 names for a standard package, one it reaches and the application's own, so that
# ros2_style.cpp includes nothing but ROS 2 paths.
hypha_generate_ros2_headers(consumer_ros2_headers
  PACKAGES std_msgs geometry_msgs consumer_msgs)

add_executable(hypha_package_consumer main.cpp ros2_style.cpp)
target_link_libraries(hypha_package_consumer PRIVATE hypha::sdk consumer_msgs consumer_ros2_headers)
