# Large zero-copy payloads over shared memory, with a source and a sink
# that measure what the transport costs.
#
# Self-contained: this builds inside the hypha tree, and equally well on its
# own against an installed SDK.
#
#   cmake -S . -B build && cmake --build build

cmake_minimum_required(VERSION 3.20)
project(large_payload VERSION 0.3.0 LANGUAGES CXX)

include(GNUInstallDirs)

# Already defined when this builds as part of the hypha tree. Standalone, the
# installed package supplies it.
if(NOT TARGET hypha::sdk)
  find_package(hypha CONFIG REQUIRED)
endif()

add_executable(hypha_large_source src/large_source.cpp)
target_include_directories(hypha_large_source PRIVATE inc)
target_link_libraries(hypha_large_source PRIVATE hypha::sdk)
target_compile_features(hypha_large_source PRIVATE cxx_std_23)

add_executable(hypha_large_sink src/large_sink.cpp)
target_include_directories(hypha_large_sink PRIVATE inc)
target_link_libraries(hypha_large_sink PRIVATE hypha::sdk)
target_compile_features(hypha_large_sink PRIVATE cxx_std_23)

# What a camera image or a lidar cloud costs to publish and receive, by path.
add_executable(hypha_loan_bench src/loan_bench.cpp)
target_include_directories(hypha_loan_bench PRIVATE inc)
target_link_libraries(hypha_loan_bench PRIVATE hypha::sdk)
target_compile_features(hypha_loan_bench PRIVATE cxx_std_23)

# The SDK is a shared library, so an installed binary has to be able to find
# it. This installs only the example, never the framework it consumes.
if(TARGET hypha::ffi)
  get_target_property(HYPHA_FFI_LIBRARY hypha::ffi IMPORTED_LOCATION)
  if(HYPHA_FFI_LIBRARY)
    get_filename_component(HYPHA_FFI_LIBRARY_DIR "${HYPHA_FFI_LIBRARY}" DIRECTORY)
    set_target_properties(hypha_large_source hypha_large_sink hypha_loan_bench PROPERTIES
      INSTALL_RPATH "${HYPHA_FFI_LIBRARY_DIR}")
  endif()
endif()

install(TARGETS hypha_large_source hypha_large_sink hypha_loan_bench RUNTIME DESTINATION "${CMAKE_INSTALL_BINDIR}")
