#!/usr/bin/env bash
# usage: scripts/test-installed-sdk.sh <prefix>
#
# Builds and runs consumers against an SDK that `cmake --install` put in
# <prefix>, the way ./install.sh and an application see it: find_package(hypha),
# hypha_generate_messages(), logging, runNode, and the
# loader refusing a missing versioned library.

set -e
set -u
set -o pipefail

cd "$(dirname "$0")/.."

PREFIX="$(cd "${1:?usage: $0 <prefix>}" && pwd)"
readonly PREFIX
WORK_DIR="$(mktemp -d)"
readonly WORK_DIR
trap 'rm -rf "${WORK_DIR}"' EXIT

abi_revision="$(tr -d '[:space:]' < abi.revision)"
if [ "$(uname -s)" = "Darwin" ]; then
  ffi_name="libhypha_ffi.${abi_revision}.dylib"
else
  ffi_name="libhypha_ffi.so.${abi_revision}"
fi
installed_ffi="$(find "${PREFIX}" -name "${ffi_name}" -type f -print -quit)"
if [ -z "${installed_ffi}" ]; then
  echo "ERROR: installed versioned library ${ffi_name} is missing." >&2
  exit 1
fi
installed_lib_dir="$(dirname "${installed_ffi}")"
if [ -e "${installed_lib_dir}/libhypha_ffi.so" ] || [ -e "${installed_lib_dir}/libhypha_ffi.dylib" ]; then
  echo "ERROR: install prefix contains an unversioned runtime library alias." >&2
  exit 1
fi

echo "==> Building a consumer with its own messages against the installed prefix..."
cmake -S test/package_consumer -B "${WORK_DIR}/consumer" \
  -DCMAKE_PREFIX_PATH="${PREFIX}"
cmake --build "${WORK_DIR}/consumer"
"${WORK_DIR}/consumer/hypha_package_consumer"

# An installed tool is a target too: hypha::topic runs the prefix's hypha topic.
echo "==> Running hypha::topic from the installed package..."
mkdir -p "${WORK_DIR}/tools"
cat > "${WORK_DIR}/tools/CMakeLists.txt" <<'CMAKE'
cmake_minimum_required(VERSION 3.20)
project(hypha_tools_consumer LANGUAGES CXX)
find_package(hypha CONFIG REQUIRED)
add_custom_target(topic_help ALL COMMAND hypha::topic --help VERBATIM)
CMAKE
cmake -S "${WORK_DIR}/tools" -B "${WORK_DIR}/tools/build" -DCMAKE_PREFIX_PATH="${PREFIX}"
cmake --build "${WORK_DIR}/tools/build"

# The installed package finds Threads itself. A consumer that links only
# hypha::sdk must build and run without naming a Threads target.
echo "==> Building and running a consumer that links only hypha::sdk..."
readonly SDK_ONLY_DIR="${WORK_DIR}/sdk-only-src"
mkdir -p "${SDK_ONLY_DIR}"
cat >"${SDK_ONLY_DIR}/CMakeLists.txt" <<'EOF_SDK_ONLY'
cmake_minimum_required(VERSION 3.20)
project(hypha_sdk_only LANGUAGES CXX)
find_package(hypha CONFIG REQUIRED)
add_executable(hypha_sdk_only main.cpp)
target_link_libraries(hypha_sdk_only PRIVATE hypha::sdk)
EOF_SDK_ONLY
cat >"${SDK_ONLY_DIR}/main.cpp" <<'EOF_SDK_ONLY'
#include <hypha_base/clock.h>

int main()
{
    // The clock is a C ABI call, so the program loads libhypha_ffi.so at run time.
    (void)hypha::steadyNow();
    return 0;
}
EOF_SDK_ONLY
cmake -S "${SDK_ONLY_DIR}" -B "${WORK_DIR}/sdk-only" -DCMAKE_PREFIX_PATH="${PREFIX}"
cmake --build "${WORK_DIR}/sdk-only"
"${WORK_DIR}/sdk-only/hypha_sdk_only"

