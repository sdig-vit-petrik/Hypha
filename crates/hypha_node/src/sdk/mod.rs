//! The hypha client API: a thin, safe layer over the runtime's safe Rust functions
//! (`crate::safe`), the same functions the C ABI wraps. This crate contains no
//! `unsafe` code.
//!
//! A process must use one SDK: never load the C++ SDK's `libhypha_ffi` and link
//! this crate in the same process.
//!
//! # Writing a node
//!
//! ```no_run
//! use hypha_node::sdk::BusNode;
//!
//! let bus = BusNode::new("demo_node")?;
//! let mut readings = bus.subscribe_raw("/readings", 1)?;
//!
//! loop {
//!     while let Some(sample) = readings.try_receive()? {
//!         println!("{} bytes", sample.bytes().map_or(0, <[u8]>::len));
//!     }
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Raw receive
//!
//! A raw subscriber owns each received transport sample until it drops it.
//!
//! ```no_run
//! use hypha_node::sdk::{BusNode, ReceivedSample, SubscriberOptions};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let bus = BusNode::new("loan_node")?;
//! let mut input = bus.subscribe_raw_with_options(
//!     "/camera/image",
//!     SubscriberOptions::default().keeping(2),
//! )?;
//! let sample: Option<ReceivedSample> = input.try_receive()?;
//! if let Some(sample) = sample {
//!     if let Some(payload) = sample.bytes() {
//!         println!("{} bytes", payload.len());
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Waking on a sample
//!
//! ```no_run
//! use hypha_node::sdk::{BusNode, PublisherOptions, SubscriberOptions};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let bus = BusNode::new("event_node")?;
//! let mut output = bus.publish_raw_configured(
//!     "/scan",
//!     "std_msgs/String",
//!     "example-md5",
//!     "",
//!     PublisherOptions::default(),
//! )?;
//! let mut input = bus.subscribe_raw_with_options(
//!     "/scan",
//!     SubscriberOptions::default().keeping(4),
//! )?;
//!
//! if !output.publish(b"scan") {
//!     eprintln!("the transport refused the sample");
//! }
//! // This subscriber is attached to no WaitSet, so it is read by hand. The
//! // framework creates no thread; nothing drains it behind this loop.
//! while bus.ok() {
//!     if let Some(sample) = input.try_receive()? {
//!         assert_eq!(sample.bytes(), Some(b"scan".as_slice()));
//!         break;
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!

#![forbid(unsafe_code)]

/// `hypha clean`: removing the shared memory of the bus domains nobody uses.
pub mod clean {
    pub use crate::clean::{
        CleanOutcome, CleanReport, DomainSelection, KeepReason, KeptDomain, RemovedDomain, clean,
        domain_label, domain_shared_memory,
    };
}
pub mod clock;
/// The acknowledgement rules of a lockstep `/clock`, shared with the clock driver.
pub use hypha_runtime::clock_barrier;
pub(crate) mod ffi;
mod loan;
mod metrics;
mod options;
mod raw;
pub mod schema;
mod status;
pub mod waitset;

/// The operating-system calls a tool needs that only `unsafe` code can make, reached
/// through `hypha_sys`.
pub mod sys {
    #[cfg(target_os = "linux")]
    pub use hypha_sys::set_parent_death_signal;
    pub use hypha_sys::{ReadOnlyMap, map_read_only};
}

pub use loan::{
    RawSample, RawSubscriber, ReceiveError, ReceivedSample, SampleIdentity, SubscriberCounters,
};
pub use options::{PayloadCapacity, PublisherOptions, SubscriberOptions};
pub use raw::{
    BusNode, GraphWatch, NodeInfo, RawPublisher, TopicInfo, WaitResult, domain, is_cli_node_name,
    list_nodes, topic_matches_pattern,
};
pub use schema::{ResolvedSchema, SCHEMA_RETRY_INTERVAL, SchemaError, SchemaIdentityCache};
pub use waitset::{AttachmentId, Delivery, RunResult, StopReason, WaitSet};

pub use hypha_core::message;
/// The parameter-file loader a node uses: includes, `${NAME}`, duplicate refusal.
pub use hypha_core::parameters;
/// The queue depth of hypha's `default` policy, which a publisher that declares no policy
/// creates its topic with: what a subscriber depth of 0 keeps on such a topic.
pub use hypha_iceoryx2::qos::DEFAULT_MAX_DEPTH;
/// Subscribers a topic admits beyond its policy from endpoints that declare no policy:
/// tools, bridges, `hypha load` workers. A further undeclared subscriber is refused.
pub use hypha_iceoryx2::qos::RESERVED_OBSERVER_SUBSCRIBERS as UNDECLARED_SUBSCRIBER_SLOTS;
pub use hypha_msgdef as msgdef;
pub use hypha_msgdef::md5;
/// The standard message types (`std_msgs`, `geometry_msgs`, ..., `hypha_msgs`).
pub use hypha_msgs as msgs;
pub use hypha_wire::codec;
pub use hypha_wire::ros_time::{RosDuration, RosTime};
pub use hypha_wire::{is_valid_md5, schema_hash_from_md5};
