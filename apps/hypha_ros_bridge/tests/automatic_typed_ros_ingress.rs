//! Automatic ROS 1 ingress must deliver samples that a typed hypha consumer accepts.

#![allow(clippy::expect_used)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use hypha_node::sdk::codec::CodecError;
use hypha_node::sdk::message::{DecodeOwned, MessageIdentity};
use hypha_node::sdk::{BusNode, RawSubscriber, SubscriberOptions};
use hypha_ros_bridge::{BridgeConfig, run};
use hypha_testkit::unique_name;
use serde_xmlrpc::Value;

const COUNTER_TYPE: &str = "bridge_test_msgs/Counter";
const COUNTER_V1_MD5: &str = "d9d213e8c146065ec958333c617eb1a6";
const COUNTER_V1_DEFINITION: &str = "uint32 value\n";
const COUNTER_V2_MD5: &str = "a2c9fb44e48f75feda2746b01055cfa1";
const COUNTER_V2_DEFINITION: &str = "uint64 value\n";
const TALKER_NODE: &str = "/fake_talker";
const SCHEMA_READER_TOPIC: &str = "HYPHA_BRIDGE_SCHEMA_READER_TOPIC";

const ACCEPT_POLL: Duration = Duration::from_millis(5);
const SAMPLE_PERIOD: Duration = Duration::from_millis(20);
const RECEIVE_POLL: Duration = Duration::from_millis(10);
const SOCKET_TIMEOUT: Duration = Duration::from_secs(2);
const DELIVERY_LIMIT: Duration = Duration::from_secs(20);
const DRAIN_PERIOD: Duration = Duration::from_secs(1);
const RESOLVE_LIMIT: Duration = Duration::from_secs(5);
const RESOLVE_RETRY: Duration = Duration::from_millis(50);

struct CounterV1 {
    value: u32,
}

impl MessageIdentity for CounterV1 {
    const TYPE_NAME: &'static str = COUNTER_TYPE;
    const SCHEMA_HASH: &'static str = COUNTER_V1_MD5;
}

impl DecodeOwned for CounterV1 {
    fn decode_from(bytes: &[u8]) -> Result<Self, CodecError> {
        let array: [u8; 4] = bytes
            .try_into()
            .map_err(|_| CodecError::Truncated { needed: 4, available: bytes.len() })?;
        Ok(Self { value: u32::from_le_bytes(array) })
    }
}

struct CounterV2 {
    value: u64,
}

impl MessageIdentity for CounterV2 {
    const TYPE_NAME: &'static str = COUNTER_TYPE;
    const SCHEMA_HASH: &'static str = COUNTER_V2_MD5;
}

impl DecodeOwned for CounterV2 {
    fn decode_from(bytes: &[u8]) -> Result<Self, CodecError> {
        let array: [u8; 8] = bytes
            .try_into()
            .map_err(|_| CodecError::Truncated { needed: 8, available: bytes.len() })?;
        Ok(Self { value: u64::from_le_bytes(array) })
    }
}

/// The schema one fake publisher announces and the body width it sends.
#[derive(Clone)]
struct FakeSchema {
    topic: String,
    md5: &'static str,
    definition: &'static str,
    body_len: usize,
}

/// Answers one XML-RPC method call with a ROS status code and value.
trait XmlRpcHandler: Send + Sync + 'static {
    fn answer(&self, method: &str, args: &[Value]) -> (i32, Value);
}

struct MasterState {
    publisher_api: String,
    subscriber_apis: Vec<String>,
}

struct MasterHandler {
    uri: String,
    topic: String,
    state: Arc<Mutex<MasterState>>,
}

