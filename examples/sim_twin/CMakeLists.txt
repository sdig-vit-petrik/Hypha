# A simulated factory under simulated time: one clock node drives /clock with
# a scripted jump schedule, one twin node runs cycles and a timer on that clock.
#
# Self-contained: this builds inside the hypha tree, and equally well on its
# own against an installed SDK.
#
#   cmake -S . -B build && cmake --build build

cmake_minimum_required(VERSION 3.20)
project(sim_twin VERSION 0.2.0 LANGUAGES CXX)

include(GNUInstallDirs)

# Already defined when this builds as part of the hypha tree. Standalone, the
# installed package supplies it.
if(NOT TARGET hypha::sdk)
  find_package(hypha CONFIG REQUIRED)
endif()

add_executable(hypha_sim_clock src/sim_clock.cpp)
target_link_libraries(hypha_sim_clock PRIVATE hypha::sdk)
target_compile_features(hypha_sim_clock PRIVATE cxx_std_23)

add_executable(hypha_sim_twin src/sim_twin.cpp)
target_link_libraries(hypha_sim_twin PRIVATE hypha::sdk)
target_compile_features(hypha_sim_twin PRIVATE cxx_std_23)

# The SDK is a shared library, so an installed binary has to be able to find
# it. This installs only the example, never the framework it consumes.
if(TARGET hypha::ffi)
  get_target_property(HYPHA_FFI_LIBRARY hypha::ffi IMPORTED_LOCATION)
  if(HYPHA_FFI_LIBRARY)
    get_filename_component(HYPHA_FFI_LIBRARY_DIR "${HYPHA_FFI_LIBRARY}" DIRECTORY)
    set_target_properties(hypha_sim_clock hypha_sim_twin PROPERTIES
      INSTALL_RPATH "${HYPHA_FFI_LIBRARY_DIR}")
  endif()
endif()

install(TARGETS hypha_sim_clock hypha_sim_twin RUNTIME DESTINATION "${CMAKE_INSTALL_BINDIR}")

install(DIRECTORY config/ DESTINATION "${CMAKE_INSTALL_DATADIR}/hypha-examples/sim_twin")
