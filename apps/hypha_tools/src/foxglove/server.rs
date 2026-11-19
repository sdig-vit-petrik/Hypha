//! Blocking multi-client WebSocket server speaking `foxglove.websocket.v1`.

use super::protocol::{
    Channel, FOXGLOVE_SUBPROTOCOL, create_binary_message, create_time_message,
    parse_client_message, serialize_advertise, serialize_server_info,
};
use crate::lock;
use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tungstenite::handshake::server::{Callback, ErrorResponse, Request, Response};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket, accept_hdr_with_config};

mod queue;
pub(super) use self::queue::epoch_nanos_now;
use self::queue::{ClientHandle, LatchedSample, QueuedFrame, try_enqueue};

/// The name the server reports in `serverInfo`.
const SERVER_NAME: &str = "hypha_foxglove_bridge";

/// Server-level configuration.
#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// Listen address.
    pub address: String,
    /// Listen port (0 lets the OS choose).
    pub port: u16,
    /// Longest wait of a live sample or time frame for its client. An older
    /// one is dropped.
    pub max_frame_age: Duration,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            address: "0.0.0.0".to_string(),
            port: 8765,
            max_frame_age: Duration::from_millis(500),
        }
    }
}

struct SubprotocolCallback;

impl Callback for SubprotocolCallback {
    fn on_request(
        self,
        request: &Request,
        mut response: Response,
    ) -> Result<Response, ErrorResponse> {
        if let Some(proto) = request.headers().get("Sec-WebSocket-Protocol")
            && let Ok(proto_str) = proto.to_str()
            && proto_str.contains(FOXGLOVE_SUBPROTOCOL)
        {
            response.headers_mut().append(
                "Sec-WebSocket-Protocol",
                tungstenite::http::HeaderValue::from_static(FOXGLOVE_SUBPROTOCOL),
            );
        }
        Ok(response)
    }
}

/// Thread-safe multi-client Foxglove WebSocket server.
pub struct FoxgloveServer {
    listener_thread: Option<JoinHandle<()>>,
    stop_signal: Arc<AtomicBool>,
    port: u16,
    options: ServerOptions,
    channels: Arc<Mutex<HashMap<u32, Channel>>>,
    clients: Arc<Mutex<HashMap<usize, ClientHandle>>>,
    next_channel_id: Arc<Mutex<u32>>,
    latched: Arc<Mutex<HashMap<u32, Vec<LatchedSample>>>>,
}

impl FoxgloveServer {
    /// Binds to `options.address:options.port` and starts listening for
    /// Foxglove Studio connections.
    ///
    /// # Errors
    /// Returns an error if the address binding fails.
    pub fn bind(options: ServerOptions) -> std::io::Result<Self> {
        let listener = TcpListener::bind((options.address.as_str(), options.port))?;
        listener.set_nonblocking(true)?;
        let local_port = listener.local_addr()?.port();

        let stop_signal = Arc::new(AtomicBool::new(false));
        let channels = Arc::new(Mutex::new(HashMap::new()));
        let clients: Arc<Mutex<HashMap<usize, ClientHandle>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let next_channel_id = Arc::new(Mutex::new(1u32));
        let latched: Arc<Mutex<HashMap<u32, Vec<LatchedSample>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let listener_thread = Self::spawn_listener_thread(
            listener,
            Arc::clone(&stop_signal),
            Arc::clone(&channels),
            Arc::clone(&clients),
            Arc::clone(&latched),
        );

        Ok(Self {
            listener_thread: Some(listener_thread),
            stop_signal,
            port: local_port,
            options,
            channels,
            clients,
            next_channel_id,
            latched,
        })
    }