impl XmlRpcHandler for MasterHandler {
    fn answer(&self, method: &str, args: &[Value]) -> (i32, Value) {
        let mut state = self.state.lock().expect("master state");
        let topic = Value::String(self.topic.clone());
        match method {
            "getUri" => (1, Value::String(self.uri.clone())),
            "registerSubscriber" => {
                let Some(Value::String(caller_api)) = args.get(3) else {
                    return (-1, Value::Int(0));
                };
                if !state.subscriber_apis.contains(caller_api) {
                    state.subscriber_apis.push(caller_api.clone());
                }
                let wanted_topic =
                    matches!(args.get(1), Some(Value::String(name)) if *name == self.topic);
                if !wanted_topic {
                    return (1, Value::Array(Vec::new()));
                }
                (1, Value::Array(vec![Value::String(state.publisher_api.clone())]))
            }
            "getSystemState" => {
                let publishers = Value::Array(vec![Value::Array(vec![
                    topic,
                    Value::Array(vec![Value::String(TALKER_NODE.to_owned())]),
                ])]);
                (
                    1,
                    Value::Array(vec![
                        publishers,
                        Value::Array(Vec::new()),
                        Value::Array(Vec::new()),
                    ]),
                )
            }
            "getTopicTypes" | "getPublishedTopics" => (
                1,
                Value::Array(vec![Value::Array(vec![
                    topic,
                    Value::String(COUNTER_TYPE.to_owned()),
                ])]),
            ),
            "lookupNode" => {
                let talker =
                    matches!(args.get(1), Some(Value::String(node)) if node == TALKER_NODE);
                if !talker {
                    return (-1, Value::String(String::new()));
                }
                (1, Value::String(state.publisher_api.clone()))
            }
            "registerPublisher" => (1, Value::Array(Vec::new())),
            _ => (1, Value::Int(1)),
        }
    }
}

struct PublisherHandler {
    tcpros_port: u16,
}

impl XmlRpcHandler for PublisherHandler {
    fn answer(&self, method: &str, _args: &[Value]) -> (i32, Value) {
        if method != "requestTopic" {
            return (1, Value::Int(1));
        }
        (
            1,
            Value::Array(vec![
                Value::String("TCPROS".to_owned()),
                Value::String("127.0.0.1".to_owned()),
                Value::Int(i32::from(self.tcpros_port)),
            ]),
        )
    }
}

struct FakeMaster {
    uri: String,
    topic: String,
    state: Arc<Mutex<MasterState>>,
}

impl FakeMaster {
    fn start(topic: &str, publisher_api: &str) -> Self {
        let (listener, uri) = bind_local();
        let state = Arc::new(Mutex::new(MasterState {
            publisher_api: publisher_api.to_owned(),
            subscriber_apis: Vec::new(),
        }));
        let handler =
            MasterHandler { uri: uri.clone(), topic: topic.to_owned(), state: Arc::clone(&state) };
        serve_xmlrpc(listener, handler, Arc::new(AtomicBool::new(true)));
        Self { uri, topic: topic.to_owned(), state }
    }

    /// Registers a restarted talker the way a ROS master does: the node name
    /// stays, its URI changes, and every subscriber gets `publisherUpdate`.
    fn replace_publisher(&self, publisher_api: &str) {
        let subscriber_apis = {
            let mut state = self.state.lock().expect("master state");
            publisher_api.clone_into(&mut state.publisher_api);
            state.subscriber_apis.clone()
        };
        let body = serde_xmlrpc::request_to_string(
            "publisherUpdate",
            vec![
                Value::String("/master".to_owned()),
                Value::String(self.topic.clone()),
                Value::Array(vec![Value::String(publisher_api.to_owned())]),
            ],
        )
        .expect("publisherUpdate request");
        for subscriber_api in subscriber_apis {
            post_xmlrpc(&subscriber_api, &body);
        }
    }
}

struct FakePublisher {
    api_uri: String,
    alive: Arc<AtomicBool>,
}

impl FakePublisher {
    fn start(schema: FakeSchema) -> Self {
        let alive = Arc::new(AtomicBool::new(true));
        let tcpros = TcpListener::bind("127.0.0.1:0").expect("tcpros listener");
        let tcpros_port = tcpros.local_addr().expect("tcpros address").port();
        tcpros.set_nonblocking(true).expect("nonblocking tcpros listener");
        let tcpros_alive = Arc::clone(&alive);
        thread::spawn(move || accept_tcpros(tcpros, schema, tcpros_alive));
        let (listener, api_uri) = bind_local();
        serve_xmlrpc(listener, PublisherHandler { tcpros_port }, Arc::clone(&alive));
        Self { api_uri, alive }
    }

