#!/usr/bin/env bash
# usage: scripts/in-image.sh <command> [args...]
#
# Runs a command in the builder image of docker/Dockerfile (built or reused by
# docker/build.sh) in the current directory of this checkout. The repository is
# mounted at its own path, the main one too when this is a worktree: the cargo
# cache and git's worktree links hold absolute paths. The process runs as you,
# with a 2 GB /dev/shm for the transport tests and without seccomp for TSan.
# The cargo registry is kept in target/linux-cargo-registry of the main checkout.
#
# build/ and target/ belong to one platform: on a host that is not Linux, run
# this in a checkout or worktree the host itself does not build in.
#
# Environment: HYPHA_BUILDER_IMAGE uses that image instead of building one.
set -euo pipefail

if [[ $# -eq 0 ]]; then
  sed -n '2,15p' "$0" >&2
  exit 2
fi

common="$(git rev-parse --path-format=absolute --git-common-dir)"
main="$(dirname "$common")"
image="${HYPHA_BUILDER_IMAGE:-}"
if [[ -z "$image" ]]; then
  log="$(mktemp)"
  trap 'rm -f "$log"' EXIT
  image="$("$(git rev-parse --show-toplevel)/docker/build.sh" builder 2>"$log")" || { cat "$log" >&2; exit 1; }
fi
registry="$main/target/linux-cargo-registry"
mkdir -p "$registry"
tty=()
[[ -t 0 && -t 1 ]] && tty=(-it)

docker run --rm "${tty[@]}" \
  -v "$main:$main" -w "$PWD" \
  -v "$registry:/usr/local/cargo/registry" \
  --user "$(id -u):$(id -g)" -e HOME=/tmp \
  --shm-size=2g --security-opt seccomp=unconfined \
  "$image" "$@"
