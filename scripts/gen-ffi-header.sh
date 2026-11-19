#!/usr/bin/env bash
# Regenerates crates/hypha_ffi/include/hypha/hypha_ffi.h from the hypha_ffi
# crate with cbindgen. check.sh calls this script in verify mode to catch a
# header that has drifted from the Rust source.

set -e
set -u
set -o pipefail

cd "$(dirname "$0")/.."

if ! command -v cbindgen >/dev/null 2>&1; then
  echo "ERROR: 'cbindgen' is required. Install it with: cargo install cbindgen --locked" >&2
  exit 1
fi

readonly CRATE_DIR="crates/hypha_ffi"
readonly CONFIG_FILE="${CRATE_DIR}/cbindgen.toml"
readonly HEADER_FILE="${CRATE_DIR}/include/hypha/hypha_ffi.h"
readonly ABI_REVISION_FILE="abi.revision"

abi_revision="$(tr -d '[:space:]' < "$ABI_REVISION_FILE")"
if [[ ! "$abi_revision" =~ ^[0-9]+$ ]]; then
  echo "ERROR: ${ABI_REVISION_FILE} must contain one unsigned integer ABI revision." >&2
  exit 1
fi

generate_header() {
  local raw_header="$1"
  local output_header="$2"
  cbindgen --quiet --config "$CONFIG_FILE" --crate hypha_ffi --output "$raw_header" "$CRATE_DIR"
  awk -v revision="$abi_revision" '
    { print }
    $0 == "#define HYPHA_FFI_H" {
      print ""
      print "#define HYPHA_ABI_REVISION " revision
    }
  ' "$raw_header" > "$output_header"
}

verify_only=0
if [[ "${1:-}" == "--verify" ]]; then
  verify_only=1
fi

if [[ "$verify_only" -eq 1 ]]; then
  generated_file="$(mktemp)"
  raw_file="$(mktemp)"
  trap 'rm -f "$generated_file" "$raw_file"' EXIT
  generate_header "$raw_file" "$generated_file"
  if ! diff -u "$HEADER_FILE" "$generated_file"; then
    echo "ERROR: ${HEADER_FILE} is stale. Run: ./scripts/gen-ffi-header.sh" >&2
    exit 1
  fi
  echo "OK: ${HEADER_FILE} matches the generated header."
else
  raw_file="$(mktemp)"
  trap 'rm -f "$raw_file"' EXIT
  generate_header "$raw_file" "$HEADER_FILE"
  echo "Wrote ${HEADER_FILE}."
fi
