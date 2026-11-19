# Runs the node NODE with the parameter file of CASE and fails unless it exits with
# EXPECTED. ctest -P passes NODE, CASE, EXPECTED and WORK_DIR.
#
#   missing  a parameter file that does not exist
#   unread   a key nobody reads
#   throw    a timer callback that throws in the run
#   clean    a stop from a timer callback

set(parameter_file "${WORK_DIR}/run_node_exit_code_${CASE}.toml")
file(REMOVE "${parameter_file}")
if(CASE STREQUAL "unread")
  file(WRITE "${parameter_file}" "[k8]\nending = \"stop\"\ntypo_key = 1\n")
elseif(CASE STREQUAL "throw")
  file(WRITE "${parameter_file}" "[k8]\nending = \"throw\"\n")
elseif(CASE STREQUAL "clean")
  file(WRITE "${parameter_file}" "[k8]\nending = \"stop\"\n")
elseif(NOT CASE STREQUAL "missing")
  message(FATAL_ERROR "unknown case '${CASE}'")
endif()

execute_process(
  COMMAND "${NODE}" "${parameter_file}"
  RESULT_VARIABLE exit_code
  OUTPUT_VARIABLE output
  ERROR_VARIABLE output
  TIMEOUT 30
)
file(REMOVE "${parameter_file}")
if(NOT exit_code STREQUAL "${EXPECTED}")
  message(FATAL_ERROR "case ${CASE}: exit code '${exit_code}', expected ${EXPECTED}\n${output}")
endif()
