#!/usr/bin/env bash
# usage: slice_a_replay.sh --input <slice-a.mcap> [--build <dir>] [--hypha <hypha>]
#                          [--work <dir>]
#
# Replays a recording of the concatenated lidar cloud through Slice A of the Autoware
# port, on simulated time and in the replay lockstep (#72): the ground filter, the voxel
# downsample and the clustering, each one process. hypha_record --sync records their
# outputs. Two runs, at different rates and chunk lengths, must give identical outputs,
# message for message; the script then summarizes them. Make the input with hypha convert
# from a ROS 2 bag (README.md, "Slice A end to end").
#
# Each run uses its own bus domain, slicea_<pid>_<run>, and stops only the processes it
# started. Exit 0 when the runs match, 1 when they differ or a process fails, 2 for a
# usage or setup error.
set -euo pipefail

example_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd "${example_dir}/../.." && pwd)"
build_dir="${repo_dir}/build/examples/autoware"
hypha_bin="${repo_dir}/target/release/hypha"
input=""
work_dir=""

while [ "$#" -gt 0 ]; do
    case "$1" in
        --input) input="$2"; shift 2 ;;
        --build) build_dir="$2"; shift 2 ;;
        --hypha) hypha_bin="$2"; shift 2 ;;
        --work) work_dir="$2"; shift 2 ;;
        -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
        *) echo "slice_a_replay: unknown argument '$1'" >&2; exit 2 ;;
    esac
done

ground_bin="${build_dir}/autoware_ground_filter"
voxel_bin="${build_dir}/autoware_voxel_grid_downsample_filter"
cluster_bin="${build_dir}/autoware_voxel_grid_based_euclidean_cluster"
compare_bin="${build_dir}/autoware_slice_a_compare"
for program in "${hypha_bin}" "${ground_bin}" "${voxel_bin}" "${cluster_bin}" "${compare_bin}"; do
    if [ ! -x "${program}" ]; then
        echo "slice_a_replay: ${program} is not an executable" >&2
        exit 2
    fi
done
if [ -z "${input}" ] || [ ! -f "${input}" ]; then
    echo "slice_a_replay: --input names no recording: '${input}'" >&2
    exit 2
fi
if [ -z "${work_dir}" ]; then
    work_dir="$(mktemp -d)"
fi
mkdir -p "${work_dir}"

config_dir="${example_dir}/config/slice_a"
input_topics=(/sensing/lidar/concatenated/pointcloud /tf_static)
output_topics=(
    /perception/obstacle_segmentation/single_frame/pointcloud
    /perception/obstacle_segmentation/pointcloud
    /perception/object_recognition/detection/objects
)
nodes=(ground_filter voxel_grid_downsample_filter voxel_grid_based_euclidean_cluster_node)
# The longest wait for a ready line, in seconds of host time. A bound, not a pace.
start_limit_s=30

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
trap stop_started EXIT

# Waits until file $2 holds a line that starts with $3, while process $1 runs.
wait_for_line() {
    local pid="$1" file="$2" prefix="$3"
    local deadline=$((SECONDS + start_limit_s))
    while ! grep -q "^${prefix}" "${file}" 2>/dev/null; do
        if ! kill -0 "${pid}" 2>/dev/null; then
            echo "slice_a_replay: the process that writes ${file} ended before '${prefix}'" >&2
            cat "${file}" >&2 || true
            return 1
        fi
        if [ "${SECONDS}" -ge "${deadline}" ]; then
            echo "slice_a_replay: no '${prefix}' in ${file} after ${start_limit_s} s" >&2
            return 1
        fi
        sleep 0.05
    done
}

# One replay: run name $1, then the player's rate and chunk length in milliseconds.
replay_once() {
    local run="$1" rate="$2" chunk="$3"
    local recording="${work_dir}/${run}.mcap"
    rm -f "${recording}"
    export HYPHA_DOMAIN="slicea_$$_${run}"

    # The nodes first, on simulated time: they take part in the lockstep.
    local program config
    for program in ground voxel cluster; do
        case "${program}" in
            ground) config="ground_filter.toml"; bin="${ground_bin}" ;;
            voxel) config="voxel_grid_downsample_filter.toml"; bin="${voxel_bin}" ;;
            cluster) config="voxel_grid_based_euclidean_cluster.toml"; bin="${cluster_bin}" ;;
        esac
        HYPHA_USE_SIM_TIME=1 "${bin}" "${config_dir}/${config}" \
            >"${work_dir}/${run}.${program}.log" 2>&1 &
        started_pids+=("$!")
    done

    # The recorder, a participant too: it holds each chunk until it is written.
    "${hypha_bin}" record --sync --topics "${output_topics[@]}" --compression none \
        -o "${recording}" >"${work_dir}/${run}.record.log" 2>&1 &
    local record_pid=$!
    started_pids+=("${record_pid}")
    wait_for_line "${record_pid}" "${work_dir}/${run}.record.log" "Following"

    # The player waits at its start barrier for every named participant.
    echo "slice_a_replay: ${run}: hypha play --clock --rate ${rate} --chunk ${chunk}"
    "${hypha_bin}" play --input "${input}" --clock --rate "${rate}" --chunk "${chunk}" \
        --nodes "${nodes[@]}" hypha_record --topics "${input_topics[@]}" \
        </dev/null >"${work_dir}/${run}.play.log" 2>&1 || {
        echo "slice_a_replay: ${run}: the player failed" >&2
        tail -5 "${work_dir}/${run}.play.log" >&2
        return 1
    }
    grep "effective rate" "${work_dir}/${run}.play.log" || true
    stop_started
}

echo "slice_a_replay: input ${input}; work ${work_dir}"
replay_once run_a max 100
replay_once run_b 2 50

status=0
"${compare_bin}" compare "${work_dir}/run_a.mcap" "${work_dir}/run_b.mcap" || status=1
"${compare_bin}" summary "${work_dir}/run_a.mcap"
if [ "${status}" -ne 0 ]; then
    echo "slice_a_replay: FAILED: the two runs differ; logs in ${work_dir}" >&2
    exit 1
fi
echo "slice_a_replay: passed"
