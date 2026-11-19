#!/usr/bin/env bash
# Rust tests of crates/ under coverage, a Cobertura report in coverage/, and
# the 80 % line-coverage gate per crate. The apps have no coverage gate.
set -euo pipefail

cd "$(dirname "$0")/.."

cargo llvm-cov nextest --locked --workspace \
  --exclude hypha_tools --exclude hypha_ros_bridge \
  --no-report
mkdir -p coverage
cargo llvm-cov report --cobertura --output-path coverage/cobertura.xml

for package in hypha_wire hypha_msgdef hypha_time hypha_msggen hypha_iceoryx2 \
               hypha_ffi hypha_core hypha_cycle hypha_runtime hypha_log; do
  cargo llvm-cov report --package "$package" --fail-under-lines 80
done
