# hypha_generate_messages(PACKAGE <name>
#                         [MSG_DIR <dir>] [SRV_DIR <dir>]
#                         [DEPENDS <package>...] [OUT_DIR <dir>])
#
# Declares one message package and turns its .msg and .srv files into C++
# headers. It defines <name>, an INTERFACE library carrying the include path and
# the build-order dependency, so a consumer links it and needs nothing else:
#
#   hypha_generate_messages(PACKAGE geometry_msgs
#     MSG_DIR ${CMAKE_SOURCE_DIR}/msg/common_msgs/geometry_msgs/msg
#     DEPENDS std_msgs)
#   target_link_libraries(app PRIVATE geometry_msgs)
#
# The package name is the one given here; the directory names carry no meaning.
# MSG_DIR and SRV_DIR each hold the definitions directly; a subdirectory is not
# read. A repository with several packages gets one call per package.
#
# DEPENDS names the packages whose types this package's fields refer to. Each
# one is declared by an earlier call, or is one of hypha's standard packages
# (std_msgs, geometry_msgs, ...). Their definitions are read, never re-emitted.
#
# OUT_DIR defaults to ${CMAKE_CURRENT_BINARY_DIR}/generated/include. Headers
# land at their ROS 1 path, <OUT_DIR>/<package>/<Name>.h, and OUT_DIR itself is
# what goes on the include path: #include <app_msgs/Reading.h>.
#
# Each package directory has one owner. hypha's standard packages belong to the
# installed SDK, so a second declaration of one from another directory is
# refused, as is any package declared twice.
#
# hypha_generate_ros2_headers(<target> PACKAGES <package>... [OUT_DIR <dir>])
#
# Generates ROS 2 alias headers for declared or standard packages: for each
# message <package>/msg/<name>.hpp, for each service <package>/srv/<name>.hpp,
# with <name> the rosidl spelling (Vector3Stamped is vector3_stamped). Each
# includes the ROS 1 header and declares <package>::msg (and ::srv) as a name of
# <package>, for its package and every package its fields reach; a service
# header adds <Name>_Request and <Name>_Response. A type that reaches a time or
# duration field also names those types builtin_interfaces::msg::Time and
# Duration. The type stays the ROS 1 one, with its fields, its time stamps and
# its Ptr, ConstPtr, SharedPtr, UniquePtr and <Name>_<Allocator> names. It
# defines <target>, an INTERFACE
# library with the include path; link it beside the packages:
#
#   hypha_generate_ros2_headers(app_ros2_headers PACKAGES std_msgs app_msgs)
#   target_link_libraries(app PRIVATE app_ros2_headers)
#   #include <std_msgs/msg/header.hpp>      // std_msgs::msg::Header
#
# The headers are the application's own: hypha installs none. OUT_DIR defaults
# to ${CMAKE_CURRENT_BINARY_DIR}/generated/ros2.

# Every declared package is recorded by name, so DEPENDS can find its
# definitions and the packages those definitions refer to in turn.
function(_hypha_register_message_package package msg_dir closure)
  get_filename_component(msg_dir "${msg_dir}" ABSOLUTE)
  get_property(known GLOBAL PROPERTY HYPHA_MSG_DIR_${package})
  if(known AND NOT known STREQUAL msg_dir)
    message(FATAL_ERROR
      "hypha_generate_messages: package '${package}' is declared twice: "
      "'${known}' and '${msg_dir}'")
  endif()
  set_property(GLOBAL PROPERTY HYPHA_MSG_DIR_${package} "${msg_dir}")
  set_property(GLOBAL PROPERTY HYPHA_MSG_CLOSURE_${package} "${closure}")
endfunction()

