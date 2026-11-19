#!/usr/bin/env bash
# Regenerates crates/hypha_testing_mcap/include/hypha/testing/mcap_reader.h from the
# hypha_testing_mcap crate with cbindgen. check.sh calls this script in verify mode
# to catch a header that has drifted from the Rust source.

set -e
set -u
set -o pipefail

cd "$(dirname "$0")/.."

if ! command -v cbindgen >/dev/null 2>&1; then
  echo "ERROR: 'cbindgen' is required. Install it with: cargo install cbindgen --locked" >&2
  exit 1
fi

readonly CRATE_DIR="crates/hypha_testing_mcap"
readonly CONFIG_FILE="${CRATE_DIR}/cbindgen.toml"
readonly HEADER_FILE="${CRATE_DIR}/include/hypha/testing/mcap_reader.h"

generate_header() {
  cbindgen --quiet --config "$CONFIG_FILE" --crate hypha_testing_mcap --output "$1" "$CRATE_DIR"
}

if [[ "${1:-}" == "--verify" ]]; then
  generated_file="$(mktemp)"
  trap 'rm -f "$generated_file"' EXIT
  generate_header "$generated_file"
  if ! diff -u "$HEADER_FILE" "$generated_file"; then
    echo "ERROR: ${HEADER_FILE} is stale. Run: ./scripts/gen-mcap-reader-header.sh" >&2
    exit 1
  fi
  echo "OK: ${HEADER_FILE} matches the generated header."
else
  generate_header "$HEADER_FILE"
  echo "Wrote ${HEADER_FILE}."
fi
