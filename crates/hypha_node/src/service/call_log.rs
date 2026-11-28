//! The call log: each client publishes `hypha_msgs/ServiceCall` on `<service>/calls` as
//! each of its calls ends (`docs/design/native-services.md` §4).
//!
//! A record is encoded only while the topic has a subscriber, which costs one read of the
//! service's subscriber count per call. A record never holds a call up: a publisher whose
//! samples are all taken drops it, counts it, and the readers see the gap in the frame
//! sequence. A client whose log cannot be opened still calls; the reason is logged.

use hypha_core::port::{PayloadCapacity, QosPolicy, QosRequest, SERVICE_OBSERVER_CLIENTS};
use hypha_msgs::hypha_msgs::ServiceCall;
use hypha_wire::ros_time::RosTime;

use crate::node::HyphaNode;
use crate::publisher::{HyphaPublisher, PublisherSettings, open_publisher, send_frame_with};
use crate::qos::Declaration;

use super::outcome::CallResult;

/// The topic of a service's call log: `<service>/calls`.
#[must_use]
pub fn call_log_topic(service: &str) -> String {
    format!("{service}/calls")
}

/// The resolved definition of `hypha_msgs/ServiceCall`.
pub const SERVICE_CALL_DEFINITION: &str = "uint8 ANSWERED=0\nuint8 REFUSED=1\nuint8 NO_SERVER=2\n\
uint8 NOT_SENT=3\nuint8 SERVER_LOST=4\nuint8 TIMEOUT=5\nstring service\nstring type\nstring md5\n\
string client_node\nstring client_port\nuint64 sequence\nuint8 outcome\nstring reason\ntime sent\n\
time done\nuint8[] request\nuint8[] response\nuint64 request_len\nuint64 response_len\nbool truncated";

/// The name of the policy every call log topic is created with. Its publishers are the
/// clients a service admits, tools included.
pub const CALL_LOG_POLICY: &str = "serviceCalls";

/// The deepest queue a reader of a call log may ask for. Each client's publisher holds
/// every reader's queue in shared memory, records of up to [`record_bound`] each: under
/// `defaultService` (1 declared reader and 3 tools) 26 records of 10 KiB, 260 KiB.
const CALL_LOG_DEPTH: usize = 4;

/// The bytes of the request, and of the response, that a record carries: the first
/// [`MAX_LOGGED_BYTES`] of each, whatever the service policy allows. A record never costs
/// a payload bound, which may be megabytes; the lengths and the `truncated` flag of the
/// record say what was cut.
pub const MAX_LOGGED_BYTES: usize = 16 * 1024;

/// The longest reason a record carries; a longer one is cut at a character boundary.
const MAX_REASON_BYTES: usize = 1024;

/// What a record holds besides the request, the response and the reason: the strings of
/// the identity (each at most a name or a type long) and the fixed fields.
const IDENTITY_BYTES: usize = 1024;

/// The most bytes one record of a client of `policy` takes: what the identity and the
/// reason take, and each half of the call up to its bound or [`MAX_LOGGED_BYTES`]. A
/// client's log holds a pool of these in shared memory.
#[must_use]
pub fn record_bound(policy: &hypha_core::port::ServicePolicy) -> usize {
    IDENTITY_BYTES
        .saturating_add(MAX_REASON_BYTES)
        .saturating_add(policy.max_request_bytes.min(MAX_LOGGED_BYTES))
        .saturating_add(policy.max_response_bytes.min(MAX_LOGGED_BYTES))
}

/// The policy the log topic of a service of `policy` is created with: the service's
/// clients and tools publish, one declared reader.
fn log_qos(policy: &hypha_core::port::ServicePolicy) -> QosPolicy {
    let publishers = policy
        .max_clients
        .saturating_add(SERVICE_OBSERVER_CLIENTS)
        .min(hypha_core::port::MAX_POLICY_PUBLISHERS);
    QosPolicy::new(CALL_LOG_POLICY, publishers, 1, CALL_LOG_DEPTH)
}

/// The shared memory the call log of one client of `service` under `policy` holds, in
/// bytes: its pool of samples, each [`record_bound`] long, whether or not anyone reads
/// the log.
#[must_use]
pub fn log_pool_bytes(service: &str, policy: &hypha_core::port::ServicePolicy) -> usize {
    let qos = log_qos(policy);
    let readers = hypha_iceoryx2::qos::subscriber_capacity(&call_log_topic(service), &qos);
    hypha_iceoryx2::qos::pool_samples(&qos, readers, 1).saturating_mul(record_bound(policy))
}

/// One client's call log.
pub(crate) struct CallLog {
    publisher: HyphaPublisher,
    service: String,
    type_name: String,
    md5: String,
    client_node: String,
    client_port: String,
    /// The bound of one record.
    max_payload: usize,
    /// Records dropped: no free sample.
    dropped: u64,
}

/// One finished call, as the log records it.
pub(crate) struct Entry<'a> {
    pub(crate) sequence: u64,
    pub(crate) request: &'a [u8],
    pub(crate) result: &'a CallResult,
}

