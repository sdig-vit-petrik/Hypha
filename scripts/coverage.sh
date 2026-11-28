#!/usr/bin/env bash
# Rust tests of crates/ under coverage, a Cobertura report in coverage/, and
# onion-aligned line-coverage reporting:
# - Pure core crates enforce >= 80% line coverage (NFR-10); pair with scripts/mutants.sh.
# - Outer transport/runtime/FFI glue is reported without a hard line-percentage gate
#   to avoid coverage-padding filler tests; verified by boundary tests, Miri, and sanitizers.
# - Apps have no coverage gate.
set -euo pipefail

cd "$(dirname "$0")/.."

cargo llvm-cov nextest --locked --workspace \
  --exclude hypha_tools --exclude hypha_ros_bridge \
  --no-report
mkdir -p coverage
cargo llvm-cov report --cobertura --output-path coverage/cobertura.xml

CORE_CRATES=(
  hypha_wire
  hypha_msgdef
  hypha_time
  hypha_msggen
  hypha_core
  hypha_cycle
  hypha_runtime
  hypha_log
)

GLUE_CRATES=(
  hypha_iceoryx2
  hypha_node
  hypha_ffi
)

for package in "${CORE_CRATES[@]}"; do
  cargo llvm-cov report --package "$package" --fail-under-lines 80
done

echo "--- report-only (outer rings: transport / node / FFI) ---"
for package in "${GLUE_CRATES[@]}"; do
  cargo llvm-cov report --package "$package"
done
