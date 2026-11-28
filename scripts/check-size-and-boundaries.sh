#!/usr/bin/env bash
# usage: scripts/check-size-and-boundaries.sh
#
# Architectural boundary checks and file-size ratchets.
# - No Rust source under crates/*/src/ may spawn OS threads.
# - Environment-variable reads under crates/*/src/ stay in their designated files.
# - File lengths stay under the cap for their category; existing files above the
#   cap are pinned to a shrink-only ratchet so they cannot grow further.
set -euo pipefail

cd "$(dirname "$0")/.."

failures=0

# 1. No ad-hoc OS threads in shipped library crates (crates/*/src/).
#    Threads belong in apps/ (tools, bridge), never inside core/node/ffi crates.
thread_spawns="$(git grep -lE 'thread::(spawn|Builder)' -- 'crates/*/src/*' || true)"
if [[ -n "$thread_spawns" ]]; then
  echo "FAIL: crates/*/src/ must not spawn OS threads (use WaitSet / apps/):" >&2
  echo "$thread_spawns" >&2
  failures=$((failures + 1))
fi

# 2. Environment-variable reads in crates/*/src/ stay in their single owning modules.
env_allowlist='crates/hypha_core/src/parameters_load.rs
crates/hypha_iceoryx2/src/domain.rs
crates/hypha_node/src/node.rs
crates/hypha_node/src/replay_restart.rs'
env_reads="$(git grep -lE 'env::var(_os)?\b' -- 'crates/*/src/*' | grep -vxF -f <(printf '%s\n' "$env_allowlist") || true)"
if [[ -n "$env_reads" ]]; then
  echo "FAIL: env::var read outside the allowed config modules in crates/*/src/:" >&2
  echo "$env_reads" >&2
  failures=$((failures + 1))
fi

# 3. File-size caps.
#    - Production Rust (rust_prod <= 600, ffi_src <= 300): hard cap + shrink-only ratchet
#      for grandfathered files.
#    - Tests (<= 600), docs (<= 600), design notes (<= 300): hard cap on all normal and
#      new files; grandfathered monoliths warn on growth so legitimate test rows or docs
#      are not blocked while encouraging topic splits (<suite>/<topic>.rs, docs/<doc>/<topic>.md).
python3 -I - <<'PY' || failures=$((failures + 1))
import subprocess, sys

CAPS = {
    "ffi_src": 300,
    "rust_prod": 600,
    "tests": 600,
    "docs": 600,
    "design_docs": 300,
}

# Production files above their cap: strict shrink-only ratchet (hard failure on growth).
PROD_RATCHET = {
    "apps/hypha_tools/src/bin/hypha/top/intercept.rs": 1554,
    "apps/hypha_tools/src/bin/hypha/top/ui.rs": 1530,
    "apps/hypha_tools/src/load/coordinator.rs": 1057,
    "apps/hypha_tools/src/bin/hypha/top/intercept_ui.rs": 859,
    "crates/hypha_cycle/src/capture.rs": 857,
    "apps/hypha_ros_bridge/src/automatic_runtime.rs": 844,
    "apps/hypha_tools/src/bin/hypha/top/state.rs": 840,
    "crates/hypha_node/src/subscriber.rs": 826,
    "crates/hypha_cycle/src/runtime.rs": 717,
    "apps/hypha_tools/src/bin/hypha/top/graph_ui.rs": 670,
    "crates/hypha_node/src/cycle/tick.rs": 651,
    "crates/hypha_node/src/waitset.rs": 610,
}

