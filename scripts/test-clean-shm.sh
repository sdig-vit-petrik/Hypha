#!/usr/bin/env bash
# usage: scripts/test-clean-shm.sh
#
# clean-shm.sh --domain removes the resources of the matching domains nobody uses and
# keeps the rest: a domain a live process holds, a domain whose run lives, another
# domain and the default domain. It works on scratch domains of its own under the test
# root t_ and removes what it created.
set -euo pipefail

cd "$(dirname "$0")/.."

prefix="t_shmtest$$_"
dead="${prefix}1_dead"
held="${prefix}1_held"
running="${prefix}$$_run"
other="t_shmtest$$x_other"
default_name="iox2_shmtest$$.event"
node_dir="/tmp/iceoryx2/nodes/shmtest$$"
holder=""

cleanup() {
  if [ -n "${holder}" ]; then
    kill "${holder}" 2>/dev/null || true
    wait "${holder}" 2>/dev/null || true
  fi
  rm -rf /tmp/iceoryx2/iox2-"${prefix}"* /tmp/iceoryx2/nodes/iox2-"${prefix}"* \
    "/tmp/iceoryx2/iox2-${other}-x.event" \
    "/tmp/iceoryx2/${default_name}" "${node_dir}" "/tmp/iceoryx2/nodes/shmtest$$_only"
}
trap cleanup EXIT

fail() {
  echo "test-clean-shm.sh: FAILED: $1" >&2
  exit 1
}

mkdir -p "${node_dir}" "/tmp/iceoryx2/nodes/shmtest$$_only" "/tmp/iceoryx2/iox2-${dead}-_hypha_schemas"
for domain in "${dead}" "${held}" "${running}" "${other}"; do
  : >"/tmp/iceoryx2/iox2-${domain}-x.event"
done
: >"/tmp/iceoryx2/iox2-${dead}-_hypha_schemas/schema"
: >"/tmp/iceoryx2/${default_name}"
# A node directory shares another domain's file with a dead one's; one holds a dead file only.
: >"${node_dir}/iox2-${dead}-1.port_tag"
: >"${node_dir}/iox2_shmtest.port_tag"
: >"/tmp/iceoryx2/nodes/shmtest$$_only/iox2-${dead}-2.port_tag"
: >"/tmp/iceoryx2/nodes/iox2-${held}-1.node_monitor"
# A live process holds the node monitor of the held domain open.
sleep 600 3<"/tmp/iceoryx2/nodes/iox2-${held}-1.node_monitor" &
holder=$!

./clean-shm.sh --domain "${prefix}"

[ ! -e "/tmp/iceoryx2/iox2-${dead}-x.event" ] || fail "a dead domain's file stays"
[ ! -e "/tmp/iceoryx2/iox2-${dead}-_hypha_schemas" ] || fail "a dead domain's directory stays"
[ ! -e "${node_dir}/iox2-${dead}-1.port_tag" ] || fail "a dead domain's node file stays"
[ ! -e "/tmp/iceoryx2/nodes/shmtest$$_only" ] || fail "a node directory it emptied stays"
[ -e "${node_dir}/iox2_shmtest.port_tag" ] || fail "another domain's node file is gone"
[ -e "/tmp/iceoryx2/iox2-${held}-x.event" ] || fail "a domain a live process holds is gone"
[ -e "/tmp/iceoryx2/nodes/iox2-${held}-1.node_monitor" ] || fail "a held file is gone"
[ -e "/tmp/iceoryx2/iox2-${running}-x.event" ] || fail "the domain of a live run is gone"
[ -e "/tmp/iceoryx2/iox2-${other}-x.event" ] || fail "a domain outside the prefix is gone"
[ -e "/tmp/iceoryx2/${default_name}" ] || fail "a default-domain file is gone"
if ./clean-shm.sh --domain "Not-A-Prefix" 2>/dev/null; then
  fail "an invalid prefix is accepted"
fi
echo "test-clean-shm.sh: passed"
