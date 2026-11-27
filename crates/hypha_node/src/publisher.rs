//! Sending: borrow a buffer, fill it, hand it back.

use std::sync::Arc;

use hypha_core::message::MessageIdentity;
use hypha_core::port::TopicSpec;
use hypha_msgs::hypha_msgs::{NodeIntrospection, NodeState};

use crate::node::HyphaNode;
use crate::qos::{Declaration, HyphaQos, read_declaration};
use crate::status::{Error, HyphaStatus};

pub(crate) type BackendPublisher = hypha_iceoryx2::Iox2Publisher;
pub(crate) type BackendLoan = hypha_iceoryx2::Iox2Loan;

/// A sending endpoint. Opaque to the caller.
pub struct HyphaPublisher {
    /// The address of the node that made this publisher, compared and never dereferenced.
    pub(crate) owner_node: usize,
    pub(crate) cycle_owned: std::sync::atomic::AtomicBool,
    pub(crate) port: BackendPublisher,
    pub(crate) max_payload_bytes: usize,
    /// The most loans this publisher may hold at once.
    pub(crate) loaned_samples: usize,
    pub(crate) schema_hash: u64,
    pub(crate) publisher_id: u64,
    /// The sequence number the last frame took. A publisher is still used from one thread
    /// at a time.
    pub(crate) next_sequence: Arc<std::sync::atomic::AtomicU64>,
    /// The publisher row of the node introspection, removed with this publisher.
    pub(crate) introspection: crate::introspection::IntrospectionRow,
    /// Set when the node that made this publisher left the bus: nothing is sent then.
    pub(crate) node_left: Arc<std::sync::atomic::AtomicBool>,
}

/// Endpoint options of a publisher, passed to `hypha_publish_with_qos`.
///
/// The payload bound is `qos.max_payload_dynamic`: zero is growable, and a fixed-size
/// type is bounded to its exact encoded size. A bound that does not fit `size_t` is
/// `HYPHA_STATUS_INVALID_ARGUMENT`; a larger loan or publish fails with
/// `HYPHA_STATUS_TOO_LARGE` and sends nothing. A growable publisher's first slot size is
/// `qos.initial_payload`, 4096 bytes when zero.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct HyphaPublisherOptions {
    /// Non-zero enables late-joiner replay.
    pub latch: i32,
    /// Samples this publisher may hold on loan at once. Zero selects the framework
    /// default, `PUBLISHER_LOANED_SAMPLES` (2). A value that does not fit is
    /// `HYPHA_STATUS_RESOURCE_LIMIT`.
    pub loaned_samples: u32,
    /// The service policy this publisher declares, and its payload bound.
    pub qos: HyphaQos,
}

/// What `open_publisher` opens a publisher with, once the options are read.
pub struct PublisherSettings {
    /// True when the publisher latches its samples.
    pub latch: bool,
    /// Samples this publisher may hold on loan at once. Zero selects the default.
    pub loaned_samples: u32,
    /// The service policy and payload bound of the publisher.
    pub declaration: Declaration,
}

impl PublisherSettings {
    /// The settings `options` gives; `qos_name` is the text of `options.qos.name`.
    ///
    /// # Errors
    /// `InvalidArgument` for a declaration in `options.qos` that `read_declaration` refuses.
    pub fn from_options(options: &HyphaPublisherOptions, qos_name: &str) -> Result<Self, Error> {
        let declaration = read_declaration(&options.qos, qos_name)?;
        Ok(Self { latch: options.latch != 0, loaned_samples: options.loaned_samples, declaration })
    }
}

/// The node's own report topics and the one type each carries.
const REPORT_TOPICS: [(&str, &str, &str); 2] = [
    (
        "state",
        <NodeState as MessageIdentity>::TYPE_NAME,
        <NodeState as MessageIdentity>::SCHEMA_HASH,
    ),
    (
        "introspection",
        <NodeIntrospection as MessageIdentity>::TYPE_NAME,
        <NodeIntrospection as MessageIdentity>::SCHEMA_HASH,
    ),
];

/// Refuses a publisher of another type than the report's on the node's own `~/state`
/// or `~/introspection`: a tool reads those topics by their type.
fn refuse_a_foreign_report_type(
    node: &HyphaNode,
    name: &str,
    data_type: &str,
    md5: &str,
) -> Result<(), Error> {
    if hypha_core::port::is_cli_node_name(&node.name) {
        return Ok(());
    }
    for (suffix, report_type, report_md5) in REPORT_TOPICS {
        let own = format!("~/{suffix}");
        let Ok(resolved_own) = node.resolve_name(&own) else {
            continue;
        };
        let foreign = name == resolved_own && !md5.eq_ignore_ascii_case(report_md5);
        if foreign {
            log::error!(target: "hypha", "hypha: '{name}' is refused: {own} carries {report_type}, not {data_type}");
            return Err(Error::InvalidTopic);
        }
    }
    Ok(())
}

