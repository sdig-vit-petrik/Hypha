#!/usr/bin/env bash
# Usage: launch.sh [up [--attach] | attach | down]
# Replay an Isuzu bag through Slice A and the localization under hypha launch
# (launch/isuzu.toml), for Foxglove.
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
repo=$(cd -- "$here/../.." && pwd -P)
# Relative paths are the caller's: the units run in $here.
absolute() { case "$1" in /*) echo "$1" ;; *) echo "$PWD/$1" ;; esac; }
build=$(absolute "${AUTOWARE_BUILD_DIR:-$repo/build/examples/autoware}")
# hypha from PATH, else the one ./build.sh leaves in target/release.
export PATH="$build:$PATH:$repo/target/release"
ISUZU_INPUT=$(absolute "${ISUZU_INPUT:-$repo/data/isuzu/all-sensors-bag4/isuzu-localization.mcap}")
export ISUZU_INPUT

if [ "${1:-up}" = up ] && [ ! -f "$ISUZU_INPUT" ]; then
    echo "launch.sh: no recording at ISUZU_INPUT=$ISUZU_INPUT (README.md, \"Watch the port in Foxglove\")" >&2
    exit 2
fi

echo "Isuzu: binaries from $build, replaying $ISUZU_INPUT"
cd "$here"
exec hypha launch "${1:-up}" launch/isuzu.toml "${@:2}"
