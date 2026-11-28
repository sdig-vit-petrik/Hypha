#!/usr/bin/env bash
# usage: scripts/repo-health.sh
#
# Prints a one-screen health dashboard for Hypha: LOC balance, test pyramid shape,
# ratchet debt, public C ABI surface, AGENTS.md budget, and open PR WIP count.
set -euo pipefail

cd "$(dirname "$0")/.."

python3 -I - <<'PY'
import pathlib, re, shutil, subprocess

files = subprocess.check_output(["git", "ls-files"], text=True).splitlines()
exts = (".rs", ".cpp", ".h", ".hpp", ".md", ".py", ".sh", ".cmake")

prod = tests = docs = examples = other = 0
top_tests = core_tests = 0
for p in files:
    if not (p.endswith(exts) or p.endswith("CMakeLists.txt")):
        continue
    if p.startswith(("crates/hypha-cxx/upstream/", "examples/autoware/")):
        continue
    n = pathlib.Path(p).read_bytes().count(b"\n")
    if p.startswith("docs/"):
        docs += n
    elif p.startswith("examples/"):
        examples += n
    elif "/tests/" in p or "/test/" in p or p.startswith("test/") or p.endswith(("_test.cpp", "_tests.rs", "/tests.rs")):
        tests += n
        if p.startswith(("crates/hypha_ffi/", "crates/hypha-cxx/", "apps/hypha_tools/")):
            top_tests += n
        elif p.startswith(("crates/hypha_wire/", "crates/hypha_time/", "crates/hypha_core/", "crates/hypha_cycle/", "crates/hypha_runtime/")):
            core_tests += n
    elif p.startswith(("crates/", "apps/")):
        prod += n
    else:
        other += n

header = pathlib.Path("crates/hypha_ffi/include/hypha/hypha_ffi.h").read_text()
abi_fns = len(set(re.findall(r"\b(hypha_[a-z0-9_]+)\(", header)))
cxx_headers = (
    len(list(pathlib.Path("crates/hypha-cxx/include/hypha").glob("*.h")))
    + len(list(pathlib.Path("crates/hypha-cxx/base/include/hypha_base").glob("*.h")))
    + len(list(pathlib.Path("crates/hypha-cxx/log/include/hypha_log").glob("*.h")))
)

agents_lines = pathlib.Path("AGENTS.md").read_bytes().count(b"\n")
ratchet_src = pathlib.Path("scripts/check-size-and-boundaries.sh").read_text()
prod_ratchet = len(re.findall(r'^\s+"[^"]+":\s+\d+,', ratchet_src.split("PROD_RATCHET = {")[1].split("}")[0], re.M))
soft_ratchet = len(re.findall(r'^\s+"[^"]+":\s+\d+,', ratchet_src.split("SOFT_RATCHET = {")[1].split("}")[0], re.M))

top_pct = (100.0 * top_tests / tests) if tests else 0.0
core_pct = (100.0 * core_tests / tests) if tests else 0.0
ratio = (tests / prod) if prod else 0.0

print("Hypha Repository Health")
print("=======================")
print(f"LOC:          prod={prod:,}  tests={tests:,} (ratio {ratio:.2f}x)  docs={docs:,}  examples={examples:,}")
print(f"Test pyramid: top layers (ffi/cxx/tools)={top_pct:.0f}%  pure core={core_pct:.0f}%  (target: invert upward)")
print(f"Ratchet debt: {prod_ratchet} prod files (>600 LOC), {soft_ratchet} test/doc monoliths")
print(f"Public API:   {abi_fns} C ABI functions, {cxx_headers} public C++ headers")
agents_note = "  [WARN: >120 lines]" if agents_lines > 120 else ""
print(f"AGENTS.md:    {agents_lines} lines{agents_note}")

if shutil.which("gh"):
    res = subprocess.run(
        ["gh", "pr", "list", "--base", "develop", "--state", "open", "--json", "number"],
        capture_output=True, text=True, check=False,
    )
    if res.returncode == 0 and res.stdout.strip().startswith("["):
        import json
        n_prs = len(json.loads(res.stdout))
        wip_note = "  [WARN: >5 WIP PRs]" if n_prs > 5 else ""
        print(f"Open PR WIP:  {n_prs} open against develop{wip_note}")
PY