/// Open a publisher of `node` on `topic`, which may name the node (`~/x`).
///
/// The publisher's `owner_node` is null: a framework publisher belongs to no application.
pub(crate) fn open_publisher(
    node: &HyphaNode,
    topic: &str,
    data_type: &str,
    md5: &str,
    definition: &str,
    settings: PublisherSettings,
) -> Result<HyphaPublisher, Error> {
    let resolved = node.resolve_name(topic)?;
    let name = resolved.as_str();
    refuse_a_foreign_report_type(node, name, data_type, md5)?;
    let loaned_samples = match settings.loaned_samples {
        0 => hypha_core::port::PUBLISHER_LOANED_SAMPLES,
        count => usize::try_from(count).map_err(|_| Error::InvalidArgument)?,
    };
    let Declaration { request, payload, initial_payload } = settings.declaration;
    let mut spec = TopicSpec::new(name)
        .with_type(data_type)
        .with_md5(md5)
        .with_definition(definition)
        .with_loaned_samples(loaned_samples)
        .with_payload_capacity(payload)
        .with_initial_payload(initial_payload)
        .with_qos(request);
    if settings.latch {
        spec = spec.latched();
    }
    if !spec.has_valid_public_name() || !spec.has_valid_schema_identity() {
        return Err(Error::InvalidTopic);
    }
    let schema_hash = hypha_wire::schema_hash_from_md5(md5).ok_or(Error::InvalidTopic)?;
    let port = node.transport.publish(&spec).map_err(Error::from)?;
    let max_payload_bytes = port.max_payload().unwrap_or(usize::MAX);
    Ok(HyphaPublisher {
        owner_node: 0,
        cycle_owned: std::sync::atomic::AtomicBool::new(false),
        port,
        max_payload_bytes,
        loaned_samples,
        schema_hash,
        publisher_id: hypha_wire::integrity::next_publisher_id(
            u64::try_from(hypha_time::steady_now().nanos()).unwrap_or(0),
            std::process::id(),
        ),
        next_sequence: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        node_left: Arc::clone(&node.left),
        introspection: crate::introspection::IntrospectionRow::new(
            &node.introspection,
            crate::introspection::Row::Publisher(hypha_core::introspection::EndpointRow {
                latched: settings.latch,
                md5: md5.to_owned(),
                ..hypha_core::introspection::EndpointRow::new(name, data_type)
            }),
        ),
    })
}

/// Publish `topic` of `node`, which may name the node (`~/x`), with the type identity
/// and `settings`; see `hypha_publish_with_qos`. The publisher belongs to `node`.
///
/// # Errors
/// `InvalidTopic` for a refused name or type identity, and the transport's error.
pub fn publish_with_qos(
    node: &HyphaNode,
    topic: &str,
    data_type: &str,
    md5: &str,
    definition: &str,
    settings: PublisherSettings,
) -> Result<HyphaPublisher, Error> {
    let mut publisher = open_publisher(node, topic, data_type, md5, definition, settings)?;
    publisher.owner_node = std::ptr::from_ref(node) as usize;
    Ok(publisher)
}

/// The smallest queue size among the subscribers connected to `publisher`, or 0 when
/// none is connected.
#[must_use]
pub fn publisher_min_subscriber_buffer(publisher: &HyphaPublisher) -> usize {
    publisher.port.min_subscriber_buffer().unwrap_or(0)
}

/// Encode directly into a stack-owned loan. Failed writes release it unsent. A pool
/// with no free buffer drops the sample.
pub(crate) fn send_frame_with(
    publisher: &HyphaPublisher,
    payload_len: usize,
    write: impl FnOnce(&mut [u8]) -> Result<(), HyphaStatus>,
) -> Result<(), HyphaStatus> {
    let mut loan = crate::loan::lend_frame_or_gap(publisher, payload_len)?;
    let payload = loan.payload_mut().ok_or(HyphaStatus::Backend)?;
    if payload.len() != payload_len {
        return Err(HyphaStatus::Backend);
    }
    write(payload)?;
    loan.send_frame()?;
    Ok(())
}

/// The number of subscribers `publisher` reaches now, or `None` when the transport
/// cannot tell.
#[must_use]
pub fn publisher_subscriber_count(publisher: &HyphaPublisher) -> Option<usize> {
    publisher.port.subscriber_count()
}
