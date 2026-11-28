#!/usr/bin/env bash
# usage: scripts/rated-load.sh [report.json]
#
# Runs the rated load of NFR-04 with the limits of NFR-03 and NFR-04
# (examples/load/rated.toml) on the release build of `hypha`, and writes the report
# (default target/rated-load.json). It does not build: run ./build.sh first.
#
# The exit status is hypha load's: 0 every limit passed, 1 a limit failed, 70 the run
# itself failed; 2 when there is no release build. A result counts for NFR-03 and
# NFR-04 only on the reference platform, which is not defined yet: the report's `host`
# block names the host it measured.
set -uo pipefail

cd "$(dirname "$0")/.." || exit 2

readonly HYPHA=target/release/hypha
report="${1:-target/rated-load.json}"

if [[ ! -x "$HYPHA" ]]; then
  echo "Error: no release build at $HYPHA; run ./build.sh first" >&2
  exit 2
fi
mkdir -p "$(dirname "$report")" || exit 2

status=0
"$HYPHA" load examples/load/rated.toml --report "$report" || status=$?
echo "rated load: exit $status, report $report"
exit "$status"