    fn stop(&self) {
        self.alive.store(false, Ordering::Release);
    }
}

fn bind_local() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("xmlrpc listener");
    let port = listener.local_addr().expect("xmlrpc address").port();
    listener.set_nonblocking(true).expect("nonblocking xmlrpc listener");
    (listener, format!("http://127.0.0.1:{port}/"))
}

fn serve_xmlrpc<H: XmlRpcHandler>(listener: TcpListener, handler: H, alive: Arc<AtomicBool>) {
    thread::spawn(move || {
        while alive.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => answer_http(&handler, stream),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(ACCEPT_POLL);
                }
                Err(_) => return,
            }
        }
    });
}

fn answer_http<H: XmlRpcHandler>(handler: &H, mut stream: TcpStream) {
    if stream.set_nonblocking(false).is_err()
        || stream.set_read_timeout(Some(SOCKET_TIMEOUT)).is_err()
    {
        return;
    }
    let Some(body) = read_http_body(&mut stream) else {
        return;
    };
    let Ok((method, args)) = serde_xmlrpc::request_from_str(&body) else {
        return;
    };
    let (code, value) = handler.answer(&method, &args);
    let reply = Value::Array(vec![Value::Int(code), Value::String(String::new()), value]);
    let Ok(xml) = serde_xmlrpc::response_to_string(std::iter::once(reply)) else {
        return;
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{xml}",
        xml.len()
    );
    // A caller that hung up early has nothing left to tell.
    let _ = stream.write_all(response.as_bytes());
}

fn read_http_body(stream: &mut TcpStream) -> Option<String> {
    let mut received = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        if let Some(header_end) = find(&received, b"\r\n\r\n") {
            let body_start = header_end.checked_add(4)?;
            let head = String::from_utf8_lossy(received.get(..header_end)?).to_ascii_lowercase();
            let length: usize = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))?
                .trim()
                .parse()
                .ok()?;
            let body_end = body_start.checked_add(length)?;
            if received.len() >= body_end {
                return String::from_utf8(received.get(body_start..body_end)?.to_vec()).ok();
            }
        }
        let count = stream.read(&mut chunk).ok()?;
        if count == 0 {
            return None;
        }
        received.extend_from_slice(chunk.get(..count)?);
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn post_xmlrpc(uri: &str, body: &str) {
    let without_scheme = uri.trim_start_matches("http://");
    let address = without_scheme.split('/').next().expect("subscriber address");
    let mut stream = TcpStream::connect(address).expect("subscriber API connection");
    stream.set_read_timeout(Some(SOCKET_TIMEOUT)).expect("subscriber API timeout");
    let request = format!(
        "POST / HTTP/1.1\r\nHost: {address}\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).expect("publisherUpdate write");
    let mut response = Vec::new();
    // The answer is not needed; reading it only waits for the update to land.
    let _ = stream.read_to_end(&mut response);
}

fn encode_header(fields: &[(&str, &str)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (key, value) in fields {
        let field = format!("{key}={value}");
        let field_len = u32::try_from(field.len()).expect("header field length");
        body.extend_from_slice(&field_len.to_le_bytes());
        body.extend_from_slice(field.as_bytes());
    }
    let body_len = u32::try_from(body.len()).expect("header length");
    let mut header = body_len.to_le_bytes().to_vec();
    header.extend_from_slice(&body);
    header
}

fn read_tcpros_header(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length).ok()?;
    let length = usize::try_from(u32::from_le_bytes(length)).ok()?;
    let mut header = vec![0_u8; length];
    stream.read_exact(&mut header).ok()?;
    Some(header)
}

fn accept_tcpros(listener: TcpListener, schema: FakeSchema, alive: Arc<AtomicBool>) {
    while alive.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                let connection_schema = schema.clone();
                let connection_alive = Arc::clone(&alive);
                thread::spawn(move || stream_samples(stream, connection_schema, connection_alive));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL);
            }
            Err(_) => return,
        }
    }
}

