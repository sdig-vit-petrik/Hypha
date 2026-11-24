# A complete node pair: one publishes a speed, the other watches it
# for staleness and an over-limit condition. Start with this one.
#
# Self-contained: this builds inside the hypha tree, and equally well on its
# own against an installed SDK.
#
#   cmake -S . -B build && cmake --build build

cmake_minimum_required(VERSION 3.20)
project(speed VERSION 0.3.0 LANGUAGES CXX)

include(GNUInstallDirs)

# Already defined when this builds as part of the hypha tree. Standalone, the
# installed package supplies it.
if(NOT TARGET hypha::sdk)
  find_package(hypha CONFIG REQUIRED)
endif()

add_executable(hypha_speed_monitor src/speed_monitor.cpp)
target_include_directories(hypha_speed_monitor PRIVATE inc)
target_link_libraries(hypha_speed_monitor PRIVATE hypha::sdk)
target_compile_features(hypha_speed_monitor PRIVATE cxx_std_23)

add_executable(hypha_speed_source src/speed_source.cpp)
target_include_directories(hypha_speed_source PRIVATE inc)
target_link_libraries(hypha_speed_source PRIVATE hypha::sdk)
target_compile_features(hypha_speed_source PRIVATE cxx_std_23)


# The SDK is a shared library, so an installed binary has to be able to find
# it. This installs only the example, never the framework it consumes.
if(TARGET hypha::ffi)
  get_target_property(HYPHA_FFI_LIBRARY hypha::ffi IMPORTED_LOCATION)
  if(HYPHA_FFI_LIBRARY)
    get_filename_component(HYPHA_FFI_LIBRARY_DIR "${HYPHA_FFI_LIBRARY}" DIRECTORY)
    set_target_properties(hypha_speed_monitor hypha_speed_source PROPERTIES
      INSTALL_RPATH "${HYPHA_FFI_LIBRARY_DIR}")
  endif()
endif()

install(TARGETS hypha_speed_monitor hypha_speed_source RUNTIME DESTINATION "${CMAKE_INSTALL_BINDIR}")

install(DIRECTORY config/ DESTINATION "${CMAKE_INSTALL_DATADIR}/hypha-examples/speed")
