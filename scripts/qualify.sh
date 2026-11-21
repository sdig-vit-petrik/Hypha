#!/usr/bin/env bash
# usage: scripts/qualify.sh [fuzz-seconds-per-target]
#
# Runs the qualification of this tree and writes target/qualification/report.md: the
# result of every requirement of docs/requirements-specification.md from the tests
# docs/traceability.toml names for it. The runs, each with its log beside the report:
# check.sh, the build, the Rust tests, the C++ tests, the coverage gate, Miri, the
# fuzz targets (default 60 s each), AddressSanitizer with UndefinedBehaviorSanitizer,
# and ThreadSanitizer. A failed run does not stop the next one; the report states it.
#
# Exit status: 0 every requirement is verified (a release needs it), 3 every one is
# verified but those with no test yet and a recorded gap (CI accepts it), 1 a test or a
# run failed or did not run, or an open requirement has no gap, 2 the build failed or a
# test run left no result file.
set -uo pipefail

cd "$(dirname "$0")/.." || exit 2

fuzz_seconds="${1:-60}"
out=target/qualification
rm -rf "$out"
mkdir -p "$out"

# Runs "$@" with its output in $out/$name.log; prints and returns its exit status.
run_step() {
  local name="$1"
  shift
  local status=0
  "$@" >"$out/$name.log" 2>&1 || status=$?
  echo "$name: exit $status"
  return "$status"
}

outcome_of() {
  if [[ "$1" -eq 0 ]]; then
    echo pass
  else
    echo fail
  fi
}

run_step check ./check.sh
check_status=$?

if ! run_step build ./build.sh --tests; then
  echo "Error: the build failed, see $out/build.log" >&2
  exit 2
fi

# Their failures reach the report through the result files, test by test.
run_step rust cargo nextest run --locked --workspace
if ! cp target/nextest/default/junit.xml "$out/nextest-junit.xml"; then
  echo "Error: the Rust tests left no result file, see $out/rust.log" >&2
  exit 2
fi
# A result file of an earlier run would count as a result of this one.
rm -rf build/gtest-xml
run_step cxx ctest --test-dir build -j "$(getconf _NPROCESSORS_ONLN)" --output-on-failure

run_step coverage scripts/coverage.sh
coverage_status=$?
run_step miri scripts/miri.sh
miri_status=$?
run_step fuzz scripts/fuzz.sh "$fuzz_seconds"
fuzz_status=$?
run_step asan scripts/sanitize.sh address
asan_status=$?
run_step tsan scripts/sanitize.sh thread
tsan_status=$?

python3 -I scripts/traceability.py report --output "$out/report.md" \
  --nextest-junit "$out/nextest-junit.xml" --gtest-xml build/gtest-xml \
  --run "check=$(outcome_of "$check_status")" \
  --run "coverage=$(outcome_of "$coverage_status")" \
  --run "miri=$(outcome_of "$miri_status")" \
  --run "fuzz=$(outcome_of "$fuzz_status")" \
  --run "asan=$(outcome_of "$asan_status")" \
  --run "tsan=$(outcome_of "$tsan_status")"
