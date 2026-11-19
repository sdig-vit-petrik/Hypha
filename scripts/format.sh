#!/usr/bin/env bash
# usage: scripts/format.sh [--check]
#
# Formats every tracked Rust, C++ and Python file with rustfmt.toml,
# .clang-format and ruff.toml. --check writes nothing and fails on drift.
set -euo pipefail

cd "$(dirname "$0")/.."

# Another clang-format major formats C++ differently.
readonly CLANG_FORMAT_MAJOR=23
if ! clang-format --version | grep -q "version ${CLANG_FORMAT_MAJOR}\."; then
  echo "ERROR: needs clang-format ${CLANG_FORMAT_MAJOR}.x, found: $(clang-format --version)" >&2
  echo "ERROR: install it with: python3 -m pip install 'clang-format==${CLANG_FORMAT_MAJOR}.*'" >&2
  exit 2
fi

# .clang-format-ignore keeps clang-format off the cbindgen header.
cxx_files() { git ls-files -z '*.cpp' '*.cc' '*.h' '*.hpp'; }

if [[ "${1:-}" == "--check" ]]; then
  cargo fmt --all -- --check
  cxx_files | xargs -0 clang-format --dry-run -Werror
  git ls-files -z '*.py' | xargs -0 python3 -m ruff format --check --config ruff.toml
else
  cargo fmt --all
  cxx_files | xargs -0 clang-format -i
  git ls-files -z '*.py' | xargs -0 python3 -m ruff format --config ruff.toml
fi
