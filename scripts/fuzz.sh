#!/usr/bin/env bash
# usage: scripts/fuzz.sh [seconds-per-target]
#
# Runs every fuzz target of fuzz/ (cargo-fuzz, libFuzzer with AddressSanitizer)
# for seconds-per-target (default 60), one after another, and fails if any target
# reports a crash: a panic, a failed invariant, a sanitizer report, a timeout or an
# out-of-memory. Each target starts from its committed seeds in fuzz/corpus/<target>/
# and grows that directory; a crash input lands in fuzz/artifacts/<target>/. Both
# grown files are git-ignored.
#
# The nightly toolchain is the one docker/rust-nightly names; HYPHA_NIGHTLY
# overrides it (e.g. HYPHA_NIGHTLY=nightly). HYPHA_FUZZ_SANITIZER replaces the
# sanitizer (cargo fuzz --sanitizer: address, the default, or none on a host where
# an /etc/ld.so.preload library breaks AddressSanitizer at start-up). The build
# goes to fuzz/target/.
set -euo pipefail

cd "$(dirname "$0")/.."

seconds="${1:-60}"
if ! [[ "$seconds" =~ ^[1-9][0-9]*$ ]]; then
  sed -n '2,15p' "$0" >&2
  exit 2
fi

toolchain="${HYPHA_NIGHTLY:-$(cat docker/rust-nightly)}"
if ! cargo "+$toolchain" --version >/dev/null 2>&1; then
  echo "Error: toolchain $toolchain is not installed: rustup toolchain install $toolchain," >&2
  echo "or set HYPHA_NIGHTLY to an installed nightly" >&2
  exit 2
fi
sanitizer="${HYPHA_FUZZ_SANITIZER:-address}"
export CARGO_INCREMENTAL=0

cd fuzz
mapfile -t targets < <(cargo "+$toolchain" fuzz list)
if [[ ${#targets[@]} -eq 0 ]]; then
  echo "Error: no fuzz targets in fuzz/" >&2
  exit 1
fi
cargo "+$toolchain" fuzz build --sanitizer "$sanitizer"

failed=()
for target in "${targets[@]}"; do
  echo "== $target: ${seconds}s"
  mkdir -p "corpus/$target"
  # -rss_limit_mb and -timeout make a blow-up a reported crash, not a stall.
  if ! cargo "+$toolchain" fuzz run --sanitizer "$sanitizer" "$target" "corpus/$target" -- \
      -max_total_time="$seconds" -timeout=10 -rss_limit_mb=2048 -print_final_stats=1; then
    failed+=("$target")
  fi
done

if [[ ${#failed[@]} -gt 0 ]]; then
  echo "FAIL: ${failed[*]} crashed; the inputs are in fuzz/artifacts/" >&2
  exit 1
fi
echo "fuzz: ${#targets[@]} targets, ${seconds}s each, no crash"
