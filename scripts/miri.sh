#!/usr/bin/env bash
# usage: scripts/miri.sh [test name filters or other cargo nextest run arguments...]
#
# A name filter narrows the set below; a further -E widens it (nextest joins -E
# expressions with "or").
#
# Runs the Rust tests that need no transport under Miri, which interprets them and
# fails on undefined behaviour, a data race or a leak. Miri cannot run a node: it
# calls getrlimit, opens iceoryx2 shared memory and spawns peers.
#
# HYPHA_NIGHTLY overrides the nightly toolchain docker/rust-nightly names. The build
# goes to target-miri/, so target/ is untouched.
set -euo pipefail

cd "$(dirname "$0")/.."

toolchain="${HYPHA_NIGHTLY:-$(cat docker/rust-nightly)}"
if ! cargo "+$toolchain" miri --version >/dev/null 2>&1; then
  echo "Error: no Miri on toolchain $toolchain: rustup toolchain install $toolchain -c miri -c rust-src," >&2
  echo "or set HYPHA_NIGHTLY to an installed nightly with Miri" >&2
  exit 2
fi

# The crates read the wall clock and the config tests write temp files: isolation
# would refuse both.
export MIRIFLAGS="${MIRIFLAGS:--Zmiri-disable-isolation}"
export CARGO_INCREMENTAL=0

packages=(hypha_time hypha_wire hypha_msgdef hypha_cycle hypha_core hypha_runtime hypha_log
          hypha_msgs hypha_node)
args=()
for package in "${packages[@]}"; do
  args+=(-p "$package")
done
node_tests='kind(lib) | binary(endpoint_options) | binary(topic_pattern_matching)'
node_tests+=' | (binary(clock) & test(=the_clock_reads_the_host_clock_until_simulated_time_is_enabled))'
filter="not package(hypha_node) | (package(hypha_node) & ($node_tests))"

# nextest runs each test in a Miri process of its own, in parallel; libtest under
# Miri would run them one after another.
cargo "+$toolchain" miri nextest run --locked --target-dir target-miri \
  --config-file .config/nextest-miri.toml "${args[@]}" -E "$filter" "$@"