    fn spawn_listener_thread(
        listener: TcpListener,
        stop_signal: Arc<AtomicBool>,
        channels: Arc<Mutex<HashMap<u32, Channel>>>,
        clients: Arc<Mutex<HashMap<usize, ClientHandle>>>,
        latched: Arc<Mutex<HashMap<u32, Vec<LatchedSample>>>>,
    ) -> JoinHandle<()> {
        thread::spawn(move || {
            let mut client_counter = 0usize;

            while !stop_signal.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, addr)) => {
                        client_counter = client_counter.saturating_add(1);
                        let client_id = client_counter;

                        let ws_config = WebSocketConfig::default();
                        let ws_res =
                            accept_hdr_with_config(stream, SubprotocolCallback, Some(ws_config));
                        if let Ok(mut ws) = ws_res {
                            let _ = ws.get_ref().set_nodelay(true);

                            // A client reconnecting after a bridge restart must not mistake
                            // the new process for a continuation of an earlier session.
                            let session_id = epoch_nanos_now().to_string();
                            let metadata = HashMap::new();
                            let server_info = serialize_server_info(
                                SERVER_NAME,
                                &["time"],
                                &["ros1"],
                                &session_id,
                                &metadata,
                            );
                            let _ = ws.send(Message::Text(server_info));

                            let current_channels: Vec<Channel> =
                                lock(&channels).values().cloned().collect();
                            if !current_channels.is_empty() {
                                let adv = serialize_advertise(&current_channels);
                                let _ = ws.send(Message::Text(adv));
                            }

                            let (tx, rx) = channel::<QueuedFrame>();
                            let buffered_bytes = Arc::new(AtomicUsize::new(0));
                            let dropped = Arc::new(AtomicUsize::new(0));
                            {
                                let mut cl_lock = lock(&clients);
                                cl_lock.insert(
                                    client_id,
                                    ClientHandle {
                                        tx,
                                        subscriptions: HashMap::new(),
                                        buffered_bytes: Arc::clone(&buffered_bytes),
                                        dropped: Arc::clone(&dropped),
                                    },
                                );
                            }

                            let cl_map_ref = Arc::clone(&clients);
                            let latched_ref = Arc::clone(&latched);
                            let cl_stop_ref = Arc::clone(&stop_signal);
                            let remote_addr = addr.to_string();
                            println!("[Client {client_id}] Connected from {remote_addr}");

                            thread::spawn(move || {
                                FoxgloveServer::run_client_worker(
                                    client_id,
                                    ws,
                                    &rx,
                                    &buffered_bytes,
                                    &dropped,
                                    &cl_map_ref,
                                    &latched_ref,
                                    &cl_stop_ref,
                                    &remote_addr,
                                );
                            });
                        }
                    }
                    Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => {
                        thread::sleep(Duration::from_millis(50));
                    }
                }
            }
        })
    }

    // Each handle has a distinct synchronization role and is shared with the listener thread.
    #[allow(clippy::too_many_arguments)]
    fn run_client_worker(
        client_id: usize,
        mut ws: WebSocket<TcpStream>,
        rx: &Receiver<QueuedFrame>,
        buffered_bytes: &Arc<AtomicUsize>,
        dropped: &Arc<AtomicUsize>,
        clients_map: &Arc<Mutex<HashMap<usize, ClientHandle>>>,
        latched_map: &Arc<Mutex<HashMap<u32, Vec<LatchedSample>>>>,
        stop_signal: &Arc<AtomicBool>,
        remote_addr: &str,
    ) {
        if let Ok(stream) = ws.get_ref().try_clone() {
            let _ = stream.set_nonblocking(true);
        }

        // The socket is non-blocking. A frame it cannot take yet stays in the
        // WebSocket's write buffer; until that buffer is flushed no further frame
        // is written, so later frames wait in the queue, where they go stale.
        let mut socket_full = false;
        'serve: while !stop_signal.load(Ordering::Relaxed) {
            loop {
                if socket_full {
                    match ws.flush() {
                        Ok(()) => socket_full = false,
                        Err(tungstenite::Error::Io(ref e)) if e.kind() == ErrorKind::WouldBlock => {
                            break;
                        }
                        Err(e) => {
                            println!("[Client {client_id}] Write error ({e}). Disconnecting.");
                            break 'serve;
                        }
                    }
                }
                match rx.try_recv() {
                    Ok(frame) => {
                        buffered_bytes.fetch_sub(
                            buffered_bytes.load(Ordering::Relaxed).min(frame.bytes.len()),
                            Ordering::Relaxed,
                        );
                        if frame.is_stale(Instant::now()) {
                            dropped.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        match ws.send(Message::Binary(frame.bytes)) {
                            Ok(()) => {}
                            Err(tungstenite::Error::Io(ref e))
                                if e.kind() == ErrorKind::WouldBlock =>
                            {
                                socket_full = true;
                            }
                            Err(e) => {
                                println!("[Client {client_id}] Write error ({e}). Disconnecting.");
                                break 'serve;
                            }
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        println!("[Client {client_id}] Internal channel closed. Disconnecting.");
                        break 'serve;
                    }
                }
            }

            match ws.read() {
                Ok(Message::Text(txt)) => {
                    Self::handle_client_text(client_id, &txt, clients_map, latched_map);
                }
                Ok(Message::Close(_)) => {
                    println!("[Client {client_id}] Disconnected cleanly ({remote_addr}).");
                    break 'serve;
                }
                Err(tungstenite::Error::Io(ref e)) if e.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(e) => {
                    println!("[Client {client_id}] Disconnected with error: {e}");
                    break 'serve;
                }
                _ => {}
            }
        }

        lock(clients_map).remove(&client_id);
    }

    /// Applies one inbound text frame's subscribe/unsubscribe requests, and
    /// replays each new subscription's latched sample(s) immediately.
    fn handle_client_text(
        client_id: usize,
        text: &str,
        clients_map: &Arc<Mutex<HashMap<usize, ClientHandle>>>,
        latched_map: &Arc<Mutex<HashMap<u32, Vec<LatchedSample>>>>,
    ) {
        let Some(client_msg) = parse_client_message(text) else {
            return;
        };

        let mut newly_subscribed: Vec<(u32, u32)> = Vec::new(); // (sub_id, channel_id)

        {
            let mut clients = lock(clients_map);
            if let Some(handle) = clients.get_mut(&client_id) {
                if let Some(subs) = client_msg.subscriptions {
                    for sub in subs {
                        println!(
                            "[Client {client_id}] Subscribed to channel {} (sub_id: {})",
                            sub.channel_id, sub.id
                        );
                        handle.subscriptions.insert(sub.id, sub.channel_id);
                        newly_subscribed.push((sub.id, sub.channel_id));
                    }
                }
                if let Some(unsubs) = client_msg.subscription_ids {
                    for sub_id in unsubs {
                        println!("[Client {client_id}] Unsubscribed from sub_id {sub_id}");
                        handle.subscriptions.remove(&sub_id);
                    }
                }
            }
        }

        if newly_subscribed.is_empty() {
            return;
        }

        let latched_lock = lock(latched_map);
        let clients_lock = lock(clients_map);
        let Some(handle) = clients_lock.get(&client_id) else {
            return;
        };
        replay_latched(handle, &latched_lock, newly_subscribed);
    }

    /// Local TCP port bound.
    #[must_use]
    pub fn local_port(&self) -> u16 {
        self.port
    }

    /// Number of currently connected clients.
    #[must_use]
    pub fn client_count(&self) -> usize {
        lock(&self.clients).len()
    }

    /// Number of channels currently published.
    #[must_use]
    pub fn channel_count(&self) -> usize {
        lock(&self.channels).len()
    }

    /// Publishes a new channel and notifies all connected clients.
    #[must_use]
    pub fn add_channel(&self, topic: &str, schema_name: &str, schema: &str) -> u32 {
        let mut id_lock = lock(&self.next_channel_id);
        let id = *id_lock;
        *id_lock = id.saturating_add(1);
        drop(id_lock);

        let ch = Channel {
            id,
            topic: topic.to_string(),
            encoding: "ros1".to_string(),
            schema_name: schema_name.to_string(),
            schema: schema.to_string(),
            schema_encoding: "ros1msg".to_string(),
        };

        {
            let mut ch_lock = lock(&self.channels);
            ch_lock.insert(id, ch.clone());
        }

        let adv_msg = serialize_advertise(&[ch]);
        let clients = lock(&self.clients);
        for handle in clients.values() {
            try_enqueue(handle, QueuedFrame::lasting(adv_msg.as_bytes().to_vec()));
        }

        println!("[Server] Published channel {id} for topic '{topic}' ({schema_name})");
        id
    }

    /// Broadcasts a binary sample to all clients subscribed to `channel_id`.
    pub fn broadcast_sample(&self, channel_id: u32, timestamp_ns: u64, payload: &[u8]) {
        let clients = lock(&self.clients);
        for handle in clients.values() {
            for (&sub_id, &sub_ch_id) in &handle.subscriptions {
                if sub_ch_id == channel_id {
                    let frame = create_binary_message(sub_id, timestamp_ns, payload);
                    try_enqueue(handle, QueuedFrame::live(frame, self.options.max_frame_age));
                }
            }
        }
    }

    /// Broadcasts the `TIME_DATA` opcode (0x02) to every connected client.
    pub fn broadcast_time(&self, timestamp_ns: u64) {
        let frame = create_time_message(timestamp_ns);
        let clients = lock(&self.clients);
        for handle in clients.values() {
            try_enqueue(handle, QueuedFrame::live(frame.clone(), self.options.max_frame_age));
        }
    }

    /// Caches `payload` as the sole latched sample for `channel_id`, replacing
    /// whatever was cached before.
    pub fn latch_replace(&self, channel_id: u32, timestamp_ns: u64, payload: Arc<[u8]>) {
        lock(&self.latched).insert(channel_id, vec![LatchedSample { timestamp_ns, payload }]);
    }

    /// Merges a `/tf_static` `payload` into `channel_id`'s latched set; see
    /// [`merge_tf_static`].
    pub fn latch_tf_static(&self, channel_id: u32, timestamp_ns: u64, payload: Arc<[u8]>) {
        let mut latched = lock(&self.latched);
        merge_tf_static(latched.entry(channel_id).or_default(), timestamp_ns, payload);
    }

    /// Replays every latched sample to every client currently subscribed to
    /// its channel. Called after a playback timeline loop is detected on `/clock`.
    pub fn replay_all_latched(&self) {
        let latched = lock(&self.latched);
        let clients = lock(&self.clients);
        for handle in clients.values() {
            let subscribed =
                handle.subscriptions.iter().map(|(&sub_id, &channel_id)| (sub_id, channel_id));
            replay_latched(handle, &latched, subscribed);
        }
    }

    /// Stops the server.
    pub fn stop(&mut self) {
        self.stop_signal.store(true, Ordering::Relaxed);
        if let Some(t) = self.listener_thread.take() {
            let _ = t.join();
        }
    }
}

