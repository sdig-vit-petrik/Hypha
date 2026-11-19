#!/usr/bin/env bash
# usage: clean-shm.sh [--force]
#
# Removes the iceoryx2 resources of this machine: the shared-memory segments
# under /dev/shm (Linux), the shared-memory objects and their state files
# /tmp/iox2*.shm_state (macOS), and the metadata and sockets under /tmp/iceoryx2. A
# process that still holds one of them stops the removal; --force kills it first.
set -euo pipefail

FORCE=false
if [ "${1:-}" = "--force" ]; then
  FORCE=true
fi

parent_of() {
  local pid="$1"
  if [ -r "/proc/${pid}/stat" ]; then
    awk '{print $4}' "/proc/${pid}/stat"
    return
  fi
  ps -o ppid= -p "${pid}" 2>/dev/null | tr -d ' ' || echo 1
}

own_process_tree() {
  local pid=$$
  while [ -n "${pid}" ] && [ "${pid}" -gt 1 ]; do
    echo "${pid}"
    pid=$(parent_of "${pid}")
  done
}

# A minimal Linux image has /proc and no procps; macOS has ps and no /proc.
describe_process() {
  local pid="$1"
  local command=""
  if [ -r "/proc/${pid}/cmdline" ]; then
    command=$(tr '\0' ' ' <"/proc/${pid}/cmdline" 2>/dev/null || true)
  else
    command=$(ps -o command= -p "${pid}" 2>/dev/null || true)
  fi
  echo "  ${pid} ${command:-<gone>}" >&2
}

list_holders() {
  local segments=()
  if compgen -G "/dev/shm/iox2*" >/dev/null; then
    segments=(/dev/shm/iox2*)
  fi
  if [ "${#segments[@]}" -gt 0 ]; then
    lsof -t "${segments[@]}" 2>/dev/null || true
  fi
  if [ -d /tmp/iceoryx2 ]; then
    lsof -t +D /tmp/iceoryx2 2>/dev/null || true
  fi
}

has_resources=false
if compgen -G "/dev/shm/iox2*" >/dev/null || compgen -G "/tmp/iox2*.shm_state" >/dev/null \
  || [ -d /tmp/iceoryx2 ]; then
  has_resources=true
fi

if [ "${has_resources}" = true ]; then
  if ! command -v lsof >/dev/null 2>&1; then
    if [ "${FORCE}" = false ]; then
      echo "clean-shm.sh: lsof is missing, so live holders cannot be found" >&2
      echo "clean-shm.sh: install lsof, or rerun with --force to remove anyway" >&2
      exit 1
    fi
    echo "clean-shm.sh: lsof is missing; removing without a holder check" >&2
  else
    holders=$(list_holders | grep '^[0-9][0-9]*$' | sort -u || true)
    if [ -n "${holders}" ]; then
      holders=$(comm -23 <(echo "${holders}") <(own_process_tree | sort -u) | tr '\n' ' ')
    fi
    if [ -n "${holders// /}" ]; then
      if [ "${FORCE}" = false ]; then
        echo "clean-shm.sh: still in use by:" >&2
        for pid in ${holders}; do describe_process "${pid}"; done
        echo "clean-shm.sh: rerun with --force to kill them" >&2
        exit 1
      fi
      echo "clean-shm.sh: killing:" >&2
      for pid in ${holders}; do describe_process "${pid}"; done
      kill -TERM ${holders} 2>/dev/null || true
      sleep 1
      kill -KILL ${holders} 2>/dev/null || true
    fi
  fi
fi

rm -rf /dev/shm/iox2* /tmp/iceoryx2
if compgen -G "/tmp/iox2*.shm_state" >/dev/null; then
  # A state file holds the name of its kernel object: unlink that, then the file.
  python3 -I - <<'PY'
import glob
import os
import sys

import _posixshmem

failed = 0
for state in glob.glob("/tmp/iox2*.shm_state"):
    try:
        with open(state, "rb") as handle:
            name = handle.read(255).split(b"\0", 1)[0].decode()
        if name:
            try:
                _posixshmem.shm_unlink(name)
            except FileNotFoundError:
                pass
        os.remove(state)
    except OSError as cause:
        failed += 1
        print(f"clean-shm.sh: cannot remove {state}: {cause}", file=sys.stderr)
sys.exit(1 if failed else 0)
PY
fi
echo "clean-shm.sh: cleared"
