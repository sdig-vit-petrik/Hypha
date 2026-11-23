#!/usr/bin/env bash
# Usage: launch.sh [up [--attach] | attach | down]
# Replay a bag through Slice A under hypha launch (launch/slice_a.toml), for Foxglove.
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
repo=$(cd -- "$here/../.." && pwd -P)
# Relative paths are the caller's: the units run in $here.
absolute() { case "$1" in /*) echo "$1" ;; *) echo "$PWD/$1" ;; esac; }
build=$(absolute "${AUTOWARE_BUILD_DIR:-$repo/build/examples/autoware}")
# hypha from PATH, else the one ./build.sh leaves in target/release.
export PATH="$build:$PATH:$repo/target/release"
SLICE_A_INPUT=$(absolute "${SLICE_A_INPUT:-$repo/data/isuzu/all-sensors-bag4/slice-a.mcap}")
export SLICE_A_INPUT

if [ "${1:-up}" = up ] && [ ! -f "$SLICE_A_INPUT" ]; then
    echo "launch.sh: no recording at SLICE_A_INPUT=$SLICE_A_INPUT (README.md, \"Slice A end to end\")" >&2
    exit 2
fi

echo "Slice A: binaries from $build, replaying $SLICE_A_INPUT"
cd "$here"
exec hypha launch "${1:-up}" launch/slice_a.toml "${@:2}"