impl CallLog {
    /// Open the log of a client of `spec` labelled `client_port`, or `None` with the reason
    /// logged.
    pub(crate) fn open(
        node: &HyphaNode,
        spec: &hypha_core::port::ServiceSpec,
        client_port: &str,
    ) -> Option<Self> {
        let topic = call_log_topic(&spec.name);
        let policy = &spec.policy;
        let max_payload = record_bound(policy);
        let settings = PublisherSettings {
            latch: false,
            loaned_samples: 1,
            declaration: Declaration {
                request: QosRequest::Declared(log_qos(policy)),
                payload: PayloadCapacity::Bounded(max_payload),
                initial_payload: 0,
            },
        };
        let opened = open_publisher(
            node,
            &topic,
            ServiceCall::TYPE_NAME,
            ServiceCall::SCHEMA_HASH,
            SERVICE_CALL_DEFINITION,
            settings,
        );
        match opened {
            Ok(publisher) => Some(Self {
                publisher,
                service: spec.name.clone(),
                type_name: spec.type_name.clone(),
                md5: spec.md5.clone(),
                client_node: node.name.clone(),
                client_port: client_port.to_owned(),
                max_payload,
                dropped: 0,
            }),
            Err(error) => {
                log::warn!(target: "hypha", "hypha: service '{}': the calls of this client are not logged on {topic}: {error:?}", spec.name);
                None
            }
        }
    }

    /// Record `entry` when someone reads the log.
    pub(crate) fn record(&mut self, entry: &Entry<'_>) {
        if self.publisher.port.subscriber_count() == Some(0) {
            return;
        }
        let result = entry.result;
        let response = result.outcome.response().unwrap_or_default();
        let record = ServiceCall {
            service: self.service.clone(),
            r#type: self.type_name.clone(),
            md5: self.md5.clone(),
            client_node: self.client_node.clone(),
            client_port: self.client_port.clone(),
            sequence: entry.sequence,
            outcome: u8::try_from(result.outcome.code()).unwrap_or(u8::MAX),
            reason: bounded_reason(&result.outcome.to_string()),
            sent: ros_time(result.sent_nanos),
            done: ros_time(result.done_nanos),
            request: logged(entry.request).to_vec(),
            response: logged(response).to_vec(),
            request_len: u64::try_from(entry.request.len()).unwrap_or(u64::MAX),
            response_len: u64::try_from(response.len()).unwrap_or(u64::MAX),
            truncated: entry.request.len() > MAX_LOGGED_BYTES || response.len() > MAX_LOGGED_BYTES,
        };
        let len = record.encoded_len();
        let sent = if len > self.max_payload {
            Err(crate::status::HyphaStatus::TooLarge)
        } else {
            send_frame_with(&self.publisher, len, |payload| {
                hypha_core::message::EncodeInto::encode_into(&record, payload)
                    .map(drop)
                    .map_err(|_| crate::status::HyphaStatus::InvalidFrame)
            })
        };
        if let Err(status) = sent {
            self.dropped = self.dropped.saturating_add(1);
            if self.dropped == 1 || self.dropped.is_power_of_two() {
                log::warn!(target: "hypha", "hypha: service '{}': the call log dropped a record ({status:?}); {} dropped so far", self.service, self.dropped);
            }
        }
    }

    /// Records dropped so far.
    pub(crate) const fn dropped(&self) -> u64 {
        self.dropped
    }
}

/// The first [`MAX_LOGGED_BYTES`] of `half`.
fn logged(half: &[u8]) -> &[u8] {
    half.get(..MAX_LOGGED_BYTES).unwrap_or(half)
}

/// `text` cut to [`MAX_REASON_BYTES`] at a character boundary.
fn bounded_reason(text: &str) -> String {
    if text.len() <= MAX_REASON_BYTES {
        return text.to_owned();
    }
    let mut end = MAX_REASON_BYTES;
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    text.get(..end).unwrap_or_default().to_owned()
}

/// A framework time as ROS `time`; a time before the epoch is 0.
fn ros_time(nanos: i64) -> RosTime {
    let nanos = u64::try_from(nanos).unwrap_or(0);
    let secs = u32::try_from(nanos / 1_000_000_000).unwrap_or(u32::MAX);
    let nsecs = u32::try_from(nanos % 1_000_000_000).unwrap_or(0);
    RosTime { secs, nsecs }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_LOGGED_BYTES, SERVICE_CALL_DEFINITION, ServiceCall, bounded_reason, log_pool_bytes,
        logged, record_bound,
    };
    use hypha_core::port::ServicePolicy;

    #[test]
    fn the_definition_is_the_one_of_the_generated_type() {
        let md5 =
            hypha_msgdef::md5::md5_from_definition(ServiceCall::TYPE_NAME, SERVICE_CALL_DEFINITION);
        assert_eq!(md5.as_deref(), Some(ServiceCall::SCHEMA_HASH));
    }

    #[test]
    fn a_long_reason_is_cut_at_a_character_boundary() {
        let long = "é".repeat(600);
        let cut = bounded_reason(&long);
        assert!(cut.len() <= 1024);
        assert!(long.starts_with(&cut));
    }

    #[test]
    fn a_record_costs_no_payload_bound() {
        // 26 samples of 10 KiB under defaultService, as the pool sizes them.
        let default = ServicePolicy::default_service();
        assert_eq!(record_bound(&default), 10 * 1024);
        assert_eq!(log_pool_bytes("/svc", &default), 26 * 10 * 1024);

        let large = ServicePolicy {
            max_request_bytes: 4 << 20,
            max_response_bytes: 4 << 20,
            ..ServicePolicy::default_service()
        };
        assert_eq!(record_bound(&large), 2048 + 2 * MAX_LOGGED_BYTES);
        assert!(log_pool_bytes("/svc", &large) < (1 << 20), "{} B", log_pool_bytes("/svc", &large));
    }

    #[test]
    fn a_half_is_cut_to_its_first_bytes() {
        let long = vec![7_u8; MAX_LOGGED_BYTES + 1];
        assert_eq!(logged(&long).len(), MAX_LOGGED_BYTES);
        assert_eq!(logged(&vec![7_u8; MAX_LOGGED_BYTES]).len(), MAX_LOGGED_BYTES);
        assert_eq!(logged(&[1, 2]), [1, 2]);
    }
}
