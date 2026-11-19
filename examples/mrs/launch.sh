#!/usr/bin/env bash
# Usage: launch.sh [up [--attach] | attach | down]
# Fly uav1 from the keyboard under hypha launch (launch/mrs_teleop.toml).
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
build=${MRS_BUILD_DIR:-$here/../../build/examples/mrs}
export PATH="$build:$PATH"
export MRS_LOGS="$build/logs"
mkdir -p "$MRS_LOGS"

echo "MRS teleop: binaries from $build, teleop log in $MRS_LOGS/teleop.log"
cd "$here"
exec hypha launch "${1:-up}" launch/mrs_teleop.toml "${@:2}"
