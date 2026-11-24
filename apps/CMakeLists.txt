# The command line tools: the one `hypha` executable (hypha topic, hypha play, ...),
# the ROS bridge and the boundary.

find_program(CARGO_EXECUTABLE cargo REQUIRED)

# Named, not globbed: a dropped binary fails the build instead of the device.
set(HYPHA_APP_BINARIES
    hypha
    hypha_ros_bridge)

# The old tool names. Each is a link to `hypha`, which runs the tool its name names.
set(HYPHA_TOOL_LINKS
    hypha_clean
    hypha_foxglove_bridge
    hypha_intercept
    hypha_launch
    hypha_node
    hypha_param
    hypha_play
    hypha_record
    hypha_top
    hypha_topic)

# Cargo's debug profile only for a Debug build: RelWithDebInfo is still optimised.
if(CMAKE_BUILD_TYPE STREQUAL "Debug")
  set(HYPHA_APP_PROFILE_DIR debug)
else()
  set(HYPHA_APP_PROFILE_DIR release)
endif()

set(HYPHA_APP_TARGET_DIR
    "${CMAKE_CURRENT_SOURCE_DIR}/../target/${HYPHA_APP_PROFILE_DIR}")
cmake_path(NORMAL_PATH HYPHA_APP_TARGET_DIR)

# hypha::<tool> (hypha::topic for hypha_topic, hypha::hypha for hypha) names each
# executable and each link, so an application runs $<TARGET_FILE:hypha::topic>
# without knowing cargo's layout or the prefix.
set(HYPHA_APP_OUTPUTS "")
set(tools_file "${CMAKE_CURRENT_BINARY_DIR}/hyphaTools.cmake")
file(WRITE "${tools_file}" "# The command line tools installed with the SDK.\n")
foreach(binary IN LISTS HYPHA_APP_BINARIES)
  set(output "${HYPHA_APP_TARGET_DIR}/${binary}")
  list(APPEND HYPHA_APP_OUTPUTS "${output}")
  string(REGEX REPLACE "^hypha_" "" tool "${binary}")
  add_executable(hypha::${tool} IMPORTED GLOBAL)
  set_target_properties(hypha::${tool} PROPERTIES IMPORTED_LOCATION "${output}")
  add_dependencies(hypha::${tool} hypha_ffi_cargo)
  file(APPEND "${tools_file}"
    "add_executable(hypha::${tool} IMPORTED)\n"
    "set_target_properties(hypha::${tool} PROPERTIES IMPORTED_LOCATION "
    "\"\${PACKAGE_PREFIX_DIR}/${CMAKE_INSTALL_BINDIR}/${binary}\")\n")
endforeach()

# The links of this tree, next to the built `hypha`, made after each cargo build.
set(HYPHA_TOOL_LINK_DIR "${CMAKE_CURRENT_BINARY_DIR}/tool-links")
set(link_commands "")
foreach(link IN LISTS HYPHA_TOOL_LINKS)
  list(APPEND link_commands COMMAND "${CMAKE_COMMAND}" -E create_symlink
       "${HYPHA_APP_TARGET_DIR}/hypha" "${HYPHA_TOOL_LINK_DIR}/${link}")
endforeach()
add_custom_target(hypha_tool_links ALL
  COMMAND "${CMAKE_COMMAND}" -E make_directory "${HYPHA_TOOL_LINK_DIR}"
  ${link_commands}
  COMMENT "Linking the old tool names to hypha"
  VERBATIM)
add_dependencies(hypha_tool_links hypha_ffi_cargo)

foreach(link IN LISTS HYPHA_TOOL_LINKS)
  string(REGEX REPLACE "^hypha_" "" tool "${link}")
  add_executable(hypha::${tool} IMPORTED GLOBAL)
  set_target_properties(hypha::${tool} PROPERTIES
    IMPORTED_LOCATION "${HYPHA_TOOL_LINK_DIR}/${link}")
  add_dependencies(hypha::${tool} hypha_tool_links)
  file(APPEND "${tools_file}"
    "add_executable(hypha::${tool} IMPORTED)\n"
    "set_target_properties(hypha::${tool} PROPERTIES IMPORTED_LOCATION "
    "\"\${PACKAGE_PREFIX_DIR}/${CMAKE_INSTALL_BINDIR}/${link}\")\n")
endforeach()

# Its own component, so that an image stage can install the tools apart from the SDK.
install(PROGRAMS ${HYPHA_APP_OUTPUTS} DESTINATION ${CMAKE_INSTALL_BINDIR} COMPONENT apps)
foreach(link IN LISTS HYPHA_TOOL_LINKS)
  install(CODE "file(CREATE_LINK hypha \"\$ENV{DESTDIR}\${CMAKE_INSTALL_PREFIX}/${CMAKE_INSTALL_BINDIR}/${link}\" SYMBOLIC)"
          COMPONENT apps)
endforeach()
install(FILES "${tools_file}" DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/hypha COMPONENT apps)