fn stream_samples(mut stream: TcpStream, schema: FakeSchema, alive: Arc<AtomicBool>) {
    if stream.set_nonblocking(false).is_err()
        || stream.set_read_timeout(Some(SOCKET_TIMEOUT)).is_err()
    {
        return;
    }
    if read_tcpros_header(&mut stream).is_none() {
        return;
    }
    let header = encode_header(&[
        ("callerid", TALKER_NODE),
        ("latching", "0"),
        ("md5sum", schema.md5),
        ("message_definition", schema.definition),
        ("topic", &schema.topic),
        ("type", COUNTER_TYPE),
    ]);
    if stream.write_all(&header).is_err() {
        return;
    }
    let mut counter: u64 = 1;
    while alive.load(Ordering::Acquire) {
        let counter_bytes = counter.to_le_bytes();
        let Some(body) = counter_bytes.get(..schema.body_len) else {
            return;
        };
        let Ok(body_len) = u32::try_from(body.len()) else {
            return;
        };
        let mut packet = body_len.to_le_bytes().to_vec();
        packet.extend_from_slice(body);
        if stream.write_all(&packet).is_err() {
            return;
        }
        counter = counter.wrapping_add(1);
        thread::sleep(SAMPLE_PERIOD);
    }
}

/// A consumer that accepts only frames of `M`'s schema and decodes them as `M`.
fn typed_consumer<M: MessageIdentity>(node: &BusNode, topic: &str) -> RawSubscriber {
    node.subscribe_raw_with_schema(
        topic,
        M::TYPE_NAME,
        M::SCHEMA_HASH,
        SubscriberOptions::default().keeping(8),
    )
    .expect("typed consumer")
}

fn receive_within<M>(subscriber: &mut RawSubscriber, limit: Duration) -> Option<M>
where
    M: DecodeOwned + MessageIdentity,
{
    let deadline = Instant::now().checked_add(limit).expect("deadline");
    while Instant::now() < deadline {
        if let Ok(Some(sample)) = subscriber.try_receive() {
            let bytes = sample.bytes().expect("payload");
            return Some(M::decode_from(bytes).expect("a frame of the subscribed schema decodes"));
        }
        thread::sleep(RECEIVE_POLL);
    }
    None
}

/// The frames the library refused because their schema hash was another one.
fn schema_mismatches(subscriber: &RawSubscriber) -> u64 {
    subscriber
        .metrics()
        .expect("subscriber metrics")
        .into_iter()
        .find(|(name, _)| name == "integrity.schema_mismatch")
        .map(|(_, value)| value)
        .expect("the schema mismatch metric")
}

/// The body of the schema reader process. Without a topic it does nothing.
#[test]
fn bridged_schema_reader_child() {
    let Ok(topic) = std::env::var(SCHEMA_READER_TOPIC) else {
        return;
    };
    let reader = BusNode::new(&unique_name("bridged_schema_reader")).expect("reader node");
    let schema_hash = hypha_node::sdk::schema_hash_from_md5(COUNTER_V1_MD5).expect("a valid MD5");
    let deadline = Instant::now().checked_add(RESOLVE_LIMIT).expect("deadline");
    let mut resolved = None;
    while resolved.is_none() && Instant::now() < deadline {
        resolved = reader.resolve_schema_hash(&topic, schema_hash).ok().flatten();
        thread::sleep(RESOLVE_RETRY);
    }
    let schema = resolved.expect("the bridge must answer a schema query from another process");
    assert_eq!(schema.data_type, COUNTER_TYPE);
    assert_eq!(schema.md5, COUNTER_V1_MD5);
    assert_eq!(schema.definition, COUNTER_V1_DEFINITION);
}

fn bridged_schema_resolves_in_another_process(topic: &str) -> bool {
    let status = Command::new(std::env::current_exe().expect("test binary"))
        .args(["bridged_schema_reader_child", "--exact", "--nocapture", "--test-threads=1"])
        .env(SCHEMA_READER_TOPIC, topic)
        .status()
        .expect("schema reader process");
    status.success()
}

