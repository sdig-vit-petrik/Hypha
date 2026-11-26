# Runs the node NODE with the parameter file of CASE and fails unless it exits with
# EXPECTED. ctest -P passes NODE, CASE, EXPECTED and WORK_DIR, and CONFIG (a valid
# parameter file of the node) for noports.
#
#   missing    a parameter file that does not exist
#   malformed  a parameter file that is not TOML
#   noports    a valid parameter file without its [ports.*] tables, so the first
#              endpoint finds no topic
#
# A configuration refused before the run is exit 78 (hypha::kExitConfiguration), which a
# supervisor never restarts; 70 would be restarted without end.

get_filename_component(node_name "${NODE}" NAME)
set(parameter_file "${WORK_DIR}/${node_name}_${CASE}.toml")
file(REMOVE "${parameter_file}")
if(CASE STREQUAL "malformed")
  file(WRITE "${parameter_file}" "report_period_ms = \n[unterminated\n")
elseif(CASE STREQUAL "noports")
  file(READ "${CONFIG}" parameters)
  string(REGEX REPLACE "\\[ports\\.[a-z]+\\.[a-z_]+\\]\n[^\n]*\n" "" parameters "${parameters}")
  file(WRITE "${parameter_file}" "${parameters}")
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
  message(FATAL_ERROR "${node_name} ${CASE}: exit code '${exit_code}', expected ${EXPECTED}\n${output}")
endif()
