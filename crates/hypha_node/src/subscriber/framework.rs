//! The framework-owned inputs of a node: `/clock` and `/clock_progress`. Each is an
//! ordinary subscriber whose every frame is validated against a fixed identity.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize};
use std::time::Duration;

use hypha_core::frame_validation::FrameValidator;
use hypha_core::port::TopicSpec;

use super::loan_budget::LoanBudget;
use super::{HyphaSubscriber, NoPublisherWatch};
use crate::status::Error;

/// Open the framework-owned `/clock` input.
///
/// Every frame is validated against the `rosgraph_msgs/Clock` identity.
pub(crate) fn create_sim_clock_subscriber(
    transport: &hypha_iceoryx2::Iox2Transport,
    introspection: &Arc<crate::introspection::IntrospectionRegistry>,
) -> Result<Box<HyphaSubscriber>, Error> {
    create_framework_subscriber(
        transport,
        introspection,
        "/clock",
        "rosgraph_msgs/Clock",
        crate::clock::CLOCK_MD5,
        1,
    )
}

/// Open the framework-owned `/clock_progress` input. Each publisher has its own queue,
/// so one node's reports never evict another's.
pub(crate) fn create_progress_subscriber(
    transport: &hypha_iceoryx2::Iox2Transport,
    introspection: &Arc<crate::introspection::IntrospectionRegistry>,
) -> Result<Box<HyphaSubscriber>, Error> {
    create_framework_subscriber(
        transport,
        introspection,
        crate::node_progress::PROGRESS_TOPIC,
        hypha_msgs::hypha_msgs::ReplayProgress::TYPE_NAME,
        hypha_msgs::hypha_msgs::ReplayProgress::SCHEMA_HASH,
        crate::node_progress::PROGRESS_INPUT_DEPTH,
    )
}

/// Open a framework input on `topic`, which keeps `depth` samples and validates every
/// frame against the identity `md5` of `data_type`.
fn create_framework_subscriber(
    transport: &hypha_iceoryx2::Iox2Transport,
    introspection: &Arc<crate::introspection::IntrospectionRegistry>,
    topic: &str,
    data_type: &str,
    md5: &str,
    depth: usize,
) -> Result<Box<HyphaSubscriber>, Error> {
    let schema_hash = hypha_wire::schema_hash_from_md5(md5).ok_or(Error::InvalidSchema)?;
    // A framework input declares no policy: it adopts the topic's service.
    let spec = TopicSpec::new(topic).keeping(depth).with_type(data_type).with_md5(md5);
    let port = transport.subscribe(&spec).map_err(Error::from)?;
    let budget =
        Arc::new(LoanBudget { budget: 1, held: AtomicUsize::new(0), refused: AtomicU64::new(0) });
    Ok(Box::new(HyphaSubscriber {
        owner_node: 0,
        topic: topic.to_owned(),
        data_type: data_type.to_owned(),
        md5: md5.to_owned(),
        port,
        budget,
        // The node drains it itself; nothing else may claim it.
        mode: AtomicU8::new(super::MODE_FRAMEWORK),
        attached: AtomicBool::new(false),
        validator: FrameValidator::new(Some(schema_hash)),
        no_publisher_watch: NoPublisherWatch::new(Duration::ZERO, std::time::Instant::now()),
        other_schemas: Arc::default(),
        rejection_report: super::RejectionReport::default(),
        introspection: crate::introspection::IntrospectionRow::new(
            introspection,
            crate::introspection::Row::Subscriber(hypha_core::introspection::EndpointRow {
                queue_depth: depth,
                md5: md5.to_owned(),
                ..hypha_core::introspection::EndpointRow::new(topic, data_type)
            }),
        ),
    }))
}
