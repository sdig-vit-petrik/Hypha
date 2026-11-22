#!/usr/bin/env bash
# usage: replay_check.sh [--hypha <hypha>] [--processor <hypha_cloud_processor>]
#                        [--compare <sensor_pipeline_compare>] [--fixture <inputs.mcap>]
#                        [--work <directory>]
#
# Replays the recorded inputs into hypha_cloud_processor three times on simulated time,
# from the build tree of ./build.sh, and records the output of each run. The processor
# takes part in the replay lockstep (#72): the player waits for its callbacks.
#   run_a, run_b  processor.toml: both outputs must match by header stamp
#   slow          processor_slow.toml: the stalled node must set the replay's rate and
#                 lose no IMU sample and no timer period; its worker thread, outside the
#                 lockstep, may still drop clouds at its hand-over queue
# The run uses its own bus domain, sp_<pid>, and stops only the processes it started.
# Exit 0 when every check passes, 1 otherwise.
set -euo pipefail

example_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd "${example_dir}/../.." && pwd)"
hypha_bin="${repo_dir}/target/release/hypha"
processor_bin="${repo_dir}/build/examples/sensor_pipeline/hypha_cloud_processor"
compare_bin="${repo_dir}/build/examples/sensor_pipeline/sensor_pipeline_compare"
fixture="${example_dir}/fixture/sensor_inputs.mcap"
work_dir=""

while [ "$#" -gt 0 ]; do
    case "$1" in
        --hypha) hypha_bin="$2"; shift 2 ;;
        --processor) processor_bin="$2"; shift 2 ;;
        --compare) compare_bin="$2"; shift 2 ;;
        --fixture) fixture="$2"; shift 2 ;;
        --work) work_dir="$2"; shift 2 ;;
        -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
        *) echo "replay_check: unknown argument '$1'" >&2; exit 2 ;;
    esac
done

for program in "${hypha_bin}" "${processor_bin}" "${compare_bin}"; do
    if [ ! -x "${program}" ]; then
        echo "replay_check: ${program} is not an executable; build with ./build.sh --tests" >&2
        exit 2
    fi
done
if [ ! -f "${fixture}" ]; then
    echo "replay_check: no fixture ${fixture}" >&2
    exit 2
fi
if [ -z "${work_dir}" ]; then
    work_dir="$(mktemp -d)"
fi
mkdir -p "${work_dir}"

config_dir="${example_dir}/config"
input_topics=(/imu /lidar/points /tf /tf_static)
output_topic="/cloud_processor/centroid"
# The worker's arithmetic is the same on every run, so the outputs are equal; the
# tolerance only absorbs a reordering of floating-point sums.
tolerance_m="1e-9"
# The longest wait for a ready line, in seconds of host time. A bound, not a pace.
start_limit_s=30
domain="sp_$$"
export HYPHA_DOMAIN="${domain}"

started_pids=()

# Stops, with SIGTERM, every process this script started that still runs, and waits for it.
stop_started() {
    local pid
    for pid in "${started_pids[@]+"${started_pids[@]}"}"; do
        if kill -0 "${pid}" 2>/dev/null; then
            kill -TERM "${pid}" 2>/dev/null || true
            wait "${pid}" 2>/dev/null || true
        fi
    done
    started_pids=()
}

# Removes the shared-memory files of this run's domain, unless a process still holds one.
remove_domain_files() {
    local files=()
    local file
    while IFS= read -r file; do
        files+=("${file}")
    done < <(find /dev/shm /tmp/iceoryx2 -maxdepth 2 -name "iox2-${domain}-*" 2>/dev/null || true)
    if [ "${#files[@]}" -eq 0 ]; then
        return 0
    fi
    if command -v lsof >/dev/null 2>&1; then
        local holders
        holders="$(lsof -t "${files[@]}" 2>/dev/null || true)"
        if [ -n "${holders}" ]; then
            echo "replay_check: domain ${domain} is still in use by: ${holders}; left in place" >&2
            return 1
        fi
    fi
    rm -rf "${files[@]}"
}

on_exit() {
    local status=$?
    stop_started
    remove_domain_files || status=1
    exit "${status}"
}
trap on_exit EXIT

# Waits until file $2 holds a line that starts with $3, while process $1 runs.
wait_for_line() {
    local pid="$1"
    local file="$2"
    local prefix="$3"
    local deadline=$((SECONDS + start_limit_s))
    while ! grep -q "^${prefix}" "${file}" 2>/dev/null; do
        if ! kill -0 "${pid}" 2>/dev/null; then
            echo "replay_check: the process that writes ${file} ended before '${prefix}'" >&2
            cat "${file}" >&2 || true
            return 1
        fi
        if [ "${SECONDS}" -ge "${deadline}" ]; then
            echo "replay_check: no '${prefix}' in ${file} after ${start_limit_s} s" >&2
            return 1
        fi
        sleep 0.05
    done
}

# The value of key $3 on the line of $1 that starts with "final" and names $2.
final_value() {
    local file="$1"
    local subject="$2"
    local key="$3"
    grep "^final .* ${subject} " "${file}" | tr ' ' '\n' | sed -n "s/^${key}=//p"
}

