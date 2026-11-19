# Builds the Rust C ABI library and exposes it as a CMake target.
#
# This one cargo call builds every Rust artifact of the tree (the packages come
# from HYPHA_CARGO_PACKAGE_FLAGS in the root project): separate calls with
# different -p sets unify features differently and compile the core chain once
# for each.

cmake_minimum_required(VERSION 3.20)

find_program(CARGO_EXECUTABLE cargo)
if(NOT CARGO_EXECUTABLE)
  message(STATUS "cargo not found; the Rust C ABI target is not built")
  return()
endif()

set(HYPHA_FFI_CRATES "${CMAKE_CURRENT_SOURCE_DIR}/../..")

# Cargo's debug profile only for a Debug build: RelWithDebInfo is still optimised.
if(CMAKE_BUILD_TYPE STREQUAL "Debug")
  set(HYPHA_FFI_PROFILE_FLAG "")
  set(HYPHA_FFI_PROFILE_DIR debug)
else()
  set(HYPHA_FFI_PROFILE_FLAG --release)
  set(HYPHA_FFI_PROFILE_DIR release)
endif()

set(HYPHA_FFI_LIB
    "${HYPHA_FFI_CRATES}/target/${HYPHA_FFI_PROFILE_DIR}/${CMAKE_SHARED_LIBRARY_PREFIX}hypha_ffi${CMAKE_SHARED_LIBRARY_SUFFIX}")
file(READ "${CMAKE_CURRENT_SOURCE_DIR}/../../abi.revision" HYPHA_ABI_REVISION)
string(STRIP "${HYPHA_ABI_REVISION}" HYPHA_ABI_REVISION)
if(NOT HYPHA_ABI_REVISION MATCHES "^[0-9]+$")
  message(FATAL_ERROR "abi.revision must contain one unsigned integer")
endif()
# The one place the revision becomes a file name; crates/hypha-cxx installs and
# exports the library under it.
if(APPLE)
  set(HYPHA_FFI_VERSIONED_NAME "libhypha_ffi.${HYPHA_ABI_REVISION}.dylib")
elseif(UNIX)
  set(HYPHA_FFI_VERSIONED_NAME "libhypha_ffi.so.${HYPHA_ABI_REVISION}")
else()
  set(HYPHA_FFI_VERSIONED_NAME "${CMAKE_SHARED_LIBRARY_PREFIX}hypha_ffi${CMAKE_SHARED_LIBRARY_SUFFIX}")
endif()
set(HYPHA_FFI_VERSIONED_NAME "${HYPHA_FFI_VERSIONED_NAME}" CACHE INTERNAL "")
if(UNIX)
  set(HYPHA_FFI_VERSIONED_FILE "${CMAKE_CURRENT_BINARY_DIR}/${HYPHA_FFI_VERSIONED_NAME}")
  if(IS_SYMLINK "${HYPHA_FFI_VERSIONED_FILE}")
    file(REMOVE "${HYPHA_FFI_VERSIONED_FILE}")
  elseif(EXISTS "${HYPHA_FFI_VERSIONED_FILE}")
    message(FATAL_ERROR "Refusing to replace non-symlink ${HYPHA_FFI_VERSIONED_FILE}")
  endif()
  file(CREATE_LINK "${HYPHA_FFI_LIB}" "${HYPHA_FFI_VERSIONED_FILE}" SYMBOLIC)
else()
  set(HYPHA_FFI_VERSIONED_FILE "${HYPHA_FFI_LIB}")
endif()

# The static library of the MCAP test reader (crate hypha_testing_mcap), with
# HYPHA_BUILD_TESTING_MCAP. Nothing here links it: crates/hypha-cxx wraps it as
# hypha::testing_mcap, which only a test that replays an MCAP file links.
set(HYPHA_CARGO_BYPRODUCTS "${HYPHA_FFI_LIB}")
if(HYPHA_BUILD_TESTING_MCAP)
  set(HYPHA_TESTING_MCAP_LIB
      "${HYPHA_FFI_CRATES}/target/${HYPHA_FFI_PROFILE_DIR}/${CMAKE_STATIC_LIBRARY_PREFIX}hypha_testing_mcap${CMAKE_STATIC_LIBRARY_SUFFIX}")
  list(APPEND HYPHA_CARGO_BYPRODUCTS "${HYPHA_TESTING_MCAP_LIB}")
endif()

add_custom_target(
  hypha_ffi_cargo ALL
  COMMAND ${CARGO_EXECUTABLE} build ${HYPHA_CARGO_PACKAGE_FLAGS} ${HYPHA_FFI_PROFILE_FLAG} --locked
  WORKING_DIRECTORY "${HYPHA_FFI_CRATES}"
  BYPRODUCTS ${HYPHA_CARGO_BYPRODUCTS}
  COMMENT "Building the hypha Rust C ABI, the message generator and the tools"
  VERBATIM)

# A build-tree-only handle on the artifact. crates/hypha-cxx wraps this into
# the installable, find_package()-able hypha::ffi target.
add_library(hypha_ffi SHARED IMPORTED GLOBAL)
add_dependencies(hypha_ffi hypha_ffi_cargo)
set_target_properties(hypha_ffi PROPERTIES
  IMPORTED_LOCATION "${HYPHA_FFI_VERSIONED_FILE}"
  HYPHA_RAW_LOCATION "${HYPHA_FFI_LIB}"
)
# The name build.rs links into the library. Stated here, CMake does not read it
# from the file, which a clean configure has not built yet; without it, every
# consumer of that configure links with no build rpath and cannot load the library.
if(APPLE)
  set_target_properties(hypha_ffi PROPERTIES IMPORTED_SONAME "@rpath/${HYPHA_FFI_VERSIONED_NAME}")
elseif(UNIX)
  set_target_properties(hypha_ffi PROPERTIES IMPORTED_SONAME "${HYPHA_FFI_VERSIONED_NAME}")
endif()
target_include_directories(hypha_ffi INTERFACE
  $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
)
target_link_libraries(hypha_ffi INTERFACE
  pthread dl m
)
target_compile_features(hypha_ffi INTERFACE cxx_std_17)

# A build-tree-only handle on the MCAP reader's static library, like hypha_ffi above.
if(HYPHA_BUILD_TESTING_MCAP)
  add_library(hypha_testing_mcap_archive STATIC IMPORTED GLOBAL)
  add_dependencies(hypha_testing_mcap_archive hypha_ffi_cargo)
  set_target_properties(hypha_testing_mcap_archive PROPERTIES
    IMPORTED_LOCATION "${HYPHA_TESTING_MCAP_LIB}"
  )
endif()
