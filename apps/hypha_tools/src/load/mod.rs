//! `hypha load`: a load harness. A workload file names worker processes, the topics
//! they publish and subscribe to, the run's times and the pass limits; the coordinator
//! runs it in a private domain and writes one JSON report.
//!
//! The workers use the Rust SDK; the runtime below the C ABI is the one a C++ node
//! uses, and the thin C++ layer is not in the measured path.

pub mod coordinator;
pub mod histogram;
pub mod limits;
pub mod pacing;
pub mod probe;
pub mod protocol;
pub mod report;
pub mod series;
pub mod worker;
pub mod workload;

/// Every limit passed.
pub const EXIT_PASS: u8 = 0;
/// A limit failed.
pub const EXIT_LIMIT_FAILED: u8 = 1;
/// The run itself failed: an invalid command line or workload, a worker that died or
/// hung, a timeout, or an interrupt (`EX_SOFTWARE`).
pub const EXIT_RUN_FAILED: u8 = 70;
