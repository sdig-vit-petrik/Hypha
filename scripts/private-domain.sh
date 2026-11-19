#!/bin/sh
# usage: private-domain.sh <test binary> [args...]
#
# nextest runs every test process through this wrapper. It gives the process a bus
# domain that no other running test shares: the caller's HYPHA_DOMAIN (or "t"), the
# nextest process id, and the test's slot.
HYPHA_DOMAIN="${HYPHA_DOMAIN:-t}_${PPID}_${NEXTEST_TEST_GLOBAL_SLOT:?run by nextest}" exec "$@"
