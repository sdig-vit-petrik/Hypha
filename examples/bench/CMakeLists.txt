# Throughput and latency benchmark for the C++ SDK.
#
# Self-contained: this builds inside the hypha tree, and equally well on its
# own against an installed SDK.
#
#   cmake -S . -B build && cmake --build build

cmake_minimum_required(VERSION 3.20)
project(bench VERSION 0.1.0 LANGUAGES CXX)

include(GNUInstallDirs)

# Already defined when this builds as part of the hypha tree. Standalone, the
# installed package supplies it.
if(NOT TARGET hypha::sdk)
  find_package(hypha CONFIG REQUIRED)
endif()

add_executable(hypha_bench src/bench.cpp)
target_include_directories(hypha_bench PRIVATE inc)
target_link_libraries(hypha_bench PRIVATE hypha::sdk)
target_compile_features(hypha_bench PRIVATE cxx_std_23)


# The SDK is a shared library, so an installed binary has to be able to find
# it. This installs only the example, never the framework it consumes.
if(TARGET hypha::ffi)
  get_target_property(HYPHA_FFI_LIBRARY hypha::ffi IMPORTED_LOCATION)
  if(HYPHA_FFI_LIBRARY)
    get_filename_component(HYPHA_FFI_LIBRARY_DIR "${HYPHA_FFI_LIBRARY}" DIRECTORY)
    set_target_properties(hypha_bench PROPERTIES
      INSTALL_RPATH "${HYPHA_FFI_LIBRARY_DIR}")
  endif()
endif()

install(TARGETS hypha_bench RUNTIME DESTINATION "${CMAKE_INSTALL_BINDIR}")
