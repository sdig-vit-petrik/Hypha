#!/usr/bin/env bash
# usage: scripts/check-deps.sh [cargo-metadata.json]
#
# The dependency rules of the workspace, checked on `cargo metadata`. A rule names
# a crate and the workspace packages that may depend on it, with any kind of
# dependency (normal, dev, build); "-" means none. A crate of LEAF_CRATES depends
# on no workspace crate. A package under apps/ may depend on the workspace crates of
# APP_CRATES only. Without an argument the
# metadata of this workspace is read; a file argument checks that metadata instead
# (the fixture test).
set -euo pipefail

cd "$(dirname "$0")/.."

# crate          packages that may depend on it
RULES=(
    "hypha_iceoryx2 hypha_node"
    "hypha_sys      hypha_node"
    "hypha_ffi      -"
    "hypha_node     hypha_ffi hypha_tools hypha_ros_bridge"
    "hypha_runtime  hypha_node hypha_ffi"
    "hypha_cycle    hypha_runtime hypha_node hypha_ffi"
    "hypha_testing_mcap -"
)
# Leaf crates depend on no workspace crate.
LEAF_CRATES=(
    "hypha_cycle"
    "hypha_msgdef"
    "hypha_sys"
    "hypha_testing_mcap"
    "hypha_testkit"
    "hypha_time"
    "hypha_wire"
)
# The workspace crates an application or tool may use: hypha_node (its Rust SDK is
# hypha_node::sdk), and the test helpers.
APP_CRATES=("hypha_node" "hypha_testkit")

if [[ $# -ge 1 ]]; then
    metadata="$(cat "$1")"
else
    metadata="$(cargo metadata --format-version 1 --locked --offline --no-deps)"
fi

# One line per edge between workspace packages: "<package> <dependency> <manifest>".
edges="$(python3 -I -c '
import json, sys
metadata = json.load(sys.stdin)
members = {package["name"] for package in metadata["packages"]}
for package in metadata["packages"]:
    for dependency in package["dependencies"]:
        if dependency["name"] in members:
            print(package["name"], dependency["name"], package["manifest_path"])
' <<< "$metadata")"

echo "dependency rules:"
for rule in "${RULES[@]}"; do
    read -r crate allowed <<< "$rule"
    echo "  $crate: ${allowed}"
done
for leaf in "${LEAF_CRATES[@]}"; do
    echo "  $leaf depends on: -"
done
echo "  apps/*: workspace crates ${APP_CRATES[*]}"

failures=0
while read -r package dependency manifest; do
    [[ -n "$package" ]] || continue
    for rule in "${RULES[@]}"; do
        read -r crate allowed <<< "$rule"
        [[ "$dependency" == "$crate" ]] || continue
        permitted=false
        for name in $allowed; do
            if [[ "$package" == "$name" ]]; then
                permitted=true
            fi
        done
        if [[ "$permitted" == false ]]; then
            echo "FAIL: $package depends on $dependency; only ${allowed} may" >&2
            failures=$((failures + 1))
        fi
    done
    for leaf in "${LEAF_CRATES[@]}"; do
        if [[ "$package" == "$leaf" ]]; then
            echo "FAIL: $package depends on $dependency; it depends on no workspace crate" >&2
            failures=$((failures + 1))
        fi
    done
    if [[ "$manifest" == */apps/* ]]; then
        permitted=false
        for name in "${APP_CRATES[@]}"; do
            if [[ "$dependency" == "$name" ]]; then
                permitted=true
            fi
        done
        if [[ "$permitted" == false ]]; then
            echo "FAIL: $package (apps/) depends on $dependency; an application uses ${APP_CRATES[*]}" >&2
            failures=$((failures + 1))
        fi
    fi
done <<< "$edges"

if [[ "$failures" -gt 0 ]]; then
    exit 1
fi
echo "OK: every workspace dependency follows the rules."
