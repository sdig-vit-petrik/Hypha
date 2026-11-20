#!/usr/bin/env bash
# usage: docker/build.sh [--push] <builder|runtime>...
#        docker/build.sh manifest
#
# Builds builder or runtime from docker/Dockerfile for the architecture of this
# host and prints each reference (<image>:<version>-<arch>) on stdout. hypha does
# not cross-build: amd64 builds on an amd64 host, arm64 on an arm64 host.
#
# manifest joins the pushed -amd64 and -arm64 images of both into the multi-arch
# references <image>:<version> and pushes them, appending them to image-refs.txt.
#
# Environment:
#   HYPHA_REGISTRY       registry and namespace of the images (default: ghcr.io/sdig-vit-petrik)
#   HYPHA_IMAGE_VERSION  tag (default: the content hash of docker/, so an unchanged
#                        definition reuses its image)
#   HYPHA_IMAGE_TAG      whole tag of builder and runtime, in place of <version>-<arch>
#                        (CI sets develop on the develop branch; local images only)
#   HYPHA_CA_BUNDLE      trust bundle for the builds (default: SSL_CERT_FILE, then the system one,
#                        /etc/ssl/certs/ca-certificates.crt or on macOS /etc/ssl/cert.pem)
#
# --push also appends repository@sha256 references to image-refs.txt.
set -euo pipefail

cd "$(dirname "$0")"

push=false
if [[ "${1:-}" == "--push" ]]; then
  push=true
  shift
fi
if [[ $# -eq 0 ]]; then
  sed -n '2,20p' "$0" >&2
  exit 2
fi

registry="${HYPHA_REGISTRY:-ghcr.io/sdig-vit-petrik}"
version="${HYPHA_IMAGE_VERSION:-$(git rev-parse --short=12 HEAD:docker)}"

if [[ "$1" == manifest ]]; then
  for image in hypha-builder hypha-runtime; do
    reference="$registry/$image:$version"
    docker manifest rm "$reference" >/dev/null 2>&1 || true
    docker manifest create "$reference" "$reference-amd64" "$reference-arm64" >&2
    docker manifest push "$reference" >&2
    echo "$reference" | tee -a ../image-refs.txt
  done
  exit 0
fi

case "$(uname -m)" in
  x86_64|amd64) arch=amd64 ;;
  aarch64|arm64) arch=arm64 ;;
  *) echo "Error: no image for host architecture $(uname -m)" >&2; exit 2 ;;
esac

ca="$(mktemp -d)"
trap 'rm -rf "$ca"' EXIT
system_ca=/etc/ssl/certs/ca-certificates.crt
[[ -f "$system_ca" ]] || system_ca=/etc/ssl/cert.pem # macOS
cp "${HYPHA_CA_BUNDLE:-${SSL_CERT_FILE:-$system_ca}}" "$ca/ca-bundle.crt"

for image in "$@"; do
  case "$image" in
    builder|runtime) reference="$registry/hypha-$image:${HYPHA_IMAGE_TAG:-$version-$arch}" ;;
    *) echo "Error: unknown image $image" >&2; exit 2 ;;
  esac

  docker build . --target "$image" --platform "linux/$arch" \
    --build-context ca="$ca" -t "$reference" \
    --label "org.opencontainers.image.source=$(git config --get remote.origin.url || echo unknown)" \
    --label "org.opencontainers.image.revision=$(git rev-parse HEAD)" \
    --label "org.opencontainers.image.version=$version" >&2

  if [[ "$push" == true ]]; then
    docker push "$reference" >&2
    docker image inspect --format '{{index .RepoDigests 0}}' "$reference" >> ../image-refs.txt
  fi
  echo "$reference"
done