#[test]
fn a_typed_consumer_accepts_automatic_ros_ingress_and_follows_a_schema_change() {
    // A domain of its own keeps the topics of a live system on this machine
    // out of the bridge under test. The schema reader process inherits it.
    hypha_testkit::set_env("HYPHA_DOMAIN", format!("t_{}_bridge", std::process::id()));
    // The bridge's ROS node must advertise an IPv4 address. A shell may export
    // a name such as `localhost` here, which the ROS client refuses at start.
    hypha_testkit::set_env("ROS_IP", "127.0.0.1");
    let topic = format!("/{}", unique_name("typed_ros_ingress"));
    let first_talker = FakePublisher::start(FakeSchema {
        topic: topic.clone(),
        md5: COUNTER_V1_MD5,
        definition: COUNTER_V1_DEFINITION,
        body_len: 4,
    });
    let master = FakeMaster::start(&topic, &first_talker.api_uri);

    let consumer = BusNode::new(&unique_name("typed_ingress_consumer")).expect("consumer node");
    // A consumer declares no policy, so it cannot open the topic; this test opens the
    // topic first.
    let _opener = consumer
        .publish_raw(&topic, CounterV1::TYPE_NAME, CounterV1::SCHEMA_HASH, "")
        .expect("the topic's opener");
    let mut counter_v1 = typed_consumer::<CounterV1>(&consumer, &topic);

    let config = BridgeConfig {
        manifest_path: None,
        master_uri: master.uri.clone(),
        node_name: unique_name("typed_ingress_bridge"),
    };
    let stop = Arc::new(AtomicBool::new(false));
    let bridge_stop = Arc::clone(&stop);
    let bridge = thread::spawn(move || run(&config, bridge_stop));

    let first = receive_within::<CounterV1>(&mut counter_v1, DELIVERY_LIMIT);
    let v1_mismatches = schema_mismatches(&counter_v1);
    assert!(
        first.is_some(),
        "the typed consumer accepted no bridged sample; schema mismatches: {v1_mismatches}"
    );
    let first = first.expect("first bridged sample");
    assert!(first.value > 0, "the bridged body must decode as the published counter");
    assert_eq!(v1_mismatches, 0, "bridged frames must carry the publisher's schema");
    assert!(
        bridged_schema_resolves_in_another_process(&topic),
        "a reader in another process must resolve the bridged schema"
    );

    // The talker restarts under the same node name with a changed definition, which is a
    // stream of its own: this test opens it too.
    let _opener_v2 = consumer
        .publish_raw(&topic, CounterV2::TYPE_NAME, CounterV2::SCHEMA_HASH, "")
        .expect("the opener of the second version");
    let mut counter_v2 = typed_consumer::<CounterV2>(&consumer, &topic);
    let second_talker = FakePublisher::start(FakeSchema {
        topic: topic.clone(),
        md5: COUNTER_V2_MD5,
        definition: COUNTER_V2_DEFINITION,
        body_len: 8,
    });
    first_talker.stop();
    master.replace_publisher(&second_talker.api_uri);

    let second = receive_within::<CounterV2>(&mut counter_v2, DELIVERY_LIMIT)
        .expect("the typed consumer of the new schema must receive the restarted talker");
    assert!(second.value > 0, "the new body must decode as the new counter");

    // The new schema is another stream: the old schema's consumer meets none of its
    // frames and counts nothing. Samples it queued before the restart are legitimate;
    // discard them first.
    while let Ok(Some(_)) = counter_v1.try_receive() {}
    let late_v1 = receive_within::<CounterV1>(&mut counter_v1, DRAIN_PERIOD);
    assert!(late_v1.is_none(), "a frame of the new schema must not reach the old consumer");
    assert_eq!(schema_mismatches(&counter_v1), 0, "a frame of another stream is no fault");

    stop.store(true, Ordering::Release);
    second_talker.stop();
    bridge.join().expect("bridge thread").expect("bridge result");
}
