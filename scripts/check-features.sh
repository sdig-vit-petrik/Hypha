#!/usr/bin/env bash
# usage: scripts/check-features.sh
#
# Every C ABI function and every public C++ header must appear in docs/features.md, in
# backticks: by its name, or by a pattern such as `hypha_clock_*sim_time`. A new public surface
# then needs a row (docs/features.md, rule 3).
set -euo pipefail
cd "$(dirname "$0")/.."

python3 - <<'EOF'
import fnmatch, pathlib, re, sys

doc = pathlib.Path("docs/features.md").read_text()
names = set(re.findall(r"`([^`\s]+)`", doc))
patterns = [name for name in names if "*" in name]

def listed(name):
    return name in names or any(fnmatch.fnmatchcase(name, p) for p in patterns)

header = pathlib.Path("crates/hypha_ffi/include/hypha/hypha_ffi.h").read_text()
functions = sorted(set(re.findall(r"\b(hypha_[a-z0-9_]+)\(", header)))
headers = sorted(p.name for p in pathlib.Path("crates/hypha-cxx/include/hypha").glob("*.h"))
headers += ["hypha_base/" + p.name for p in pathlib.Path("crates/hypha-cxx/base/include/hypha_base").glob("*.h")]
headers += ["hypha_log/" + p.name for p in pathlib.Path("crates/hypha-cxx/log/include/hypha_log").glob("*.h")]

missing = [n for n in functions + headers if not listed(n)]
if missing:
    print("docs/features.md lists no row for:", file=sys.stderr)
    for name in missing:
        print("  " + name, file=sys.stderr)
    sys.exit(1)
print(f"OK: docs/features.md covers {len(functions)} C ABI functions and {len(headers)} public headers.")
EOF
