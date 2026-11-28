# hypha_add_node(<target> <sources...>)
#
# Creates an executable target for a hypha node linked against hypha::sdk
# (C++23, headers, and libhypha_ffi) and sets INSTALL_RPATH_USE_LINK_PATH so
# an installed binary finds libhypha_ffi (and any other linked shared
# libraries outside the build tree) without manual INSTALL_RPATH boilerplate
# or LD_LIBRARY_PATH / DYLD_LIBRARY_PATH.
#
#   hypha_add_node(my_node src/main.cpp)
#   target_link_libraries(my_node PRIVATE my_node_lib)
#   install(TARGETS my_node RUNTIME DESTINATION "${CMAKE_INSTALL_BINDIR}")
function(hypha_add_node target)
  if(NOT ARGN)
    message(FATAL_ERROR "hypha_add_node(${target}): at least one source file is required")
  endif()
  add_executable(${target} ${ARGN})
  target_link_libraries(${target} PRIVATE hypha::sdk)
  set_target_properties(${target} PROPERTIES INSTALL_RPATH_USE_LINK_PATH TRUE)
endfunction()
