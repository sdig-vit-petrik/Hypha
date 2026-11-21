# Sensor fusion: N sensors as N L2 cycles in one process, and one fusion cycle
# that averages the usable inputs.
#
#   cmake -S . -B build && cmake --build build

cmake_minimum_required(VERSION 3.20)
project(sensor_fusion VERSION 0.2.0 LANGUAGES CXX)

include(GNUInstallDirs)

if(NOT TARGET hypha::sdk)
  find_package(hypha CONFIG REQUIRED)
endif()

set(SENSOR_FUSION_TARGETS hypha_fusion_sensors hypha_fusion sensor_fusion_probe)

add_executable(hypha_fusion_sensors src/sensors_main.cpp)
add_executable(hypha_fusion src/fusion_main.cpp)
add_executable(sensor_fusion_probe src/probe_main.cpp)

foreach(target IN LISTS SENSOR_FUSION_TARGETS)
  target_include_directories(${target} PRIVATE inc)
  target_link_libraries(${target} PRIVATE hypha::sdk)
  target_compile_features(${target} PRIVATE cxx_std_23)
  target_compile_options(${target} PRIVATE -Wall -Wextra -Wpedantic)
endforeach()

get_target_property(HYPHA_FFI_LIBRARY hypha::ffi IMPORTED_LOCATION)
get_filename_component(HYPHA_FFI_LIBRARY_DIR "${HYPHA_FFI_LIBRARY}" DIRECTORY)
foreach(target IN LISTS SENSOR_FUSION_TARGETS)
  set_target_properties(${target} PROPERTIES INSTALL_RPATH "${HYPHA_FFI_LIBRARY_DIR}")
endforeach()

install(TARGETS ${SENSOR_FUSION_TARGETS} RUNTIME DESTINATION "${CMAKE_INSTALL_BINDIR}")
