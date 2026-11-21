#!/usr/bin/env bash
# usage: scripts/in-image.sh <command> [args...]
#
# Runs a command in the builder image of docker/Dockerfile (built or reused by
# docker/build.sh) in the current directory of this checkout. The repository is
# mounted at its own path, the main one too when this is a worktree: the cargo
# cache and git's worktree links hold absolute paths. The process runs as you,
# with a 2 GB /dev/shm for the transport tests and without seccomp for TSan.
# CMAKE_BUILD_PARALLEL_LEVEL defaults to one C++ compile per 2 GB of Docker's memory:
# GCC needs more than 1 GB a C++23 unit, and a job per core runs out of memory.
# CTEST_PARALLEL_LEVEL defaults to the same number: a sanitized test takes several times
# its memory.
#
# On a host that is not Linux, the build outputs of this checkout (build/, build-asan/,
# build-tsan/, target/, coverage/, fuzz/target/) and the cargo registry live in Docker
# volumes (hypha-<dir>-<checkout hash>), not on the host:
# they hold Linux artifacts, and Docker Desktop's file sharing serves stale entries for a
# file deleted and written again, so a rebuild in place fails (missing rlibs, failed
# links). Read their files through this script, e.g. in-image.sh cat target/x.
# On Linux they are the host's directories; the registry is target/linux-cargo-registry.
#
# Environment: HYPHA_BUILDER_IMAGE uses that image instead of building one.
set -euo pipefail

if [[ $# -eq 0 ]]; then
  sed -n '2,21p' "$0" >&2
  exit 2
fi

top="$(git rev-parse --show-toplevel)"
main="$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")"
image="${HYPHA_BUILDER_IMAGE:-}"
if [[ -z "$image" ]]; then
  log="$(mktemp)"
  trap 'rm -f "$log"' EXIT
  image="$("$top/docker/build.sh" builder 2>"$log")" || { cat "$log" >&2; exit 1; }
fi

mounts=(-v "$main:$main")
if [[ "$(uname)" == Linux ]]; then
  mkdir -p "$main/target/linux-cargo-registry"
  mounts+=(-v "$main/target/linux-cargo-registry:/usr/local/cargo/registry")
else
  key="$(printf '%s' "$top" | shasum | cut -c1-12)"
  volumes=("hypha-registry-$key:/usr/local/cargo/registry")
  for dir in build build-asan build-tsan target coverage fuzz/target; do
    volumes+=("hypha-${dir//\//-}-$key:$top/$dir")
  done
  owner=()
  owned=()
  for volume in "${volumes[@]}"; do
    mounts+=(-v "$volume")
    owner+=(-v "${volume%%:*}:/own/${volume%%:*}")
    owned+=("/own/${volume%%:*}")
  done
  # A new volume belongs to root; the build runs as you.
  docker run --rm --user 0 "${owner[@]}" "$image" chown "$(id -u):$(id -g)" "${owned[@]}" >/dev/null
fi

memory_gb=$(($(docker info --format '{{.MemTotal}}') / 1024 / 1024 / 1024))
jobs="${CMAKE_BUILD_PARALLEL_LEVEL:-$((memory_gb / 2 > 0 ? memory_gb / 2 : 1))}"
tty=()
[[ -t 0 && -t 1 ]] && tty=(-it)

docker run --rm "${tty[@]}" "${mounts[@]}" -w "$PWD" \
  --user "$(id -u):$(id -g)" -e HOME=/tmp -e CMAKE_BUILD_PARALLEL_LEVEL="$jobs" \
  -e CTEST_PARALLEL_LEVEL="${CTEST_PARALLEL_LEVEL:-$jobs}" \
  --shm-size=2g --security-opt seccomp=unconfined \
  "$image" "$@"