# The consumer's dependency names the ABI revision in its loader metadata. With
# that exact artifact missing, the loader must refuse to start the executable.
mv "${installed_ffi}" "${installed_ffi}.hidden"
set +e
"${WORK_DIR}/sdk-only/hypha_sdk_only" >/dev/null 2>&1
missing_library_status=$?
set -e
mv "${installed_ffi}.hidden" "${installed_ffi}"
if [ "${missing_library_status}" -eq 0 ]; then
  echo "ERROR: SDK consumer ran without its versioned hypha library." >&2
  exit 1
fi

echo "==> Building a logging consumer that links no Threads target itself..."
readonly LOG_CONSUMER_DIR="${WORK_DIR}/log-consumer-src"
mkdir -p "${LOG_CONSUMER_DIR}"
cat >"${LOG_CONSUMER_DIR}/CMakeLists.txt" <<'EOF_LOG_CONSUMER'
cmake_minimum_required(VERSION 3.20)
project(hypha_log_consumer LANGUAGES CXX)
find_package(hypha CONFIG REQUIRED)
add_executable(hypha_log_consumer main.cpp)
target_link_libraries(hypha_log_consumer PRIVATE hypha::sdk)
EOF_LOG_CONSUMER
cat >"${LOG_CONSUMER_DIR}/main.cpp" <<'EOF_LOG_CONSUMER'
#include <hypha/metrics.h>
#include <hypha_log/hypha_log.h>

HYPHA_LOG_MODULE_INIT("log_consumer")

int main()
{
    HYPHA_LOG_INFO("consumer started: value={}", 1);
    const std::optional<uint64_t> lines = hypha::metric("log.lines_written");
    const bool written = lines.has_value() && *lines == 1;
    return written ? 0 : 1;
}
EOF_LOG_CONSUMER
cmake -S "${LOG_CONSUMER_DIR}" -B "${WORK_DIR}/log-consumer" \
  -DCMAKE_PREFIX_PATH="${PREFIX}"
cmake --build "${WORK_DIR}/log-consumer"
"${WORK_DIR}/log-consumer/hypha_log_consumer"

echo "==> Building and running a runNode program from the install prefix..."
readonly RUN_NODE_DIR="${WORK_DIR}/run-node-src"
mkdir -p "${RUN_NODE_DIR}"
cat >"${RUN_NODE_DIR}/CMakeLists.txt" <<'EOF_RUN_NODE'
cmake_minimum_required(VERSION 3.20)
project(hypha_run_node_probe LANGUAGES CXX)
find_package(hypha CONFIG REQUIRED)
add_executable(run_node_probe main.cpp)
target_link_libraries(run_node_probe PRIVATE hypha::sdk)
EOF_RUN_NODE
cat >"${RUN_NODE_DIR}/main.cpp" <<'EOF_RUN_NODE'
#include <hypha/hypha.h>

// The probe stops its own run from a timer, so it needs no other process.
struct StopRun
{
    void operator()() const
    {
        hypha::requestStop();
    }
};

class Probe final : public hypha::CycleNode
{
  public:
    void setup(hypha::Node& node) override
    {
        this->setRate(hypha::Hz(100.0));
        this->stop = node.timer(hypha::Duration::fromMillis(20), StopRun{});
    }

    void step(const hypha::CycleTime&) override
    {
    }

  private:
    hypha::Timer stop;
};

int main(int argc, char** argv)
{
    return hypha::runNode<Probe>(argc, argv, "integrator_probe_node");
}
EOF_RUN_NODE
cmake -S "${RUN_NODE_DIR}" -B "${WORK_DIR}/run-node" -DCMAKE_PREFIX_PATH="${PREFIX}"
cmake --build "${WORK_DIR}/run-node"
"${WORK_DIR}/run-node/run_node_probe"
set +e
"${WORK_DIR}/run-node/run_node_probe" first.toml extra.toml >/dev/null 2>&1
readonly RUN_NODE_USAGE=$?
set -e
if [ "${RUN_NODE_USAGE}" -ne 2 ]; then
  echo "ERROR: runNode with two arguments returned ${RUN_NODE_USAGE}, expected 2." >&2
  exit 1
fi

echo "==> Installed SDK: OK"