# Replays the fixture into the processor with config $2; the output goes to $1.mcap.
replay_once() {
    local run="$1"
    local config="$2"
    local node_out="${work_dir}/${run}.processor.out"
    local node_err="${work_dir}/${run}.processor.err"
    local record_out="${work_dir}/${run}.record.out"
    local recording="${work_dir}/${run}.mcap"
    rm -f "${recording}"

    # 1. The node first: it creates its topics with the policies of sensor_pipeline_qos.h.
    HYPHA_USE_SIM_TIME=1 "${processor_bin}" "${config}" >"${node_out}" 2>"${node_err}" &
    local node_pid=$!
    started_pids+=("${node_pid}")
    wait_for_line "${node_pid}" "${node_out}" "cloud_processor: ready"

    # 2. The recorder, a participant of the replay: the player waits for it each chunk.
    "${hypha_bin}" record --sync --topics "${output_topic}" --compression none \
        -o "${recording}" >"${record_out}" 2>&1 &
    local record_pid=$!
    started_pids+=("${record_pid}")
    wait_for_line "${record_pid}" "${record_out}" "Following"

    # 3. The player, in the foreground: /clock, the inputs only, at the recorded pace, and
    #    no faster than the processor and the recorder answer.
    echo "replay_check: ${run}: hypha play --clock --rate 1 --nodes hypha_record cloud_processor"
    "${hypha_bin}" play --input "${fixture}" --clock --rate 1 --settle 0.5 \
        --nodes hypha_record cloud_processor --topics "${input_topics[@]}" \
        </dev/null >"${work_dir}/${run}.play.out" 2>&1
    grep "effective rate" "${work_dir}/${run}.play.out" || true

    # 4. Stop the node, then the recorder, which then finishes its file.
    kill -TERM "${node_pid}"
    local node_status=0
    wait "${node_pid}" || node_status=$?
    kill -TERM "${record_pid}"
    local record_status=0
    wait "${record_pid}" || record_status=$?
    started_pids=()
    grep "^final" "${node_out}"
    if [ "${node_status}" -ne 0 ] || [ "${record_status}" -ne 0 ]; then
        echo "replay_check: ${run}: node exit ${node_status}, recorder exit ${record_status}" >&2
        return 1
    fi
}

clouds="$("${compare_bin}" clouds "${fixture}" /lidar/points)"
echo "replay_check: fixture ${fixture}: ${clouds} clouds; domain ${domain}; work ${work_dir}"

failures=0

# A normal run accounts for every cloud it received: processed or dropped at the hand-over,
# and every processed cloud published. The replay paces the node's callbacks (#72), not its
# worker thread, so a loaded host may still make the hand-over drop; that is counted.
check_clean_run() {
    local run="$1"
    local out="${work_dir}/${run}.processor.out"
    local received processed dropped published failed bad static_state
    received="$(final_value "${out}" cloud received)"
    processed="$(final_value "${out}" node processed)"
    dropped="$(final_value "${out}" node handover_dropped)"
    published="$(final_value "${out}" node published)"
    failed="$(final_value "${out}" node publish_failed)"
    bad="$(final_value "${out}" node bad_cloud)"
    static_state="$(final_value "${out}" tf static_base_link_lidar)"
    if [ "${received:-x}" != "$((processed + dropped))" ] || [ "${processed}" != "${published}" ] ||
        [ "${failed}" != 0 ] || [ "${bad}" != 0 ] || [ "${received}" -eq 0 ] ||
        [ "${static_state}" != found ]; then
        echo "replay_check: ${run}: received ${received}, processed ${processed}, dropped" \
            "${dropped}, published ${published}, failed ${failed}, bad ${bad}, static" \
            "${static_state}" >&2
        failures=$((failures + 1))
    fi
}

# A run that lost no cloud: every cloud of the fixture published.
complete_run() {
    local out="${work_dir}/$1.processor.out"
    [ "$(final_value "${out}" cloud received)" = "${clouds}" ] &&
        [ "$(final_value "${out}" node published)" = "${clouds}" ]
}

replay_once run_a "${config_dir}/processor.toml"
check_clean_run run_a
replay_once run_b "${config_dir}/processor.toml"
check_clean_run run_b

# Two runs on one fixture: identical when neither lost a cloud; otherwise every centroid
# that both runs published is the same.
if complete_run run_a && complete_run run_b; then
    comparison=compare
else
    comparison=common
fi
if ! "${compare_bin}" "${comparison}" "${work_dir}/run_a.mcap" "${work_dir}/run_b.mcap" \
    "${output_topic}" "${tolerance_m}"; then
    failures=$((failures + 1))
fi

# Slow mode: the stalled node paces the replay instead of losing data. Its callbacks lose no
# IMU sample and miss no timer period, and the player names it as what set the rate. Its
# worker thread is outside the lockstep: what the hand-over dropped is reported, not checked.
replay_once slow "${config_dir}/processor_slow.toml"
slow_out="${work_dir}/slow.processor.out"
imu_missing="$(final_value "${slow_out}" imu missing)"
timer_missed="$(final_value "${slow_out}" node timer_missed)"
dropped="$(final_value "${slow_out}" node handover_dropped)"
rate_line="$(grep "effective rate" "${work_dir}/slow.play.out" || true)"
echo "replay_check: slow: imu missing ${imu_missing}, timer periods missed ${timer_missed}," \
    "clouds dropped at the hand-over ${dropped}"
if [ "${imu_missing:-x}" != 0 ] || [ "${timer_missed:-x}" != 0 ] ||
    [[ "${rate_line}" != *"set by 'cloud_processor'"* ]]; then
    echo "replay_check: slow: imu missing ${imu_missing}, timer missed ${timer_missed}," \
        "rate '${rate_line}': the node did not pace the replay" >&2
    failures=$((failures + 1))
fi

if [ "${failures}" -ne 0 ]; then
    echo "replay_check: FAILED (${failures} check(s)); logs in ${work_dir}" >&2
    exit 1
fi
echo "replay_check: passed"
