#!/usr/bin/env bash
# usage: scripts/overload.sh <overload|isolation-latency> [report.json]
#
# Runs a sustained-overload workload of NFR-05 on the release build of `hypha` and
# writes the report (default target/<name>.json). It does not build: run ./build.sh
# first.
#   overload           examples/load/overload.toml: memory, counted loss, refusals
#   isolation-latency  examples/load/isolation-latency.toml: the same, and the latency
#                      of the control topics
#
# The exit status is hypha load's: 0 every limit passed, 1 a limit failed, 70 the run
# itself failed; 2 for an unknown name or when there is no release build. On macOS the
# written shared memory is not measured, so its limit fails there by design.
set -uo pipefail

cd "$(dirname "$0")/.." || exit 2

readonly HYPHA=target/release/hypha
name="${1:-}"
case "$name" in
  overload | isolation-latency) ;;
  *)
    echo "usage: scripts/overload.sh <overload|isolation-latency> [report.json]" >&2
    exit 2
    ;;
esac
report="${2:-target/$name.json}"

if [[ ! -x "$HYPHA" ]]; then
  echo "Error: no release build at $HYPHA; run ./build.sh first" >&2
  exit 2
fi
mkdir -p "$(dirname "$report")" || exit 2

status=0
"$HYPHA" load "examples/load/$name.toml" --report "$report" || status=$?
echo "$name: exit $status, report $report"
exit "$status"