/// Queues each latched sample of the `(sub_id, channel_id)` subscriptions on `handle`.
fn replay_latched(
    handle: &ClientHandle,
    latched: &HashMap<u32, Vec<LatchedSample>>,
    subscriptions: impl IntoIterator<Item = (u32, u32)>,
) {
    for (sub_id, channel_id) in subscriptions {
        let Some(items) = latched.get(&channel_id) else {
            continue;
        };
        for item in items {
            let frame = create_binary_message(sub_id, item.timestamp_ns, &item.payload);
            try_enqueue(handle, QueuedFrame::lasting(frame));
        }
    }
}

impl Drop for FoxgloveServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The (parent, child) frame pairs a `tf2_msgs/TFMessage` payload carries, or
/// `None` when it does not decode.
fn tf_frame_pairs(payload: &[u8]) -> Option<Vec<(String, String)>> {
    use hypha_node::sdk::message::DecodeOwned;
    let message = hypha_node::sdk::msgs::tf2_msgs::TFMessage::decode_from(payload).ok()?;
    let mut pairs: Vec<_> = message
        .transforms
        .into_iter()
        .map(|transform| (transform.header.frame_id, transform.child_frame_id))
        .collect();
    pairs.sort_unstable();
    Some(pairs)
}

/// Latches a `/tf_static` payload beside the others on the channel.
/// The set is keyed by frame pair: the new payload replaces every latched entry whose
/// pairs it covers. A payload that does not decode as a `TFMessage` is reported and
/// not latched.
fn merge_tf_static(list: &mut Vec<LatchedSample>, timestamp_ns: u64, payload: Arc<[u8]>) {
    let Some(new_pairs) = tf_frame_pairs(&payload) else {
        eprintln!("[Server] /tf_static payload is not a tf2_msgs/TFMessage; not latched");
        return;
    };
    list.retain(|item| {
        tf_frame_pairs(&item.payload)
            .is_some_and(|pairs| !pairs.iter().all(|pair| new_pairs.binary_search(pair).is_ok()))
    });
    list.push(LatchedSample { timestamp_ns, payload });
}

#[cfg(test)]
mod tests;
