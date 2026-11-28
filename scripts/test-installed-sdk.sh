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

# Message headers sit at their ROS 1 path, <package>/<Name>.h, with no prefix.
echo "==> Checking the installed message header layout..."
for header in include/std_msgs/Header.h include/hypha_msgs/NodeState.h include/hypha_msgs/ros_time.h; do
  if [ ! -f "${PREFIX}/${header}" ]; then
    echo "ERROR: installed ${header} is missing." >&2
    exit 1
  fi
done
# The companion headers of sensor_msgs sit beside the generated messages, under both names.
for header in point_cloud2_iterator point_field_conversion image_encodings distortion_models; do
  for extension in h hpp; do
    if [ ! -f "${PREFIX}/include/sensor_msgs/${header}.${extension}" ]; then
      echo "ERROR: installed sensor_msgs/${header}.${extension} is missing." >&2
      exit 1
    fi
  done
done
if [ ! -f "${PREFIX}/include/sensor_msgs/impl/point_cloud2_iterator.h" ]; then
  echo "ERROR: installed sensor_msgs/impl/point_cloud2_iterator.h is missing." >&2
  exit 1
fi
if [ -e "${PREFIX}/include/sensor_msgs/vendor.toml" ] || [ -e "${PREFIX}/include/vendor.toml" ]; then
  echo "ERROR: a vendor manifest was installed." >&2
  exit 1
fi
if [ -e "${PREFIX}/include/hypha_msgs/std_msgs" ]; then
  echo "ERROR: message headers are still installed under the hypha_msgs/ prefix." >&2
  exit 1
fi
# The install names the headers this build generated: a file an older build left in
# the generated tree (test.sh plants this one) is not installed (#161).
if [ -e "${PREFIX}/include/std_msgs/RemovedMessage.h" ]; then
  echo "ERROR: a stale header of the build tree was installed." >&2
  exit 1
fi

# The SDK owns the directory of each standard package: an application that declares
# std_msgs from a directory of its own is refused at configure time.
echo "==> Checking that a second declaration of a standard package is refused..."
readonly REDECLARE_DIR="${WORK_DIR}/redeclare-src"
mkdir -p "${REDECLARE_DIR}/std_msgs"
printf 'float64 data\n' >"${REDECLARE_DIR}/std_msgs/Float64.msg"
cat >"${REDECLARE_DIR}/CMakeLists.txt" <<'EOF_REDECLARE'
cmake_minimum_required(VERSION 3.20)
project(hypha_redeclare LANGUAGES CXX)
find_package(hypha CONFIG REQUIRED)
hypha_generate_messages(PACKAGE std_msgs MSG_DIR "${CMAKE_CURRENT_SOURCE_DIR}/std_msgs")
EOF_REDECLARE
set +e
redeclare_output="$(cmake -S "${REDECLARE_DIR}" -B "${WORK_DIR}/redeclare" \
  -DCMAKE_PREFIX_PATH="${PREFIX}" 2>&1)"
redeclare_status=$?
set -e
if [ "${redeclare_status}" -eq 0 ]; then
  echo "ERROR: a second declaration of std_msgs was accepted." >&2
  exit 1
fi
# CMake wraps a long message, so the lines are joined before the match.
if ! tr -s '[:space:]' ' ' <<<"${redeclare_output}" | grep -q "package 'std_msgs' is declared twice"; then
  echo "ERROR: the second declaration of std_msgs failed for another reason:" >&2
  echo "${redeclare_output}" >&2
  exit 1
fi

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

# The short names: hy is hypha, and every tool `hypha --help` lists has its link
# hy<tool>; no old hypha_<tool> link is installed.
echo "==> Checking the installed short names hy and hy<tool>..."
for tool in $("${PREFIX}/bin/hypha" --help | sed -n '/^Tools:$/,/^$/s/^  \([a-z]*\) .*/\1/p'); do
  if [ ! -L "${PREFIX}/bin/hy${tool}" ]; then
    echo "ERROR: the installed link hy${tool} is missing." >&2
    exit 1
  fi
done
old_links="$(find "${PREFIX}/bin" -type l -name 'hypha_*' -print)"
if [ -n "${old_links}" ]; then
  echo "ERROR: old tool links are installed: ${old_links}" >&2
  exit 1
fi
readonly SHORT_DOMAIN="t_$$_sdk_hy"
HYPHA_DOMAIN="${SHORT_DOMAIN}" "${PREFIX}/bin/hy" topic list
HYPHA_DOMAIN="${SHORT_DOMAIN}" "${PREFIX}/bin/hytopic" list
"${PREFIX}/bin/hy" clean --domain "${SHORT_DOMAIN}"

# The installed hypha clean removes a domain nobody uses, then has nothing to remove.
echo "==> Running the installed hypha clean on a scratch test domain..."
readonly CLEAN_DOMAIN="t_$$_sdk_clean"
readonly CLEAN_FILE="/tmp/iceoryx2/iox2-${CLEAN_DOMAIN}-x.event"
mkdir -p /tmp/iceoryx2
: >"${CLEAN_FILE}"
"${PREFIX}/bin/hypha" clean --domain "${CLEAN_DOMAIN}"
if [ -e "${CLEAN_FILE}" ]; then
  echo "ERROR: the installed hypha clean left ${CLEAN_FILE}." >&2
  exit 1
fi
clean_output="$("${PREFIX}/bin/hypha" clean --domain "${CLEAN_DOMAIN}")"
if [[ "${clean_output}" != *"nothing to remove"* ]]; then
  echo "ERROR: a second hypha clean did not report nothing to remove: ${clean_output}" >&2
  exit 1
fi

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

echo "==> Building, installing, and running a hypha_add_node() program from the install prefix..."
readonly RUN_NODE_DIR="${WORK_DIR}/run-node-src"
mkdir -p "${RUN_NODE_DIR}"
cat >"${RUN_NODE_DIR}/CMakeLists.txt" <<'EOF_RUN_NODE'
cmake_minimum_required(VERSION 3.20)
project(hypha_run_node_probe LANGUAGES CXX)
find_package(hypha CONFIG REQUIRED)
hypha_add_node(run_node_probe main.cpp)
install(TARGETS run_node_probe RUNTIME DESTINATION bin)
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
cmake --install "${WORK_DIR}/run-node" --prefix "${WORK_DIR}/run-node-installed"
"${WORK_DIR}/run-node-installed/bin/run_node_probe"
set +e
"${WORK_DIR}/run-node-installed/bin/run_node_probe" first.toml extra.toml >/dev/null 2>&1
readonly RUN_NODE_USAGE=$?
set -e
if [ "${RUN_NODE_USAGE}" -ne 2 ]; then
  echo "ERROR: runNode with two arguments returned ${RUN_NODE_USAGE}, expected 2." >&2
  exit 1
fi

echo "==> Installed SDK: OK"
