#!/usr/bin/env bash
# Style, generated-file and dependency-policy checks. Reads the sources only;
# needs no build. `cargo deny check` (deny.toml) fetches the RustSec advisory
# database and the crate manifests of Cargo.lock.
set -euo pipefail

cd "$(dirname "$0")"

./scripts/gen-ffi-header.sh --verify
./scripts/gen-mcap-reader-header.sh --verify
# docs/traceability.md is generated; every requirement has an entry and every test
# reference names a test in the tree.
python3 -I scripts/traceability.py render --verify
python3 -I scripts/test_traceability.py
./scripts/check-features.sh
git ls-files '*.sh' | xargs shellcheck --severity=warning --external-sources

# The image installs the toolchain rust-toolchain.toml selects.
toolchain="$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)"
if ! grep -q "^ARG HYPHA_RUST_VERSION=${toolchain}\$" docker/Dockerfile; then
  echo "docker/Dockerfile HYPHA_RUST_VERSION differs from rust-toolchain.toml ($toolchain)" >&2
  exit 1
fi

# C++ public headers: at most 400 lines each.
long_headers="$(git ls-files 'crates/hypha-cxx/*include/*.h' | xargs wc -l | awk '$2 != "total" && $1 > 400')"
if [[ -n "$long_headers" ]]; then
  echo "C++ headers over 400 lines:" >&2
  echo "$long_headers" >&2
  exit 1
fi

# C++ public headers: no default lambda capture and no decltype(auto) (named state).
sdk_roots=(crates/hypha-cxx/include/hypha crates/hypha-cxx/base/include crates/hypha-cxx/log/include)
if grep -rnE '\[[&=][[:space:]]*[],]' "${sdk_roots[@]}"; then
  echo "C++ headers with a default lambda capture ([&] or [=]): name each capture" >&2
  exit 1
fi
if grep -rn 'decltype(auto)' "${sdk_roots[@]}"; then
  echo "C++ headers with decltype(auto): spell the return type" >&2
  exit 1
fi

./scripts/check-links.sh

./scripts/check-deps.sh
# The gate refuses a forbidden edge: a tool that depends on the transport.
forbidden_edge='
import json, sys
metadata = json.load(sys.stdin)
for package in metadata["packages"]:
    if package["name"] == "hypha_tools":
        package["dependencies"].append({"name": "hypha_iceoryx2", "kind": None})
json.dump(metadata, sys.stdout)
'
if cargo metadata --format-version 1 --locked --offline --no-deps | python3 -I -c "$forbidden_edge" \
    | ./scripts/check-deps.sh /dev/stdin >/dev/null 2>&1; then
  echo "scripts/check-deps.sh accepted a tool that depends on hypha_iceoryx2" >&2
  exit 1
fi
# ... and a crate other than hypha_node that reaches the operating-system calls.
forbidden_sys_edge="${forbidden_edge//hypha_tools/hypha_ffi}"
forbidden_sys_edge="${forbidden_sys_edge//hypha_iceoryx2/hypha_sys}"
if cargo metadata --format-version 1 --locked --offline --no-deps | python3 -I -c "$forbidden_sys_edge" \
    | ./scripts/check-deps.sh /dev/stdin >/dev/null 2>&1; then
  echo "scripts/check-deps.sh accepted hypha_ffi depending on hypha_sys" >&2
  exit 1
fi

cargo deny --locked check
# The fuzz targets are a workspace of their own with a lock file of their own. Its
# tree does not hold every licence of the list above, which is no finding there.
cargo deny --manifest-path fuzz/Cargo.toml --locked check -A license-not-encountered

./scripts/format.sh --check
