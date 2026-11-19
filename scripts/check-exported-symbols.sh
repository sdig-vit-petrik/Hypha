#!/usr/bin/env bash
# Checks that libhypha_ffi.so exports exactly the symbols recorded in
# crates/hypha_ffi/exported-symbols.txt: the sorted `nm -D --defined-only`
# output without addresses. Only a change that alters the ABI on purpose
# rewrites it, with --update.

set -e
set -u
set -o pipefail

cd "$(dirname "$0")/.."

readonly SYMBOL_FILE="crates/hypha_ffi/exported-symbols.txt"
readonly LIBRARY="target/debug/libhypha_ffi.so"

if ! command -v nm >/dev/null 2>&1; then
  echo "ERROR: 'nm' (binutils) is required." >&2
  exit 1
fi

update=0
if [[ "${1:-}" == "--update" ]]; then
  update=1
fi

cargo build -q -p hypha_ffi --locked

exported_symbols() {
  nm -D --defined-only "$LIBRARY" | awk '{ print $2, $3 }' | LC_ALL=C sort
}

if [[ "$update" -eq 1 ]]; then
  exported_symbols > "$SYMBOL_FILE"
  echo "Wrote ${SYMBOL_FILE}."
  exit 0
fi

current_file="$(mktemp)"
trap 'rm -f "$current_file"' EXIT
exported_symbols > "$current_file"
if ! diff -u "$SYMBOL_FILE" "$current_file"; then
  echo "ERROR: ${LIBRARY} exports another symbol set than ${SYMBOL_FILE}." >&2
  exit 1
fi
echo "OK: ${LIBRARY} exports the $(wc -l < "$SYMBOL_FILE") recorded symbols."
