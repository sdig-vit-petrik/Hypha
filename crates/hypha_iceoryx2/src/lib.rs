//! The transport port over iceoryx2 shared memory, the only crate that names
//! iceoryx2.
//!
//! [`Iox2Transport`] is one session: one iceoryx2 node, one service per stream, which is
//! one topic name and one schema. [`stream_service_name`] names it: the topic `/odom` of
//! the schema hash `h` is the service `odom@<h in 16 hex digits>`. Each sample carries
//! its hypha frame header as the iceoryx2 user header.
//!
//! A topic's policy (`qos.rs`) is stamped on the service by its creator, and an
//! opener that declares a different one is refused.
//!
//! The segment in `/dev/shm/iox2_*` survives a process that stops without
//! cleanup. A [`PortError::Backend`](hypha_core::port::PortError::Backend) with
//! code 2 directly after a capacity or iceoryx2 version change is almost always a
//! stale segment: run `hypha clean` with the domain stopped.
//!
//! Containers must share `/dev/shm` and `/tmp/iceoryx2` and the same numeric UID
//! and GID; sharing only one path is not a supported deployment.
//!
//! An endpoint may be created on one thread and served on another, but belongs to one
//! thread at a time.
//!
//! # Backend error codes
//!
//! [`PortError::Backend`](hypha_core::port::PortError::Backend) carries a code from
//! this crate:
//!
//! | Code | Call that failed | Usual cause |
//! |------|------------------|-------------|
//! | 0 | node registration | no segment, or no permission on `/dev/shm` |
//! | 1 | reading the session node | the session is dropped |
//! | 2 | opening or creating the service, a topic's or a service stream's | a stale segment, or a capacity mismatch |
//! | 3 | creating the publisher | the service already holds its policy's `max_publishers` |
//! | 4 | creating the subscriber | the service already holds its policy's `max_subscribers` |
//! | 5 | listing the services | the iceoryx2 registry cannot be read |
//! | 6 | — | Retired: no call returns it. A poisoned lock is read through. |
//! | 7 | notification service or port, the wake-up events and the lock of a service included | capacity, stale resources, or no Unix-socket permission |
//! | 36 | a loan failed inside iceoryx2, or a size has no limit to name | an internal iceoryx2 failure |
//! | 37 | a send to a connection whose data is corrupted | a failed shared-memory connection |
//! | 38 | a send that could not establish a connection to a subscriber | a subscriber that went away mid-send |
//! | 39 | a send that reached some subscribers but not all | a subscriber that cannot take the data |
//! | 40 | a send that failed inside iceoryx2 | an internal iceoryx2 failure |
//! | 41 | the dead run of the node's own name could not be reclaimed, so the node did not start | a crashed run that still holds the name's ports; `hypha clean` with the domain stopped |
//! | 42 | creating a `WaitSet`'s deadline timer (`timerfd_create`, Linux) | a kernel without `timerfd`, or no file descriptor left |
//! | 43 | attaching the deadline timer to the run's native `WaitSet` | the native `WaitSet` is full |
//! | 44 | setting the deadline timer before a wait (`timerfd_settime`) | an internal kernel failure |
//! | 45 | the event of a service's lock name is not that service's lock (it lacks the lock's attribute) | an event of that name that something other than a hypha server created |
//! | 46 | creating a service's server or client port | a stale segment, or an internal iceoryx2 failure |
//! | 47 | loaning, sending or receiving a request or a response | an internal iceoryx2 failure |
//! | 48 | a dead node that holds a service's server could not be reclaimed in time | another process reclaims it at the same moment; try again, or `hypha clean` with the domain stopped |
//!
//! A service (request and response from a `.srv`) is [`Iox2Transport::serve`] and
//! [`Iox2Transport::service_client`] over iceoryx2's request-response pattern, with the
//! same frame header and checks as a topic, one server per name across versions, and
//! sends that never wait (`docs/design/native-services.md`).
//!
//! Every publisher notifies its topic's event service after each successful
//! send. A wildcard subscriber has no event service and is polled. This crate
//! creates no thread. iceoryx2 has no drop counter, so a queue eviction shows
//! only as a sequence gap.
//!
//! Building an [`Iox2Transport`] sets the iceoryx2 log level to `Error` for the
//! whole process and disables iceoryx2 signal handling for its node.
//!
//! # Example
//!
//! ```no_run
//! use hypha_iceoryx2::Iox2Transport;
//! use hypha_core::port::TopicSpec;
//!
//! # fn main() -> Result<(), hypha_core::port::PortError> {
//! let transport = Iox2Transport::with_name("doc_example")?;
//! let spec = TopicSpec::new("/doc/example").keeping(4);
//!
//! let mut publisher = transport.publish(&spec)?;
//! let mut loan = publisher.loan(5)?;
//! loan.as_mut_bytes().copy_from_slice(b"hello");
//! // The header travels as the iceoryx2 user header, sealed with the payload.
//! let metadata = hypha_wire::frame::FrameMetadata {
//!     schema_hash: 0,
//!     publisher_id: 1,
//!     sequence: 1,
//!     transport_sent_nanos: 0,
//!     framework_sent_nanos: 0,
//! };
//! loan.send_frame(metadata, 0)?;
//! # Ok(())
//! # }
//! ```
//!
//! `no_run`: running it would create a real segment.

#![forbid(unsafe_code)]

mod data_service;
mod discovery;
pub mod domain;
mod endpoint;
mod header;
pub mod host_resources;
mod latch;
mod notification;
mod preflight;
mod probe;
mod publisher_nodes;
pub mod qos;
mod refusal_episode;
mod schema;
mod service;
mod service_open;
mod shm_capacity;
mod stream;
mod subscriber;
mod subscriber_port;
mod transport;
pub mod waitset;

pub use discovery::{DiscoveredNode, DiscoveredService, list_nodes, list_service_ports};
pub use domain::MAX_BORROWED_SAMPLES;
pub use endpoint::{Iox2Loan, Iox2Publisher, Iox2Sample};
pub use notification::failed_notifications;
pub use publisher_nodes::node_names;
pub use schema::{
    RecordedStream, invalid_record_count, lookup_schema, schema_file_path, topic_records,
};
pub use service::{
    CallEvent, CallId, CallReport, ClientCounters, DiscoveredServiceStream, Iox2Client, Iox2Server,
    Liveness, ReceivedRequest, RequestHandle, ServerCounters,
};
pub use stream::stream_service_name;
pub use subscriber::Iox2Subscriber;
pub use transport::{GRAPH_EVENT, GraphWatch, Iox2Transport};
pub use waitset::{
    Dispatcher, EventOutcome, Iox2Listener, Iox2WaitSet, Progression, StopHandle, Trigger,
};

/// Holds only for a type the compiler accepts as `Send`.
const fn assert_send<T: Send>() {}

// An executor worker serves endpoints created on another thread.
const _: () = assert_send::<Iox2Transport>();
const _: () = assert_send::<Iox2Publisher>();
const _: () = assert_send::<Iox2Subscriber>();
const _: () = assert_send::<Iox2Loan>();
const _: () = assert_send::<Iox2Sample>();
// A deferred answer may be sent from a worker thread.
const _: () = assert_send::<Iox2Server>();
const _: () = assert_send::<Iox2Client>();

/// The guard of `mutex`, taken as it is when a holder panicked. A shipped build aborts
/// on a panic, so no lock is ever poisoned there: the recovery serves test builds,
/// which unwind (`docs/rust-style.md`, "Locks").
#[inline]
pub(crate) fn locked<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