# hypha's standard packages are read from their installed definitions, so an
# application names std_msgs without declaring it. Their own dependencies are
# not recorded in the definitions, so each stands for all of them.
if(HYPHA_STANDARD_MSG_DIR AND IS_DIRECTORY "${HYPHA_STANDARD_MSG_DIR}")
  file(GLOB standard_entries LIST_DIRECTORIES true "${HYPHA_STANDARD_MSG_DIR}/*")
  set(standard_packages "")
  foreach(entry ${standard_entries})
    if(IS_DIRECTORY "${entry}")
      get_filename_component(standard_name "${entry}" NAME)
      list(APPEND standard_packages "${standard_name}")
    endif()
  endforeach()
  foreach(standard_name ${standard_packages})
    set(standard_closure "${standard_packages}")
    list(REMOVE_ITEM standard_closure "${standard_name}")
    _hypha_register_message_package(
      "${standard_name}" "${HYPHA_STANDARD_MSG_DIR}/${standard_name}" "${standard_closure}")
  endforeach()
endif()

function(hypha_generate_messages)
  cmake_parse_arguments(ARG "" "PACKAGE;MSG_DIR;SRV_DIR;OUT_DIR" "DEPENDS" ${ARGN})

  if(NOT ARG_PACKAGE)
    message(FATAL_ERROR "hypha_generate_messages: PACKAGE is required")
  endif()
  if(NOT ARG_MSG_DIR AND NOT ARG_SRV_DIR)
    message(FATAL_ERROR "hypha_generate_messages: ${ARG_PACKAGE} needs MSG_DIR or SRV_DIR")
  endif()
  if(ARG_UNPARSED_ARGUMENTS)
    message(FATAL_ERROR "hypha_generate_messages: unrecognised argument(s): ${ARG_UNPARSED_ARGUMENTS}")
  endif()
  if(NOT TARGET hypha::msggen)
    message(FATAL_ERROR
      "hypha_generate_messages: the hypha_msggen generator was not found. "
      "An installed hypha provides it at <prefix>/bin/hypha_msggen.")
  endif()

  if(NOT ARG_OUT_DIR)
    set(ARG_OUT_DIR "${CMAKE_CURRENT_BINARY_DIR}/generated/include")
  endif()

  set(msg_files "")
  if(ARG_MSG_DIR)
    file(GLOB msg_files CONFIGURE_DEPENDS "${ARG_MSG_DIR}/*.msg")
  endif()
  set(srv_files "")
  if(ARG_SRV_DIR)
    file(GLOB srv_files CONFIGURE_DEPENDS "${ARG_SRV_DIR}/*.srv")
  endif()
  if(NOT msg_files AND NOT srv_files)
    message(FATAL_ERROR
      "hypha_generate_messages: no .msg or .srv files for package ${ARG_PACKAGE} "
      "in '${ARG_MSG_DIR}' '${ARG_SRV_DIR}'")
  endif()

  set(generated_headers "")
  foreach(msg_file ${msg_files})
    get_filename_component(message_name "${msg_file}" NAME_WE)
    list(APPEND generated_headers "${ARG_OUT_DIR}/${ARG_PACKAGE}/${message_name}.h")
  endforeach()
  foreach(srv_file ${srv_files})
    get_filename_component(service_name "${srv_file}" NAME_WE)
    list(APPEND generated_headers
      "${ARG_OUT_DIR}/${ARG_PACKAGE}/${service_name}Request.h"
      "${ARG_OUT_DIR}/${ARG_PACKAGE}/${service_name}Response.h"
      "${ARG_OUT_DIR}/${ARG_PACKAGE}/${service_name}.h")
  endforeach()

  set(generator_args --package "${ARG_PACKAGE}" --out-dir "${ARG_OUT_DIR}")
  if(ARG_MSG_DIR)
    list(APPEND generator_args --msg-dir "${ARG_MSG_DIR}")
  endif()
  if(ARG_SRV_DIR)
    list(APPEND generator_args --srv-dir "${ARG_SRV_DIR}")
  endif()

  # The closure holds every package a field can reach, however deep.
  set(closure "")
  foreach(dependency ${ARG_DEPENDS})
    get_property(dependency_dir GLOBAL PROPERTY HYPHA_MSG_DIR_${dependency})
    if(NOT dependency_dir)
      message(FATAL_ERROR
        "hypha_generate_messages: ${ARG_PACKAGE} depends on '${dependency}', which is "
        "not a declared message package. Declare it with hypha_generate_messages() "
        "before this call. hypha's standard packages come with an installed hypha.")
    endif()
    get_property(dependency_closure GLOBAL PROPERTY HYPHA_MSG_CLOSURE_${dependency})
    list(APPEND closure "${dependency}" ${dependency_closure})
  endforeach()
  list(REMOVE_DUPLICATES closure)

  set(dependency_files "")
  foreach(dependency ${closure})
    get_property(dependency_dir GLOBAL PROPERTY HYPHA_MSG_DIR_${dependency})
    list(APPEND generator_args --dependency "${dependency}=${dependency_dir}")
    file(GLOB dependency_msgs CONFIGURE_DEPENDS "${dependency_dir}/*.msg")
    list(APPEND dependency_files ${dependency_msgs})
  endforeach()

  if(ARG_MSG_DIR)
    _hypha_register_message_package("${ARG_PACKAGE}" "${ARG_MSG_DIR}" "${closure}")
  endif()
  # For hypha_generate_ros2_headers(); DEPENDS reads the .msg definitions alone.
  if(ARG_SRV_DIR)
    get_filename_component(srv_dir "${ARG_SRV_DIR}" ABSOLUTE)
    set_property(GLOBAL PROPERTY HYPHA_SRV_DIR_${ARG_PACKAGE} "${srv_dir}")
    set_property(GLOBAL PROPERTY HYPHA_SRV_CLOSURE_${ARG_PACKAGE} "${closure}")
  endif()

  # hypha_msggen_cargo exists in the hypha source tree only: there, the
  # generator is built by this build rather than found already installed.
  set(generator_dependency "")
  if(TARGET hypha_msggen_cargo)
    set(generator_dependency hypha_msggen_cargo)
  endif()

  set(inputs_file "${CMAKE_CURRENT_BINARY_DIR}/hypha_msggen/${ARG_PACKAGE}.inputs")
  string(REPLACE ";" "\n" inputs_text
    "$<TARGET_FILE:hypha::msggen>;${generator_args};${msg_files};${srv_files};${dependency_files}")
  file(GENERATE OUTPUT "${inputs_file}" CONTENT "${inputs_text}\n")

  add_custom_command(
    OUTPUT ${generated_headers}
    COMMAND hypha::msggen ${generator_args}
    DEPENDS ${msg_files} ${srv_files} ${dependency_files} ${generator_dependency}
            "$<TARGET_FILE:hypha::msggen>" "${inputs_file}"
    COMMENT "Generating the ${ARG_PACKAGE} C++ message and service headers"
    VERBATIM)

  # ALL, not the INTERFACE library's dependency alone: an INTERFACE library is
  # not itself built, so nothing would pull the generation in for a consumer
  # that only carries the headers on its include path.
  add_custom_target(${ARG_PACKAGE}_generate ALL DEPENDS ${generated_headers})

  add_library(${ARG_PACKAGE} INTERFACE)
  target_include_directories(${ARG_PACKAGE} INTERFACE $<BUILD_INTERFACE:${ARG_OUT_DIR}>)
  add_dependencies(${ARG_PACKAGE} ${ARG_PACKAGE}_generate)
  foreach(dependency ${ARG_DEPENDS})
    if(TARGET ${dependency})
      target_link_libraries(${ARG_PACKAGE} INTERFACE ${dependency})
    endif()
  endforeach()
