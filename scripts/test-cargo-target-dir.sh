#!/usr/bin/env bash
# usage: scripts/test-cargo-target-dir.sh
#
# Where CMake looks for what cargo builds (#181): <source>/target by default, and the
# CARGO_TARGET_DIR of the environment when set, in every place that names a cargo
# output, with the cargo call given the same directory. Configures only, builds nothing.
set -euo pipefail

cd "$(dirname "$0")/.."
repo="$PWD"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

configure() {
  cmake -S . -B "$1" -DCMAKE_BUILD_TYPE=Release -DHYPHA_BUILD_TESTS=OFF \
    -DHYPHA_BUILD_EXAMPLES=OFF >"$1.log" 2>&1
}

# Unset: the tree's own target/, which the cargo call is given too.
env -u CARGO_TARGET_DIR cmake -S . -B "$work/default" -DCMAKE_BUILD_TYPE=Release \
  -DHYPHA_BUILD_TESTS=OFF -DHYPHA_BUILD_EXAMPLES=OFF >"$work/default.log" 2>&1
grep -qF "HYPHA_MSGGEN_BIN:INTERNAL=$repo/target/release/hypha_msggen" "$work/default/CMakeCache.txt"
grep -rqF -- "--target-dir $repo/target" "$work/default/crates/hypha_ffi"

# Set, to a directory that does not exist yet: every cargo output is looked for there.
external="$work/cargo-target"
CARGO_TARGET_DIR="$external" configure "$work/external"
grep -qF "HYPHA_MSGGEN_BIN:INTERNAL=$external/release/hypha_msggen" "$work/external/CMakeCache.txt"
grep -rqF -- "--target-dir $external" "$work/external/crates/hypha_ffi"
grep -rqF -- "$external/release/hypha" "$work/external/apps"
if grep -rqF "$repo/target" "$work/external"; then
  echo "Error: a configure with CARGO_TARGET_DIR set still names $repo/target" >&2
  exit 1
fi

# A relative one is refused: cargo resolves it against the directory of each call.
if CARGO_TARGET_DIR=relative/target configure "$work/relative"; then
  echo "Error: a relative CARGO_TARGET_DIR was accepted" >&2
  exit 1
fi
grep -qF "CARGO_TARGET_DIR is the relative path" "$work/relative.log"

echo "ok: CMake follows CARGO_TARGET_DIR"
