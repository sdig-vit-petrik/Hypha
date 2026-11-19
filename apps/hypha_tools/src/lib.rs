//! Operator tools for hypha: bus introspection, recording and replay, launch
//! supervision, and a Foxglove Studio bridge.
//!
//! | Binary | What it does |
//! |---|---|
//! | `hypha topic` | Bus introspection. `list` names the topics that publish while it watches. `echo` prints each message as named fields, or with `-p` as one CSV row. `hz` reports the publish cadence. `bw` reports bandwidth and mean message size. |
//! | `hypha top` | Full-screen bus monitor. Shows per-topic rate and bandwidth, the node registry, and a message inspector. |
//! | `hypha node` | Node registry query. `list` shows every node the registry holds, alive or dead. `info` shows the state, PID, and executable of each node of one name. |
//! | `hypha record` | Writes live bus traffic into an MCAP file. Each topic's schema goes into the file, so a recording decodes without the node that made it. |
//! | `hypha play` | Replays an MCAP file onto the bus in log-time order, paced by the recording's own timeline. Keys pause, single-step, change the rate, and restart. |
//! | `hypha launch` | Runs a set of processes from a TOML file, each in its own window of one tmux session, with a restart policy per unit. |
//! | `hypha param` | Reads and edits a configuration file on disk. It does not touch the bus. |
//! | `hypha foxglove` | Serves the bus to Foxglove Studio over a WebSocket. |
pub mod echo_args;
pub mod foxglove;
pub mod intercept_doc;
pub mod intercept_relay;
pub mod launch_cli;
pub mod launch_session;
pub mod launch_supervisor;
pub mod launch_tmux;
pub mod launch_units;
pub mod msg_patch;
pub mod msg_plot;
pub mod msg_reflect;
pub mod play_session;
pub mod player;
pub mod record_session;
pub mod record_sync;
pub mod recorder;
pub mod replay_bus;
pub mod replay_report;
pub mod replay_sync;
pub mod terminal_ui;
pub mod theme;
pub mod topic_follower;
pub mod utc_time;

/// A count, size or time as `f64` for a displayed statistic. Above 2^53 the
/// value rounds; no display needs it exact.
#[must_use]
#[allow(clippy::cast_precision_loss, reason = "a displayed statistic; above 2^53 it rounds")]
pub fn approx_f64(value: u64) -> f64 {
    value as f64
}

/// [`approx_f64`] for a `usize` count.
#[must_use]
pub fn approx_f64_usize(value: usize) -> f64 {
    approx_f64(u64::try_from(value).unwrap_or(u64::MAX))
}

/// Locks `mutex`, taking the data back from a poisoned lock: a panic elsewhere
/// does not take this tool down with it.
pub fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The topic `hypha play --clock` publishes on, matching ROS 1's own
/// `rosbag play --clock` convention: two u32s (sec, nsec), no header.
pub const CLOCK_TOPIC: &str = "/clock";
