#!/usr/bin/env bash
# usage: scripts/sanitize.sh [address|thread]
#
# Builds the C++ tests with a sanitizer and runs them: address (AddressSanitizer and
# UndefinedBehaviorSanitizer, the default) into build-asan/, thread (ThreadSanitizer)
# into build-tsan/. Any report fails the run. The Rust core in libhypha_ffi is not
# instrumented; its own sanitizer runs need a nightly toolchain.
set -euo pipefail

cd "$(dirname "$0")/.."

# GCC reports -Warray-bounds on code the sanitizer instrumented (a refused length in
# test_ros_serialization_bounds.cpp, libstdc++ internals); the build without a
# sanitizer keeps the warning as an error.
relaxed="-Wno-error=array-bounds"

case "${1:-address}" in
  address)
    dir=build-asan
    flags="-fsanitize=address,undefined -fno-sanitize-recover=undefined -fno-omit-frame-pointer"
    ;;
  thread)
    dir=build-tsan
    flags="-fsanitize=thread -fno-omit-frame-pointer"
    ;;
  *)
    echo "usage: $0 [address|thread]" >&2
    exit 2
    ;;
esac

cmake -S . -B "$dir" -DCMAKE_BUILD_TYPE=RelWithDebInfo -DHYPHA_BUILD_TESTS=ON \
  -DCMAKE_CXX_FLAGS="$flags $relaxed" -DCMAKE_EXE_LINKER_FLAGS="$flags" -DCMAKE_SHARED_LINKER_FLAGS="$flags"
cmake --build "$dir" --parallel

export ASAN_OPTIONS="halt_on_error=1:abort_on_error=0"
export UBSAN_OPTIONS="print_stacktrace=1:halt_on_error=1"
export TSAN_OPTIONS="halt_on_error=1:suppressions=$PWD/scripts/tsan-suppressions.txt"
# macOS has no LeakSanitizer.
if [[ "$(uname)" == Darwin ]]; then
  ASAN_OPTIONS="${ASAN_OPTIONS}:detect_leaks=0"
fi
# The gtest shards run in parallel, each in a bus domain of its own; CTEST_PARALLEL_LEVEL
# bounds them, as each sanitized process takes several times its memory. The wall-time
# tests are left out: a sanitizer slows the host they measure (the plain run has them).
ctest --test-dir "$dir" -j "${CTEST_PARALLEL_LEVEL:-$(getconf _NPROCESSORS_ONLN)}" -LE wall-time \
  --output-on-failure