# Grandfathered test and doc monoliths: warn on growth, fail if stale (dropped <= cap).
# Prefer topic splits (<suite>/<topic>.rs, docs/<doc>/<topic>.md) when adding to these.
SOFT_RATCHET = {
    # tests (>600)
    "crates/hypha-cxx/test/test_step_harness.cpp": 1425,
    "crates/hypha-cxx/test/test_cycle_node.cpp": 1375,
    "crates/hypha_ffi/tests/replay_fidelity.rs": 1215,
    "crates/hypha_ffi/tests/cycle_abi/trigger.rs": 1198,
    "crates/hypha-cxx/test/test_input_supervision.cpp": 931,
    "crates/hypha_iceoryx2/tests/service_transport.rs": 898,
    "crates/hypha-cxx/test/test_run_node_lifecycle.cpp": 854,
    "crates/hypha_ffi/tests/replay_upstream.rs": 828,
    "crates/hypha-cxx/test/test_topic_overloading.cpp": 777,
    "crates/hypha_ffi/tests/clock_driver.rs": 770,
    "apps/hypha_tools/tests/play_flow_control.rs": 757,
    "crates/hypha-cxx/test/test_mcap_replay.cpp": 753,
    "crates/hypha_msggen/tests/emit_cpp.rs": 715,
    "crates/hypha_ffi/tests/transform_buffer.rs": 702,
    "crates/hypha_ffi/tests/argument_preamble.rs": 700,
    "crates/hypha-cxx/test/test_node.cpp": 691,
    "crates/hypha_ffi/tests/waitset_timer_control.rs": 683,
    "crates/hypha_ffi/tests/cycle_emission.rs": 683,
    "crates/hypha_msgdef/tests/resolve.rs": 670,
    "crates/hypha_cycle/tests/trigger_capture.rs": 616,
    "crates/hypha_runtime/src/trigger_schedule/tests.rs": 614,
    "apps/hypha_tools/tests/record_sync.rs": 613,
    # docs (>600)
    "docs/cpp-api.md": 1331,
    "docs/ros1-migration-guide.md": 1327,
    "docs/execution-model.md": 1108,
    "docs/tools.md": 1105,
    "docs/interface-specification.md": 637,
}

files = subprocess.check_output(["git", "ls-files"], text=True).splitlines()

def category(path):
    if (path.startswith("crates/") or path.startswith("apps/")) and (
        ("/tests/" in path and path.endswith(".rs"))
        or path.endswith("/tests.rs")
        or (path.startswith("crates/hypha-cxx/") and "/test/" in path and path.endswith(".cpp"))
    ):
        return "tests"
    if path.startswith("crates/hypha_ffi/src/") and path.endswith(".rs"):
        return "ffi_src"
    if (path.startswith("crates/") or path.startswith("apps/")) and "/src/" in path and path.endswith(".rs"):
        return "rust_prod"
    if path.startswith("docs/design/") and path.endswith(".md"):
        return "design_docs"
    if path.startswith("docs/") and path.endswith(".md"):
        if path == "docs/traceability.md" or path.startswith("docs/releases/"):
            return None
        return "docs"
    return None

errors = []
warnings = []
seen = set()

for path in files:
    cat = category(path)
    if cat is None:
        continue
    seen.add(path)
    with open(path, "rb") as f:
        lines = f.read().count(b"\n")
    cap = CAPS[cat]
    if path in PROD_RATCHET:
        limit = PROD_RATCHET[path]
        if lines > limit:
            errors.append(f"  {path}: {lines} lines (prod ratchet max {limit}; split or trim instead of growing)")
        elif lines <= cap:
            errors.append(f"  {path}: {lines} lines is now <= {cap}; remove it from PROD_RATCHET in scripts/check-size-and-boundaries.sh")
    elif path in SOFT_RATCHET:
        limit = SOFT_RATCHET[path]
        if lines > limit:
            warnings.append(f"  {path}: {lines} lines (baseline {limit}; consider splitting by topic)")
        elif lines <= cap:
            errors.append(f"  {path}: {lines} lines is now <= {cap}; remove it from SOFT_RATCHET in scripts/check-size-and-boundaries.sh")
    elif lines > cap:
        errors.append(f"  {path}: {lines} lines ({cat} cap is {cap}; split by topic or trim)")

for table_name, table in (("PROD_RATCHET", PROD_RATCHET), ("SOFT_RATCHET", SOFT_RATCHET)):
    for path in sorted(set(table) - seen):
        errors.append(f"  {path}: no longer exists; remove it from {table_name} in scripts/check-size-and-boundaries.sh")

if warnings:
    print("WARN: grandfathered test/doc monoliths grew (prefer topic splits):", file=sys.stderr)
    print("\n".join(warnings), file=sys.stderr)

if errors:
    print("FAIL: file-size / ratchet violations:", file=sys.stderr)
    print("\n".join(errors), file=sys.stderr)
    sys.exit(1)

print(f"OK: file sizes and architectural boundaries pass ({len(PROD_RATCHET)} prod ratcheted, {len(SOFT_RATCHET)} test/doc grandfathered).")
PY

if [[ "$failures" -gt 0 ]]; then
  exit 1
fi
