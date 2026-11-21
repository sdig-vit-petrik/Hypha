#!/usr/bin/env bash
# usage: install.sh [prefix]
#
# Builds the SDK and the command line tools as this user and installs them to
# prefix (default /usr/local), with sudo only when the prefix is not writable.
set -euo pipefail

cd "$(dirname "$0")"

prefix="${1:-/usr/local}"

cmake -S . -B build/install -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$prefix" \
  -DHYPHA_BUILD_TESTS=OFF -DHYPHA_BUILD_EXAMPLES=OFF
cmake --build build/install --parallel "${CMAKE_BUILD_PARALLEL_LEVEL:-$(getconf _NPROCESSORS_ONLN)}"

if mkdir -p "$prefix" 2>/dev/null && [[ -w "$prefix" ]]; then
  cmake --install build/install
else
  sudo cmake --install build/install
fi
