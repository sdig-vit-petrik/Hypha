#!/usr/bin/env bash
# usage: test.sh [--clean] [rust|cpp]
#
# Builds with build.sh, then runs the Rust suite (clippy, nextest, doctests), the
# C++ suite (ctest, the installed SDK, the examples and the MRS flights against it,
# hypha as a subproject), or both by default.
set -euo pipefail

cd "$(dirname "$0")"

clean=()
if [[ "${1:-}" == "--clean" ]]; then
  clean=(--clean)
  shift
fi
suite="${1:-all}"

./build.sh --tests "${clean[@]}"
# Once is enough: every test process runs in a bus domain of its own.
./clean-shm.sh --force

if [[ "$suite" == all || "$suite" == rust ]]; then
  cargo clippy --locked --workspace --all-targets -- -D warnings
  cargo nextest run --locked --workspace
  cargo test --locked --workspace --doc
  # The debug library exports exactly the recorded C symbols.
  cargo build --locked -p hypha_ffi
  ./scripts/check-exported-symbols.sh
fi

if [[ "$suite" == all || "$suite" == cpp ]]; then
  # The gtest shards run in parallel, each in a bus domain of its own (test/test_main.cpp).
  ctest --test-dir build -j "$(getconf _NPROCESSORS_ONLN)" --output-on-failure \
    --output-junit ctest-junit.xml

  prefix="$(mktemp -d)"
  trap 'rm -rf "$prefix"' EXIT
  cmake --install build --prefix "$prefix"
  ./scripts/test-installed-sdk.sh "$prefix"
  # Every example builds against the installed SDK; the MRS port flies all its scenarios.
  examples/check.sh --prefix "$prefix" --build-dir "$prefix/examples-build"

  # How an application builds hypha: from source, in its own tree.
  cmake -S test/package_consumer -B build/subproject-consumer -DCMAKE_BUILD_TYPE=RelWithDebInfo \
    -DHYPHA_SOURCE_DIR="$PWD"
  cmake --build build/subproject-consumer --parallel "${CMAKE_BUILD_PARALLEL_LEVEL:-$(getconf _NPROCESSORS_ONLN)}"
  build/subproject-consumer/hypha_package_consumer
fi