endfunction()

# The stem of a ROS 2 header, by rosidl's rule; hypha_msggen's ros2_header_stem
# is the same rule.
function(_hypha_ros2_header_stem name out_var)
  string(REGEX REPLACE "(.)([A-Z][a-z]+)" "\\1_\\2" stem "${name}")
  string(REGEX REPLACE "([a-z0-9])([A-Z])" "\\1_\\2" stem "${stem}")
  string(TOLOWER "${stem}" stem)
  set(${out_var} "${stem}" PARENT_SCOPE)
endfunction()

function(hypha_generate_ros2_headers target)
  cmake_parse_arguments(ARG "" "OUT_DIR" "PACKAGES" ${ARGN})
  if(NOT ARG_PACKAGES)
    message(FATAL_ERROR "hypha_generate_ros2_headers: ${target} needs PACKAGES")
  endif()
  if(ARG_UNPARSED_ARGUMENTS)
    message(FATAL_ERROR
      "hypha_generate_ros2_headers: unrecognised argument(s): ${ARG_UNPARSED_ARGUMENTS}")
  endif()
  if(NOT TARGET hypha::msggen)
    message(FATAL_ERROR "hypha_generate_ros2_headers: the hypha_msggen generator was not found.")
  endif()
  if(NOT ARG_OUT_DIR)
    set(ARG_OUT_DIR "${CMAKE_CURRENT_BINARY_DIR}/generated/ros2")
  endif()
  set(generator_dependency "")
  if(TARGET hypha_msggen_cargo)
    set(generator_dependency hypha_msggen_cargo)
  endif()

  set(all_headers "")
  set(package_targets "")
  foreach(package ${ARG_PACKAGES})
    get_property(msg_dir GLOBAL PROPERTY HYPHA_MSG_DIR_${package})
    get_property(srv_dir GLOBAL PROPERTY HYPHA_SRV_DIR_${package})
    if(NOT msg_dir AND NOT srv_dir)
      message(FATAL_ERROR
        "hypha_generate_ros2_headers: '${package}' is not a declared message package. "
        "Declare it with hypha_generate_messages() before this call.")
    endif()
    set(generator_args --package "${package}" --out-dir "${ARG_OUT_DIR}" --language ros2)
    set(definition_files "")
    set(headers "")
    if(msg_dir)
      get_property(closure GLOBAL PROPERTY HYPHA_MSG_CLOSURE_${package})
      list(APPEND generator_args --msg-dir "${msg_dir}")
      file(GLOB msg_files CONFIGURE_DEPENDS "${msg_dir}/*.msg")
      list(APPEND definition_files ${msg_files})
      foreach(msg_file ${msg_files})
        get_filename_component(name "${msg_file}" NAME_WE)
        _hypha_ros2_header_stem("${name}" stem)
        list(APPEND headers "${ARG_OUT_DIR}/${package}/msg/${stem}.hpp")
      endforeach()
    else()
      get_property(closure GLOBAL PROPERTY HYPHA_SRV_CLOSURE_${package})
    endif()
    if(srv_dir)
      list(APPEND generator_args --srv-dir "${srv_dir}")
      file(GLOB srv_files CONFIGURE_DEPENDS "${srv_dir}/*.srv")
      list(APPEND definition_files ${srv_files})
      foreach(srv_file ${srv_files})
        get_filename_component(name "${srv_file}" NAME_WE)
        _hypha_ros2_header_stem("${name}" stem)
        list(APPEND headers "${ARG_OUT_DIR}/${package}/srv/${stem}.hpp")
      endforeach()
    endif()
    foreach(dependency ${closure})
      get_property(dependency_dir GLOBAL PROPERTY HYPHA_MSG_DIR_${dependency})
      list(APPEND generator_args --dependency "${dependency}=${dependency_dir}")
      file(GLOB dependency_msgs CONFIGURE_DEPENDS "${dependency_dir}/*.msg")
      list(APPEND definition_files ${dependency_msgs})
    endforeach()

    add_custom_command(
      OUTPUT ${headers}
      COMMAND hypha::msggen ${generator_args}
      DEPENDS ${definition_files} ${generator_dependency} "$<TARGET_FILE:hypha::msggen>"
      COMMENT "Generating the ${package} ROS 2 alias headers"
      VERBATIM)
    list(APPEND all_headers ${headers})
    if(TARGET ${package})
      list(APPEND package_targets ${package})
    endif()
  endforeach()

  add_custom_target(${target}_generate ALL DEPENDS ${all_headers})
  add_library(${target} INTERFACE)
  target_include_directories(${target} INTERFACE $<BUILD_INTERFACE:${ARG_OUT_DIR}>)
  add_dependencies(${target} ${target}_generate)
  target_link_libraries(${target} INTERFACE ${package_targets})
endfunction()
