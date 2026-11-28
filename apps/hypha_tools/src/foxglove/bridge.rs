//! High-level Foxglove bridge: bus sample -> channel/schema tracking,
//! `/tf_static`-style latching, `/clock` sim-time handling, timeline-loop
//! detection, and per-topic rate limiting.

use super::server::{FoxgloveServer, ServerOptions, epoch_nanos_now};
use crate::{CLOCK_TOPIC, lock};
use hypha_node::sdk::SchemaIdentityCache;
use hypha_node::sdk::{BusNode, SampleIdentity};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Shortest wall time between two time frames sent to the clients.
const TIME_BROADCAST_INTERVAL_NS: u64 = 20_000_000;

/// ROS's own hardcoded convention: unlike `/tf`, more than one independent publisher
/// shares this one topic, each contributing only the pairs it owns.
const TF_STATIC_TOPIC: &str = "/tf_static";

/// Configuration for `FoxgloveBridge`.
#[derive(Default)]
pub struct BridgeConfig {
    /// Topic patterns to serve (empty = every topic).
    pub topics: Vec<String>,
    /// Rate limit per topic in Hz (0 = unlimited).
    pub max_rate_hz: f64,
    /// WebSocket server options.
    pub server_options: ServerOptions,
}

impl BridgeConfig {
    /// The patterns to subscribe to on the bus: every topic, or the served
    /// patterns and `/clock`, which drives the clients' time.
    #[must_use]
    pub fn subscription_patterns(&self) -> Vec<String> {
        if self.topics.is_empty() {
            return vec!["**".to_string()];
        }
        let mut patterns = self.topics.clone();
        patterns.push(CLOCK_TOPIC.to_string());
        patterns
    }
}

struct PendingSample {
    timestamp_ns: u64,
    payload: Arc<[u8]>,
}

#[derive(Default)]
struct BridgeState {
    /// The channel of each topic and the schema hash of the stream it carries: the first
    /// stream seen on the name, since a Foxglove channel is one topic of one schema.
    channel_by_topic: HashMap<String, (u32, u64)>,
    /// The one most recent sample seen on a topic whose schema has not yet
    /// resolved.
    pending_by_topic: HashMap<String, PendingSample>,
    last_sent_times: HashMap<String, u64>,
    unresolved_reported: HashSet<String>,
}

/// Bridges hypha bus samples to a `FoxgloveServer`.
pub struct FoxgloveBridge {
    config: BridgeConfig,
    server: FoxgloveServer,
    state: Mutex<BridgeState>,
    sim_clock_nanos: AtomicU64,
    /// Wall time of the last time frame sent, in nanoseconds since the epoch.
    last_time_broadcast_ns: AtomicU64,
    /// A frame carries only a schema hash, so the complete type identity comes
    /// from the retained schema service.
    identities: SchemaIdentityCache,
}

impl FoxgloveBridge {
    /// Binds the WebSocket server described by `config.server_options`.
    ///
    /// # Errors
    /// Returns an error if the WebSocket server binding fails.
    pub fn start(config: BridgeConfig) -> std::io::Result<Self> {
        let server = FoxgloveServer::bind(config.server_options.clone())?;
        Ok(Self {
            config,
            server,
            state: Mutex::new(BridgeState::default()),
            sim_clock_nanos: AtomicU64::new(0),
            last_time_broadcast_ns: AtomicU64::new(0),
            identities: SchemaIdentityCache::new(),
        })
    }

    /// Topic patterns to subscribe to on the bus.
    #[must_use]
    pub fn subscription_patterns(&self) -> Vec<String> {
        self.config.subscription_patterns()
    }

    /// TCP port the WebSocket server is actually listening on.
    #[must_use]
    pub fn local_port(&self) -> u16 {
        self.server.local_port()
    }

    /// Number of currently connected Foxglove clients.
    #[must_use]
    pub fn client_count(&self) -> usize {
        self.server.client_count()
    }

    /// Number of channels currently published.
    #[must_use]
    pub fn channel_count(&self) -> usize {
        self.server.channel_count()
    }

    fn is_topic_allowed(&self, topic: &str) -> bool {
        self.config.topics.is_empty()
            || self
                .config
                .topics
                .iter()
                .any(|pattern| hypha_node::sdk::topic_matches_pattern(pattern, topic))
    }

    /// Reports a schema failure once per topic.
    fn warn_unresolved(&self, topic: &str, cause: &str) {
        let mut state = lock(&self.state);
        let should_report = state.unresolved_reported.insert(topic.to_owned());
        drop(state);
        if should_report {
            eprintln!("hypha foxglove: cannot serve '{topic}': {cause}");
        }
    }

    /// Parses a `rosgraph_msgs/Clock` sample (two little-endian u32s: sec,
    /// nsec), detects a playback timeline loop, and rebroadcasts sim time.
    fn handle_clock_sample(&self, topic: &str, bytes: &[u8]) {
        if topic != CLOCK_TOPIC {
            return;
        }
        let Some(clock_ns) =
            hypha_node::sdk::RosTime::decode_clock(bytes).map(hypha_node::sdk::RosTime::as_nanos)
        else {
            return;
        };

        let prev_clock = self.sim_clock_nanos.load(Ordering::Relaxed);
        let timeline_looped =
            prev_clock != 0 && clock_ns.saturating_add(1_000_000_000) < prev_clock;
        self.sim_clock_nanos.store(clock_ns, Ordering::Relaxed);
        let now_ns = epoch_nanos_now();
        let last_ns = self.last_time_broadcast_ns.load(Ordering::Relaxed);
        let time_frame_due = timeline_looped
            || now_ns < last_ns
            || now_ns.saturating_sub(last_ns) >= TIME_BROADCAST_INTERVAL_NS;
        if time_frame_due {
            self.last_time_broadcast_ns.store(now_ns, Ordering::Relaxed);
            self.server.broadcast_time(clock_ns);
        }
        // After the time frame: Foxglove clears its transforms when time goes back, so the
        // latched /tf_static must reach it after the jump, not before.
        if timeline_looped {
            self.handle_timeline_reset();
        }
    }

