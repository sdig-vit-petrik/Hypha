#!/usr/bin/env bash
# usage: clean-shm.sh [--force]
#        clean-shm.sh --domain <prefix>
#
# Removes the iceoryx2 resources of this machine: the shared-memory segments
# under /dev/shm (Linux), the shared-memory objects and their state files
# /tmp/iox2*.shm_state (macOS), and the metadata and sockets under /tmp/iceoryx2. A
# process that still holds one of them stops the removal; --force kills it first.
#
# --domain <prefix> removes only the resources of the bus domains whose names start
# with <prefix> (files named iox2-<prefix>*), and only of a domain nobody uses: it
# keeps a domain while a live process holds one of its files, or while the process
# whose id follows the prefix lives (a test domain is t_<pid of its run>_...). It
# kills nothing, never touches another domain, and leaves the default domain alone.
set -euo pipefail

FORCE=false
if [ "${1:-}" = "--force" ]; then
  FORCE=true
fi

if [ "${1:-}" = "--domain" ]; then
  prefix="${2:-}"
  if ! [[ "${prefix}" =~ ^[a-z][a-z0-9_]*$ ]] || [ "$#" -ne 2 ]; then
    echo "usage: clean-shm.sh --domain <prefix>, a prefix of [a-z][a-z0-9_]*" >&2
    exit 2
  fi
  if ! command -v lsof >/dev/null 2>&1; then
    echo "clean-shm.sh: lsof is missing, so live holders cannot be found; nothing removed" >&2
    exit 1
  fi
  held="$(mktemp)"
  trap 'rm -f "${held}"' EXIT
  # The open files of every process: a live node holds its node monitor files.
  {
    if [ -d /tmp/iceoryx2 ]; then
      lsof -F n +D /tmp/iceoryx2 2>/dev/null || true
    fi
    if compgen -G "/dev/shm/iox2-${prefix}*" >/dev/null; then
      lsof -F n /dev/shm/iox2-"${prefix}"* 2>/dev/null || true
    fi
  } | sed -n 's/^n//p' >"${held}"
  status=0
  python3 -I - "${prefix}" "${held}" <<'PY' || status=$?
import os
import re
import shutil
import sys

prefix, held_file = sys.argv[1], sys.argv[2]
# iox2-<domain>-<rest>: no domain name holds a "-".
NAME = re.compile(r"iox2-([a-z][a-z0-9_]*)-")


def domain_of(path):
    match = NAME.match(os.path.basename(path))
    if match is None or not match.group(1).startswith(prefix):
        return None
    return match.group(1)


def run_lives(domain):
    pid = re.match(r"[0-9]+", domain[len(prefix):])
    if pid is None or int(pid.group()) <= 1:
        return False
    try:
        os.kill(int(pid.group()), 0)
    except ProcessLookupError:
        return False
    except PermissionError:  # another user's process
        return True
    return True


with open(held_file, encoding="utf-8", errors="replace") as handle:
    in_use = {domain_of(line.rstrip("\n")) for line in handle} - {None}

# Every resource of a matching domain: a file or directory named iox2-<domain>-*,
# also inside the per-node directories, which another domain's files may share.
resources = []
for root in ("/dev/shm", "/tmp"):
    try:
        names = os.listdir(root)
    except OSError:
        continue
    resources += [os.path.join(root, name) for name in names if name.startswith("iox2-")]
emptied = set()
for root, directories, files in os.walk("/tmp/iceoryx2"):
    for name in directories + files:
        if domain_of(name) is not None:
            resources.append(os.path.join(root, name))
            if os.path.dirname(root) == "/tmp/iceoryx2/nodes":
                emptied.add(root)
    directories[:] = [name for name in directories if domain_of(name) is None]

kept, removed, failed = set(), set(), 0
for path in resources:
    domain = domain_of(path)
    if domain is None:
        continue
    if domain in in_use or run_lives(domain):
        kept.add(domain)
        continue
    try:
        if path.endswith(".shm_state"):
            # A state file holds the name of its kernel object: unlink that, then the file.
            import _posixshmem

            with open(path, "rb") as handle:
                kernel_name = handle.read(255).split(b"\0", 1)[0].decode()
            if kernel_name:
                try:
                    _posixshmem.shm_unlink(kernel_name)
                except FileNotFoundError:
                    pass
        if os.path.isdir(path) and not os.path.islink(path):
            shutil.rmtree(path)
        else:
            os.remove(path)
        removed.add(domain)
    except FileNotFoundError:
        pass
    except OSError as cause:
        failed += 1
        print(f"clean-shm.sh: cannot remove {path}: {cause}", file=sys.stderr)
# A node directory that only the removed files filled goes too; rmdir keeps a full one.
for directory in emptied:
    try:
        os.rmdir(directory)
    except OSError:
        pass
for domain in sorted(kept):
    print(f"clean-shm.sh: kept domain {domain}: in use", file=sys.stderr)
print(f"clean-shm.sh: cleared {len(removed)} domains under '{prefix}', kept {len(kept)}")
sys.exit(1 if failed else 0)
PY
  exit "${status}"
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
