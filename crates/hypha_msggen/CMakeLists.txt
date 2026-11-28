# Builds the hypha_msggen CLI and exposes hypha_generate_messages(), the
# CMake-time message-generation step.
#
# The function itself lives in cmake/hyphaMessages.cmake, which hypha-cxx also
# installs beside hyphaConfig.cmake.

cmake_minimum_required(VERSION 3.16)

find_program(CARGO_EXECUTABLE cargo)
if(NOT CARGO_EXECUTABLE)
  message(STATUS "cargo not found; hypha_generate_messages() is not available")
  return()
endif()

# Cargo's debug profile only for a Debug build: RelWithDebInfo is still optimised.
if(CMAKE_BUILD_TYPE STREQUAL "Debug")
  set(HYPHA_MSGGEN_PROFILE_FLAG "")
  set(HYPHA_MSGGEN_PROFILE_DIR debug)
else()
  set(HYPHA_MSGGEN_PROFILE_FLAG --release)
  set(HYPHA_MSGGEN_PROFILE_DIR release)
endif()

set(HYPHA_MSGGEN_BIN
    "${HYPHA_CARGO_TARGET_DIR}/${HYPHA_MSGGEN_PROFILE_DIR}/hypha_msggen"
    CACHE INTERNAL "Path to the built hypha_msggen binary")

# The binary is a byproduct, so a generation rule may depend on the file: headers are
# regenerated when the generator changes, not only when a .msg file does.
add_custom_target(hypha_msggen_cargo
  COMMAND "${CMAKE_COMMAND}" -E true
  BYPRODUCTS "${HYPHA_MSGGEN_BIN}"
  VERBATIM)
add_dependencies(hypha_msggen_cargo hypha_ffi_cargo)

# crates/hypha-cxx installs this binary.

# The two things hyphaMessages.cmake resolves differently in a source tree than
# from an installed prefix. hyphaConfig.cmake sets the same two names there.
add_executable(hypha::msggen IMPORTED GLOBAL)
set_target_properties(hypha::msggen PROPERTIES IMPORTED_LOCATION "${HYPHA_MSGGEN_BIN}")

set(HYPHA_STANDARD_MSG_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../hypha_msgs/msg"
    CACHE INTERNAL "hypha's canonical .msg definitions")

include("${CMAKE_CURRENT_SOURCE_DIR}/cmake/hyphaMessages.cmake")