    /// Clears the rate-limiter cache and replays every latched topic (chiefly
    /// `/tf_static`) so a new playback pass is not throttled by, or missing,
    /// state left over from the previous pass.
    fn handle_timeline_reset(&self) {
        println!(
            "Playback timeline loop detected on /clock -- replaying latched topics (/tf_static)"
        );
        let mut state = lock(&self.state);
        state.last_sent_times.clear();
        drop(state);
        self.server.replay_all_latched();
    }

    /// `true` if `topic` should be dropped to respect a configured
    /// `--max-rate` limit.
    fn should_rate_limit(&self, topic: &str) -> bool {
        let max_rate = self.config.max_rate_hz;
        if max_rate <= 0.0 {
            return false;
        }
        let now_ns = epoch_nanos_now();
        #[allow(clippy::cast_possible_truncation)]
        #[allow(clippy::cast_sign_loss)]
        let min_interval_ns = (1e9 / max_rate).floor() as u64;

        let mut state = lock(&self.state);
        let Some(last) = state.last_sent_times.get_mut(topic) else {
            state.last_sent_times.insert(topic.to_string(), now_ns);
            return false;
        };
        if now_ns >= *last && now_ns.saturating_sub(*last) < min_interval_ns {
            return true;
        }
        *last = now_ns;
        false
    }

    /// Returns the channel id for `topic`, creating and publishing a new
    /// channel if this is the first sample seen for it. Returns `None` when the
    /// topic's schema has not resolved yet: the sample is buffered instead, and
    /// replayed once the retained schema resolves.
    fn get_or_create_channel(
        &self,
        bus: &BusNode,
        topic: &str,
        schema_hash: u64,
        timestamp_ns: u64,
        payload: &Arc<[u8]>,
    ) -> Option<u32> {
        let known = self.lock_state().channel_by_topic.get(topic).copied();
        if let Some((id, stream)) = known {
            if stream == schema_hash {
                return Some(id);
            }
            self.warn_unresolved(
                topic,
                &format!(
                    "its channel carries the first stream seen on the name; the samples of the stream of schema {schema_hash:016x} are not served"
                ),
            );
            return None;
        }

        // The state is not locked here: a report takes that lock itself.
        let identity =
            match self.identities.resolve(bus, topic, schema_hash, std::time::Instant::now()) {
                Ok(identity) => identity,
                Err(cause) => {
                    self.warn_unresolved(topic, &format!("schema lookup failed: {cause}"));
                    None
                }
            };
        let Some(identity) = identity else {
            let pending = PendingSample { timestamp_ns, payload: Arc::clone(payload) };
            self.lock_state().pending_by_topic.insert(topic.to_owned(), pending);
            return None;
        };

        let new_id = self.server.add_channel(topic, &identity.data_type, &identity.definition);
        let mut state = self.lock_state();
        state.channel_by_topic.insert(topic.to_owned(), (new_id, schema_hash));
        if let Some(pending) = state.pending_by_topic.remove(topic) {
            drop(state);
            self.latch(topic, new_id, pending.timestamp_ns, pending.payload);
        }
        Some(new_id)
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, BridgeState> {
        lock(&self.state)
    }

    fn latch(&self, topic: &str, channel_id: u32, timestamp_ns: u64, payload: Arc<[u8]>) {
        if topic == TF_STATIC_TOPIC {
            self.server.latch_tf_static(channel_id, timestamp_ns, payload);
        } else {
            self.server.latch_replace(channel_id, timestamp_ns, payload);
        }
    }

    /// Processes one bus sample end to end. The payload is copied once, into the buffer that
    /// the latch and every client frame share.
    ///
    /// A frame without a valid source timestamp is refused rather than displayed under a
    /// substituted wall-clock time.
    pub fn on_bus_sample(&self, bus: &BusNode, sample: &SampleIdentity<'_>, bytes: &[u8]) {
        self.handle_clock_sample(sample.topic, bytes);

        if !self.is_topic_allowed(sample.topic) {
            return;
        }
        if self.should_rate_limit(sample.topic) {
            return;
        }
        if sample.transport_sent_nanos == 0 {
            self.warn_unresolved(sample.topic, "frame carries no valid source timestamp");
            return;
        }
        let timestamp_ns = sample.transport_sent_nanos;
        let payload: Arc<[u8]> = Arc::from(bytes);

        let Some(channel_id) = self.get_or_create_channel(
            bus,
            sample.topic,
            sample.schema_hash,
            timestamp_ns,
            &payload,
        ) else {
            return;
        };

        self.server.broadcast_sample(channel_id, timestamp_ns, &payload);
        self.latch(sample.topic, channel_id, timestamp_ns, payload);
    }

    /// Stops the WebSocket server and disconnects all clients.
    pub fn stop(&mut self) {
        self.server.stop();
    }
}
