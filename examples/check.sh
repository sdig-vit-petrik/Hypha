#!/usr/bin/env bash
#
# @brief      The Examples gate: every C++ example.
# @author     Vit Petrik
# @copyright  (c) 2026 Vit Petrik. Licensed under the Apache License, Version 2.0.
set -e

cd "$(dirname "$0")/.."

show_help() {
  echo "Usage: $0 [options]"
  echo
  echo "This script runs the Examples gate."
  echo
  echo "Options:"
  echo "  --prefix <PATH>     Installed hypha SDK prefix, unless it is on the"
  echo "                      default CMake search path."
  echo "  --build-dir <PATH>  Build directory (default build/examples). CMake caches the"
  echo "                      SDK it found, so give a new directory for another prefix."
  echo "  -h, --help          Display this help message and exit."
}

PREFIX=""
BUILD_DIR="build/examples"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix)
      if [[ -z "$2" ]]; then
        echo "Error: --prefix requires a value"
        exit 1
      fi
      PREFIX="$2"
      shift 2
      ;;
    --build-dir)
      if [[ -z "$2" ]]; then
        echo "Error: --build-dir requires a value"
        exit 1
      fi
      BUILD_DIR="$2"
      shift 2
      ;;
    -h|--help)
      show_help
      exit 0
      ;;
    *)
      echo "Error: unknown option $1"
      exit 1
      ;;
  esac
done

CMAKE_ARGS="-DCMAKE_BUILD_TYPE=Release"
if [[ -n "$PREFIX" ]]; then
  CMAKE_ARGS="$CMAKE_ARGS -DCMAKE_PREFIX_PATH=$PREFIX"
fi

echo "-> Building the examples against the installed SDK"
cmake -S examples -B "$BUILD_DIR" $CMAKE_ARGS
cmake --build "$BUILD_DIR" --parallel "${CMAKE_BUILD_PARALLEL_LEVEL:-$(getconf _NPROCESSORS_ONLN)}"

# Every test runs in a bus domain of its own under the test root t_ (hypha_add_test); the
# domains of the runs that ended go, a system running on this host keeps its own.
./clean-shm.sh --domain t_

echo -e "\n-> The MRS flights: unit checks, then every scenario"
# Each flight in a test domain of its own, t_<pid of this gate>_<n>.
flight=1
HYPHA_DOMAIN="t_$$_${flight}" examples/mrs/run.sh "$BUILD_DIR/mrs" takeoff_goto --checks
for scenario in estimation takeoff takeoff_via_uav_manager goto_outside_safety_area; do
  flight=$((flight + 1))
  HYPHA_DOMAIN="t_$$_${flight}" examples/mrs/run.sh "$BUILD_DIR/mrs" "$scenario"
done

echo -e "\n-> The Autoware port: the upstream unit tests, the shim, each node on the bus"
ctest --test-dir "$BUILD_DIR/autoware" --output-on-failure

echo -e "\nGate complete."
