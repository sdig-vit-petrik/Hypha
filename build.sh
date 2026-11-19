#!/usr/bin/env bash
# usage: build.sh [--clean] [--tests] [cmake --build args...]
#
# Release build of libhypha_ffi, the C++ SDK, tools, and examples.
# By default, tests are NOT built, so only a C++ compiler, cmake and the Rust toolchain are needed.
# --clean removes build/, target/ and coverage/ first.
# --tests builds tests as well (requires GTest and Python3; CMake reports a missing one).
# Toolchain versions are enforced by cmake (3.20), Cargo.toml (rust-version) and the C++23 compile.
# Any other argument goes to cmake --build, e.g. ./build.sh --target hypha_ffi_cargo
set -euo pipefail

clean=0
build_tests=OFF
build_args=()
for arg in "$@"; do
    case "$arg" in
        --clean) clean=1 ;;
        --tests|--with-tests) build_tests=ON ;;
        --no-tests) build_tests=OFF ;;
        -h|--help) sed -n '2,9p' "$0"; exit 0 ;;
        *) build_args+=("$arg") ;;
    esac
done

cd "$(dirname "$0")"
[[ $clean -eq 1 ]] && rm -rf build target coverage

cmake -S . -B build -DCMAKE_BUILD_TYPE=Release -DHYPHA_BUILD_TESTS="$build_tests"
cmake --build build --parallel ${build_args[@]+"${build_args[@]}"}
