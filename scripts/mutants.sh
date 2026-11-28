#!/usr/bin/env bash
# usage:
#   scripts/mutants.sh [crate...]        # mutate named pure core crates (default: all 6)
#   scripts/mutants.sh --diff [base]     # mutate only lines changed vs base (default: develop)
#
# Runs cargo-mutants over Hypha's transport-free pure core crates to expose
# tautological or weak tests. Install once: cargo install cargo-mutants
set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v cargo-mutants >/dev/null 2>&1; then
  echo "cargo-mutants not found; install with: cargo install cargo-mutants" >&2
  exit 1
fi

CORE_CRATES=(
  hypha_wire
  hypha_time
  hypha_msgdef
  hypha_core
  hypha_cycle
  hypha_runtime
)

if [[ "${1:-}" == "--diff" ]]; then
  base="${2:-develop}"
  diff_file="$(mktemp /tmp/hypha-mutants-diff.XXXXXX)"
  trap 'rm -f "$diff_file"' EXIT
  git diff "${base}...HEAD" -- 'crates/*/src/*.rs' 'crates/*/src/**/*.rs' > "$diff_file"
  if [[ ! -s "$diff_file" ]]; then
    echo "No Rust production changes under crates/*/src/ compared to ${base}."
    exit 0
  fi
  exec cargo mutants --in-diff "$diff_file" --no-shuffle
fi

crates=("$@")
if [[ ${#crates[@]} -eq 0 ]]; then
  crates=("${CORE_CRATES[@]}")
fi

pkg_args=()
for c in "${crates[@]}"; do
  pkg_args+=(-p "$c")
done

exec cargo mutants "${pkg_args[@]}" --no-shuffle
