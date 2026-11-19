#!/usr/bin/env bash
#
# @brief      Run one MRS scenario on hypha: start the simulator and the processes the
#             scenario needs, wait for the mission, stop the others, return its exit code.
#
# Usage: examples/mrs/run.sh <bin-dir> [scenario] [--checks]
#   scenario: estimation, takeoff, takeoff_via_uav_manager, takeoff_goto (default),
#             goto_outside_safety_area. teleop runs under hypha launch: launch.sh.
#   --checks: first run the mrs_ unit checks (ctest) of the build
# Logs: <bin-dir>/logs/<process>.log
set -u

CHECKS=0
ARGS=()
for arg in "$@"; do
  if [[ "$arg" == "--checks" ]]; then
    CHECKS=1
  else
    ARGS+=("$arg")
  fi
done
if [[ ${#ARGS[@]} -lt 1 ]]; then
  echo "usage: $0 <bin-dir> [scenario] [--checks]" >&2
  exit 2
fi
BIN="$(cd "${ARGS[0]}" && pwd)"
SCENARIO="${ARGS[1]:-takeoff_goto}"
HERE="$(cd "$(dirname "$0")" && pwd)"
CONFIG="$HERE/config"
LOGS="$BIN/logs"
mkdir -p "$LOGS"
# The executables of this build are found by name.
export PATH="$BIN:$PATH"

if [[ $CHECKS -eq 1 ]]; then
  ctest --test-dir "$BIN/.." -R mrs_ --output-on-failure || exit 1
fi

export UAV_NAME=uav1
export MRS_DIR="$HERE"
export MRS_SCENARIO="$SCENARIO"

case "$SCENARIO" in
  estimation) PROCESSES=(estimation_manager) ;;
  takeoff) PROCESSES=(estimation_manager transform_manager control_manager) ;;
  takeoff_via_uav_manager | takeoff_goto | goto_outside_safety_area)
    PROCESSES=(estimation_manager transform_manager control_manager uav_manager) ;;
  teleop) echo "run.sh: teleop runs under hypha launch: examples/mrs/launch.sh up --attach" >&2; exit 2 ;;
  *) echo "run.sh: unknown scenario '$SCENARIO'" >&2; exit 2 ;;
esac

# The simulator waits for exactly the processes this scenario starts.
PARTICIPANTS="\"${UAV_NAME}_mission\""
for process in "${PROCESSES[@]}"; do
  PARTICIPANTS="$PARTICIPANTS, \"${UAV_NAME}_$process\""
done
SIM_CONFIG="$LOGS/mrs_simulator.$SCENARIO.toml"
printf 'include = ["%s"]\n\n[clock]\nparticipants = [%s]\n' \
  "$CONFIG/mrs_simulator.toml" "$PARTICIPANTS" > "$SIM_CONFIG"
# MRS_REAL_TIME_FACTOR=1.0 paces the flight to the wall clock; unset runs it unpaced.
if [[ -n "${MRS_REAL_TIME_FACTOR:-}" ]]; then
  printf 'real_time_factor = %s\n' "$MRS_REAL_TIME_FACTOR" >> "$SIM_CONFIG"
fi

PIDS=()
cleanup() {
  for pid in "${PIDS[@]}"; do
    kill "$pid" 2>/dev/null
  done
  wait 2>/dev/null
}
trap cleanup EXIT

mrs_simulator "$SIM_CONFIG" > "$LOGS/mrs_simulator.log" 2>&1 &
SIM_PID=$!
PIDS+=("$SIM_PID")
# HYPHA_USE_SIM_TIME is set per command, never exported: the simulator publishes
# /clock and runs on host time.
for process in "${PROCESSES[@]}"; do
  HYPHA_USE_SIM_TIME=1 "mrs_$process" "$CONFIG/$process.toml" > "$LOGS/$process.log" 2>&1 &
  PIDS+=($!)
done
HYPHA_USE_SIM_TIME=1 mrs_mission "$CONFIG/mission.toml" > "$LOGS/mission.log" 2>&1 &
MISSION_PID=$!

# The mission's exit code is the result. A simulator that ends first (it fails with
# Timeout once the mission stops reporting, which is expected after the mission ends)
# leaves the mission without a clock: then the run fails.
while kill -0 "$MISSION_PID" 2>/dev/null; do
  if ! kill -0 "$SIM_PID" 2>/dev/null; then
    sleep 1
    if kill -0 "$MISSION_PID" 2>/dev/null; then
      echo "run.sh: the simulator ended before the mission:" >&2
      grep -h "ERROR" "$LOGS/mrs_simulator.log" | tail -3 >&2
      kill "$MISSION_PID" 2>/dev/null
      wait "$MISSION_PID" 2>/dev/null
      echo "run.sh: scenario $SCENARIO failed (logs in $LOGS)"
      exit 1
    fi
  fi
  sleep 0.2
done
wait "$MISSION_PID"
CODE=$?

grep -h "mission PASSED\|mission FAILED" "$LOGS/mission.log" || tail -5 "$LOGS/mission.log"
echo "run.sh: scenario $SCENARIO exited with $CODE (logs in $LOGS)"
exit $CODE
