# hypha_add_test(<name> [COMMAND <command> [<arg>...]] [WORKING_DIRECTORY <dir>])
#
# Adds the CTest test <name> that runs in a bus domain of its own, so tests that
# ctest runs in parallel, and the tests of two build trees on one host, share no
# node name, topic or /clock, and a test never meets a system running on the host.
# Without COMMAND the test runs the executable target <name>; a target named as
# the command runs its file. Every process the test starts inherits the domain.
#
# The domain is t_<pid of ctest>_<n>: the test root t_, the run, and the number of
# the test in this project. It is set when the test starts, so a test's ENVIRONMENT
# property cannot replace it, and it has at most 2 + 7 + 1 + 5 of the 32 characters
# a domain may have. `hypha clean --domain t_` removes the domains of the runs that
# ended.
#
#   hypha_add_test(my_node_test)
#   hypha_add_test(my_flight COMMAND my_flight_check --scenario takeoff)
function(hypha_add_test name)
  cmake_parse_arguments(ARG "" "WORKING_DIRECTORY" "COMMAND" ${ARGN})
  if(ARG_UNPARSED_ARGUMENTS)
    message(FATAL_ERROR "hypha_add_test: unrecognised argument(s): ${ARG_UNPARSED_ARGUMENTS}")
  endif()
  set(command ${ARG_COMMAND})
  if(NOT command)
    if(NOT TARGET ${name})
      message(FATAL_ERROR "hypha_add_test: ${name} is no target; name the test's COMMAND")
    endif()
    set(command ${name})
  endif()
  list(POP_FRONT command program)
  if(TARGET ${program})
    set(program "$<TARGET_FILE:${program}>")
  endif()

  get_property(count GLOBAL PROPERTY HYPHA_TEST_COUNT)
  if(NOT count)
    set(count 0)
  endif()
  math(EXPR count "${count} + 1")
  set_property(GLOBAL PROPERTY HYPHA_TEST_COUNT ${count})

  set(directory)
  if(ARG_WORKING_DIRECTORY)
    set(directory WORKING_DIRECTORY "${ARG_WORKING_DIRECTORY}")
  endif()
  # ctest starts the shell, so its parent is the ctest process of this run.
  add_test(NAME ${name}
    COMMAND /bin/sh -c "export HYPHA_DOMAIN=t_\${PPID}_${count}; exec \"$0\" \"$@\""
      "${program}" ${command}
    ${directory})
endfunction()
