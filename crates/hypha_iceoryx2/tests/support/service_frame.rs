//! Spike for #132: hypha's frame header as the iceoryx2 user header of a request and of
//! a response, shared by `tests/service_spike.rs` and the server helper process.
//!
//! A request is sealed for `<service>#request`. A response is sealed for
//! `<service>#response/<client>/<sequence>`: the client id and the sequence of the
//! request it answers are part of the CRC's start value, so a response verifies only
//! against the request it answers, and the header layout of a topic frame is kept.

#![allow(dead_code, reason = "the test and the helper each use part of this module")]

use hypha_wire::frame::{FrameHeader, FrameMetadata, ParseError, ParsedFrame, TopicCode};
use iceoryx2::node::Node;
use iceoryx2::prelude::*;

/// The thread-safe service variant, as the transport uses.
pub(crate) type Ipc = iceoryx2::service::ipc_threadsafe::Service;

/// The frame header in the layout iceoryx2 places in shared memory (as `Iox2FrameHeader`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, ZeroCopySend)]
#[type_name("hypha::ServiceFrameHeader")]
pub(crate) struct WireHeader {
    magic: u32,
    flags: u32,
    schema_hash: u64,
    publisher_id: u64,
    sequence: u64,
    transport_sent_nanos: u64,
    framework_sent_nanos: u64,
    crc: u32,
    reserved: u32,
}

impl From<FrameHeader> for WireHeader {
    fn from(header: FrameHeader) -> Self {
        let m = header.metadata;
        Self {
            magic: header.magic,
            flags: header.flags,
            schema_hash: m.schema_hash,
            publisher_id: m.publisher_id,
            sequence: m.sequence,
            transport_sent_nanos: m.transport_sent_nanos,
            framework_sent_nanos: m.framework_sent_nanos,
            crc: header.crc,
            reserved: 0,
        }
    }
}

impl From<WireHeader> for FrameHeader {
    fn from(header: WireHeader) -> Self {
        Self {
            magic: header.magic,
            flags: header.flags,
            metadata: FrameMetadata {
                schema_hash: header.schema_hash,
                publisher_id: header.publisher_id,
                sequence: header.sequence,
                transport_sent_nanos: header.transport_sent_nanos,
                framework_sent_nanos: header.framework_sent_nanos,
            },
            crc: header.crc,
        }
    }
}

/// The first eight bytes of the MD5 of `std_srvs/SetBool` (09fb03525b03e7ea1fd3992bafd87e16).
pub(crate) const SET_BOOL_HASH: u64 = 0x09fb_0352_5b03_e7ea;

/// Request and response payload bound of the spike's service.
pub(crate) const MAX_PAYLOAD: usize = 256;

/// The code a request of `service` is sealed for.
pub(crate) fn request_code(service: &str) -> TopicCode {
    TopicCode::of(&format!("{service}#request"))
}

/// The code the response to request `sequence` of client `client` is sealed for.
pub(crate) fn response_code(service: &str, client: u64, sequence: u64) -> TopicCode {
    TopicCode::of(&format!("{service}#response/{client:016x}/{sequence:016x}"))
}

/// Seal `payload` for `code`.
pub(crate) fn seal(code: TopicCode, sender: u64, sequence: u64, payload: &[u8]) -> WireHeader {
    let now = u64::try_from(hypha_time::steady_now().nanos()).unwrap_or(0);
    let metadata = FrameMetadata {
        schema_hash: SET_BOOL_HASH,
        publisher_id: sender,
        sequence,
        transport_sent_nanos: now,
        framework_sent_nanos: now,
    };
    hypha_wire::frame::seal(code, metadata, 0, payload).into()
}

/// Verify a received header and payload against `code`.
pub(crate) fn verify<'a>(
    code: TopicCode,
    header: &WireHeader,
    payload: &'a [u8],
) -> Result<ParsedFrame<'a>, ParseError> {
    hypha_wire::frame::verify(code, &FrameHeader::from(*header), payload)
}

fn service_name(text: &str) -> ServiceName {
    ServiceName::new(text).unwrap_or_else(|cause| panic!("service name '{text}': {cause:?}"))
}

/// The request-response service of `service`: one server, no fire-and-forget, no
/// safe overflow, one response per request.
pub(crate) fn open_service(
    node: &Node<Ipc>,
    service: &str,
) -> iceoryx2::service::port_factory::request_response::PortFactory<
    Ipc,
    [u8],
    WireHeader,
    [u8],
    WireHeader,
> {
    node.service_builder(&service_name(&format!("hypha_srv/{service}")))
        .request_response::<[u8], [u8]>()
        .request_user_header::<WireHeader>()
        .response_user_header::<WireHeader>()
        .max_servers(1)
        .max_clients(4)
        .max_active_requests_per_client(2)
        .max_response_buffer_size(1)
        .enable_fire_and_forget_requests(false)
        .enable_safe_overflow_for_requests(false)
        .enable_safe_overflow_for_responses(false)
        .open_or_create()
        .unwrap_or_else(|cause| panic!("open the service '{service}': {cause:?}"))
}

/// The wake-up event of one direction of `service` (`request` or `response`).
pub(crate) fn open_event(
    node: &Node<Ipc>,
    service: &str,
    direction: &str,
) -> iceoryx2::service::port_factory::event::PortFactory<Ipc> {
    node.service_builder(&service_name(&format!("hypha_srv/{service}/{direction}")))
        .event()
        .open_or_create()
        .unwrap_or_else(|cause| panic!("open the {direction} event of '{service}': {cause:?}"))
}

/// A node on the process's hypha domain.
pub(crate) fn node(name: &str) -> Node<Ipc> {
    let config = hypha_iceoryx2::domain::config().unwrap_or_else(|e| panic!("domain: {e:?}"));
    let node_name = NodeName::new(name).unwrap_or_else(|e| panic!("node name: {e:?}"));
    NodeBuilder::new()
        .config(config)
        .name(&node_name)
        .signal_handling_mode(SignalHandlingMode::Disabled)
        .create::<Ipc>()
        .unwrap_or_else(|cause| panic!("create the node '{name}': {cause:?}"))
}
