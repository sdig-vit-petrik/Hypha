#!/bin/sh
# usage: private-domain.sh <test binary> [args...]
#
# nextest runs every test process through this wrapper. It gives the process a bus
# domain that no other running test shares, under the test root t_ whatever domain
# the caller runs in: the nextest process id and the test's slot. `hypha clean
# --domain t_` removes it once that nextest run ended.
HYPHA_DOMAIN="t_${PPID}_${NEXTEST_TEST_GLOBAL_SLOT:?run by nextest}" exec "$@"
