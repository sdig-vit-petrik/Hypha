//! App state: topic/bus stats, sort/tab/filter state, and value formatting.

use hypha_tools::approx_f64;
use std::collections::BTreeMap;
use std::time::Instant;

/// Whether a frame with `schema_hash` carries a message of type `M`: the frame's hash is
/// the compact hash of `M`'s MD5. A zero hash names no schema, so it matches no type.
fn carries<M: hypha_node::sdk::message::MessageIdentity>(schema_hash: u64) -> bool {
    let known: bool = schema_hash != 0;
    let expected: Option<u64> = hypha_node::sdk::schema_hash_from_md5(M::SCHEMA_HASH);
    known && expected == Some(schema_hash)
}

/// Interval between rate/bandwidth recalculation ticks.
pub(crate) const RATE_TICK: std::time::Duration = std::time::Duration::from_millis(500);

/// Per-topic traffic counters and last-seen payload for the message inspector.
pub(crate) struct TopicStats {
    pub data_type: String,
    pub md5: String,
    pub schema_hash: u64,
    pub total_messages: u64,
    pub total_bytes: u64,
    pub window_messages: u64,
    pub window_bytes: u64,
    pub rate_hz: f64,
    pub bandwidth_kbs: f64,
    pub last_arrival: Instant,
    pub last_payload: Vec<u8>,
}

impl TopicStats {
    fn new(schema_hash: u64) -> Self {
        Self {
            data_type: String::new(),
            md5: String::new(),
            schema_hash,
            total_messages: 0,
            total_bytes: 0,
            window_messages: 0,
            window_bytes: 0,
            rate_hz: 0.0,
            bandwidth_kbs: 0.0,
            last_arrival: Instant::now(),
            last_payload: Vec::new(),
        }
    }

    fn record_sample(&mut self, schema_hash: u64, payload: &[u8]) {
        let schema_changed =
            self.schema_hash != 0 && schema_hash != 0 && self.schema_hash != schema_hash;
        if schema_changed {
            self.data_type.clear();
            self.md5.clear();
        }
        if schema_hash != 0 {
            self.schema_hash = schema_hash;
        }
        let byte_len = payload.len() as u64;
        self.total_messages = self.total_messages.saturating_add(1);
        self.total_bytes = self.total_bytes.saturating_add(byte_len);
        self.window_messages = self.window_messages.saturating_add(1);
        self.window_bytes = self.window_bytes.saturating_add(byte_len);
        self.last_arrival = Instant::now();
        self.last_payload.clear();
        self.last_payload.extend_from_slice(payload);
    }
}

/// Sort key for the topics table, cycled with `s`. It starts at `Name`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SortColumn {
    Bandwidth,
    Rate,
    Messages,
    Name,
}

impl SortColumn {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Bandwidth => Self::Rate,
            Self::Rate => Self::Messages,
            Self::Messages => Self::Name,
            Self::Name => Self::Bandwidth,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Bandwidth => "Bandwidth",
            Self::Rate => "Rate",
            Self::Messages => "Messages",
            Self::Name => "Name",
        }
    }
}

/// Active tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    TopicsBus,
    NodeRegistry,
    Intercept,
    Graph,
    Help,
}

impl Tab {
    pub(crate) fn index(self) -> usize {
        match self {
            Self::TopicsBus => 0,
            Self::NodeRegistry => 1,
            Self::Intercept => 2,
            Self::Graph => 3,
            Self::Help => 4,
        }
    }

    pub(crate) fn from_digit(digit: char) -> Option<Self> {
        match digit {
            '1' => Some(Self::TopicsBus),
            '2' => Some(Self::NodeRegistry),
            '3' => Some(Self::Intercept),
            '4' => Some(Self::Graph),
            '5' => Some(Self::Help),
            _ => None,
        }
    }
}

/// All mutable application state: bus-wide and per-topic traffic stats, UI selection, and
/// filter/sort/pause toggles.
pub(crate) struct AppState {
    pub topics: BTreeMap<String, TopicStats>,
    pub sort_by: SortColumn,
    pub active_tab: Tab,
    pub selected_topic: usize,
    pub selected_node: usize,
    pub paused: bool,
    pub show_inspector: bool,
    pub search_focused: bool,
    pub filter_query: String,

    pub total_bus_messages: u64,
    pub total_bus_bytes: u64,
    window_bus_messages: u64,
    window_bus_bytes: u64,
    pub bus_rate_hz: f64,
    pub bus_bandwidth_kbs: f64,
    pub peak_bandwidth_kbs: f64,
    pub peak_rate_hz: f64,

    /// The newest `/clock` value in seconds, once one arrived.
    pub sim_clock_sec: Option<f64>,
    pub node_states: BTreeMap<String, (hypha_node::sdk::msgs::hypha_msgs::NodeState, Instant)>,
    pub node_introspections: BTreeMap<String, hypha_node::sdk::msgs::hypha_msgs::NodeIntrospection>,
    /// The Intercept tab.
    pub intercept: crate::top::intercept::InterceptView,
    /// Scroll position of the topics table; it keeps the selected row in view.
    pub topics_table: ratatui::widgets::TableState,
    /// Scroll position of the node list.
    pub nodes_table: ratatui::widgets::TableState,
    /// CPU and memory of the node processes.
    pub proc_usage: hypha_tools::proc_usage::ProcSampler,
    /// The Graph tab.
    pub graph: crate::top::graph_ui::GraphView,
    /// Every node seen in the registry since start, by name. A node that left the
    /// registry, as one that exits on Ctrl+C does, stays shown as dead.
    seen_nodes: BTreeMap<String, hypha_node::sdk::NodeInfo>,
}

impl AppState {
    pub(crate) fn new() -> Self {
        Self {
            topics: BTreeMap::new(),
            sort_by: SortColumn::Name,
            active_tab: Tab::TopicsBus,
            selected_topic: 0,
            selected_node: 0,
            paused: false,
            show_inspector: false,
            search_focused: false,
            filter_query: String::new(),
            total_bus_messages: 0,
            total_bus_bytes: 0,
            window_bus_messages: 0,
            window_bus_bytes: 0,
            bus_rate_hz: 0.0,
            bus_bandwidth_kbs: 0.0,
            peak_bandwidth_kbs: 0.0,
            peak_rate_hz: 0.0,
            sim_clock_sec: None,
            node_states: BTreeMap::new(),
            node_introspections: BTreeMap::new(),
            intercept: crate::top::intercept::InterceptView::default(),
            topics_table: ratatui::widgets::TableState::default(),
            nodes_table: ratatui::widgets::TableState::default(),
            proc_usage: hypha_tools::proc_usage::ProcSampler::default(),
            graph: crate::top::graph_ui::GraphView::default(),
            seen_nodes: BTreeMap::new(),
        }
    }

    /// The nodes to show: the registry's `listed` nodes, then every node seen
    /// before whose name the registry no longer lists, as dead. A node of the
    /// same name that joins again replaces the remembered one. CLI tool nodes,
    /// `_hyphacli_<tool>` and this monitor among them, are not shown.
    pub(crate) fn track_nodes(
        &mut self,
        registry: Vec<hypha_node::sdk::NodeInfo>,
    ) -> Vec<hypha_node::sdk::NodeInfo> {
        let mut listed: Vec<hypha_node::sdk::NodeInfo> = Vec::with_capacity(registry.len());
        for node in registry {
            if !hypha_node::sdk::is_cli_node_name(&node.name) {
                listed.push(node);
            }
        }
        let mut listed_names: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for node in &listed {
            listed_names.insert(node.name.as_str());
        }
        let mut shown: Vec<hypha_node::sdk::NodeInfo> = Vec::with_capacity(listed.len());
        for (name, node) in &self.seen_nodes {
            let left_registry = !listed_names.contains(name.as_str());
            if left_registry {
                let mut gone = node.clone();
                gone.is_alive = false;
                shown.push(gone);
            }
        }
        for node in &listed {
            let remembered = self.seen_nodes.get(&node.name);
            // An alive entry wins over a dead one of the same name.
            let keep_remembered = remembered.is_some_and(|known| known.is_alive && !node.is_alive);
            if !keep_remembered {
                self.seen_nodes.insert(node.name.clone(), node.clone());
            }
        }
        shown.extend(listed);
        shown
    }

    /// Records one bus sample: per-topic and bus-wide counters, `/clock` sim-time decode,
    /// `/<node>/state` health telemetry, and `/<node>/introspection` blueprint snapshot decode.
    pub(crate) fn on_sample(&mut self, topic: &str, schema_hash: u64, payload: &[u8]) {
        let bytes = payload.len() as u64;
        self.total_bus_bytes = self.total_bus_bytes.saturating_add(bytes);
        self.total_bus_messages = self.total_bus_messages.saturating_add(1);
        self.window_bus_bytes = self.window_bus_bytes.saturating_add(bytes);
        self.window_bus_messages = self.window_bus_messages.saturating_add(1);

        if topic == "/clock"
            && let Some(time) = hypha_node::sdk::RosTime::decode_clock(payload)
        {
            self.sim_clock_sec = Some(f64::from(time.secs) + f64::from(time.nsecs) / 1e9);
        }

        // A topic's name is not its type: a `/<x>/state` of another type is no node. Each
        // decode is gated by the frame's schema hash, the identity of the message type.
        let is_intercept_state: bool = crate::top::intercept::is_state_topic(topic)
            && carries::<hypha_node::sdk::msgs::std_msgs::String>(schema_hash);
        let is_node_state: bool = topic.ends_with("/state")
            && !crate::top::intercept::is_state_topic(topic)
            && carries::<hypha_node::sdk::msgs::hypha_msgs::NodeState>(schema_hash);
        let is_node_introspection: bool = topic.ends_with("/introspection")
            && carries::<hypha_node::sdk::msgs::hypha_msgs::NodeIntrospection>(schema_hash);

        if is_intercept_state {
            self.intercept.on_state(payload, Instant::now());
        } else if is_node_state
            && let Some(candidate) = topic.strip_prefix('/').and_then(|t| t.strip_suffix("/state"))
        {
            use hypha_node::sdk::message::DecodeOwned as _;
            if let Ok(st) = hypha_node::sdk::msgs::hypha_msgs::NodeState::decode_from(payload) {
                let key = if st.node.is_empty() { candidate.to_string() } else { st.node.clone() };
                self.node_states.insert(key, (st, Instant::now()));
            }
        }

        if is_node_introspection
            && let Some(candidate) =
                topic.strip_prefix('/').and_then(|t| t.strip_suffix("/introspection"))
        {
            use hypha_node::sdk::message::DecodeOwned as _;
            if let Ok(snapshot) =
                hypha_node::sdk::msgs::hypha_msgs::NodeIntrospection::decode_from(payload)
            {
                let key = if snapshot.node.is_empty() {
                    candidate.to_string()
                } else {
                    snapshot.node.clone()
                };
                self.node_introspections.insert(key, snapshot);
            }
        }

        let stats =
            self.topics.entry(topic.to_string()).or_insert_with(|| TopicStats::new(schema_hash));
        stats.record_sample(schema_hash, payload);
    }

    /// Returns one topic whose retained schema identity is still unresolved.
    pub(crate) fn unresolved_schema(
        &self,
        retry_at: &std::collections::BTreeMap<(String, u64), Instant>,
        now: Instant,
    ) -> Option<(String, u64)> {
        self.topics.iter().find_map(|(topic, stats)| {
            let key = (topic.clone(), stats.schema_hash);
            let unresolved = stats.schema_hash != 0
                && (stats.data_type.is_empty() || stats.md5.is_empty())
                && retry_at.get(&key).is_none_or(|deadline| *deadline <= now);
            unresolved.then(|| (topic.clone(), stats.schema_hash))
        })
    }

    /// Applies a completed retained schema lookup to a matching topic.
    pub(crate) fn apply_schema(
        &mut self,
        topic: &str,
        schema_hash: u64,
        data_type: &str,
        md5: &str,
    ) {
        let Some(stats) = self.topics.get_mut(topic) else {
            return;
        };
        if stats.schema_hash != schema_hash {
            return;
        }
        data_type.clone_into(&mut stats.data_type);
        md5.clone_into(&mut stats.md5);
    }

    /// Recomputes bus-wide and per-topic rate/bandwidth over the elapsed window, pushes
    /// history samples, and updates peaks. A no-op for `dt_sec <= 0.0`. The caller skips
    /// calling this entirely while paused, so the displayed rate freezes.
    pub(crate) fn calculate_rates(&mut self, dt_sec: f64) {
        if dt_sec <= 0.0 {
            return;
        }

        self.bus_rate_hz = approx_f64(self.window_bus_messages) / dt_sec;
        self.bus_bandwidth_kbs = approx_f64(self.window_bus_bytes) / (dt_sec * 1024.0);
        self.window_bus_messages = 0;
        self.window_bus_bytes = 0;

        if self.bus_bandwidth_kbs > self.peak_bandwidth_kbs {
            self.peak_bandwidth_kbs = self.bus_bandwidth_kbs;
        }
        if self.bus_rate_hz > self.peak_rate_hz {
            self.peak_rate_hz = self.bus_rate_hz;
        }

        for stats in self.topics.values_mut() {
            stats.rate_hz = approx_f64(stats.window_messages) / dt_sec;
            stats.bandwidth_kbs = approx_f64(stats.window_bytes) / (dt_sec * 1024.0);
            stats.window_messages = 0;
            stats.window_bytes = 0;
        }
    }

    /// Topics visible under the current filter, sorted by the active `sort_by` column.
    pub(crate) fn visible_topics(&self) -> Vec<(&String, &TopicStats)> {
        let query = self.filter_query.as_str();
        let mut list: Vec<(&String, &TopicStats)> = self
            .topics
            .iter()
            .filter(|(topic, stats)| {
                query.is_empty() || topic.contains(query) || stats.data_type.contains(query)
            })
            .collect();

        match self.sort_by {
            SortColumn::Bandwidth => list.sort_by(|(_, a), (_, b)| {
                b.bandwidth_kbs.partial_cmp(&a.bandwidth_kbs).unwrap_or(std::cmp::Ordering::Equal)
            }),
            SortColumn::Rate => list.sort_by(|(_, a), (_, b)| {
                b.rate_hz.partial_cmp(&a.rate_hz).unwrap_or(std::cmp::Ordering::Equal)
            }),
            SortColumn::Messages => list.sort_by_key(|(_, s)| std::cmp::Reverse(s.total_messages)),
            SortColumn::Name => list.sort_by_key(|(t, _)| (*t).clone()),
        }
        list
    }

    /// Health state snapshot for `node_name`, if one has been received on `/<node>/state`.
    pub(crate) fn node_state(
        &self,
        node_name: &str,
    ) -> Option<&(hypha_node::sdk::msgs::hypha_msgs::NodeState, Instant)> {
        self.node_states.get(node_name)
    }

    /// Introspection snapshot for `node_name`, if one has been received on `/<node>/introspection`.
    pub(crate) fn node_introspection(
        &self,
        node_name: &str,
    ) -> Option<&hypha_node::sdk::msgs::hypha_msgs::NodeIntrospection> {
        self.node_introspections.get(node_name)
    }

    /// Active topics whose names begin with `/<node_name>/`.
    pub(crate) fn node_scoped_topics(&self, node_name: &str) -> Vec<(&String, &TopicStats)> {
        let prefix = format!("/{node_name}/");
        self.topics.iter().filter(|(topic, _)| topic.starts_with(&prefix)).collect()
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// Formats a bandwidth value in KB/s, scaling to MB/s or GB/s.
pub(crate) fn format_bandwidth(kb_sec: f64) -> String {
    if kb_sec >= 1024.0 * 1024.0 {
        format!("{:.2} GB/s", kb_sec / (1024.0 * 1024.0))
    } else if kb_sec >= 1024.0 {
        format!("{:.2} MB/s", kb_sec / 1024.0)
    } else {
        format!("{kb_sec:.1} KB/s")
    }
}

/// Formats a rate in Hz, scaling to kHz.
pub(crate) fn format_rate(hz: f64) -> String {
    if hz >= 1000.0 { format!("{:.2} kHz", hz / 1000.0) } else { format!("{hz:.1} Hz") }
}

/// Formats a byte count, scaling to KB/MB/GB.
pub(crate) fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.2} GB", approx_f64(bytes) / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.2} MB", approx_f64(bytes) / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", approx_f64(bytes) / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
#[allow(clippy::indexing_slicing)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    /// The frame hash of message type `M`, as the bus carries it.
    fn identity_hash<M: hypha_node::sdk::message::MessageIdentity>() -> u64 {
        hypha_node::sdk::schema_hash_from_md5(M::SCHEMA_HASH).expect("a valid MD5")
    }

    fn node(name: &str, pid: i32, is_alive: bool) -> hypha_node::sdk::NodeInfo {
        hypha_node::sdk::NodeInfo {
            name: name.to_string(),
            executable: String::new(),
            pid,
            is_alive,
        }
    }

    #[test]
    fn a_node_that_left_the_registry_stays_shown_as_dead() {
        let mut state = AppState::new();
        let shown = state.track_nodes(vec![node("a", 1, true), node("b", 2, true)]);
        assert_eq!(shown.len(), 2);

        // `b` exited cleanly, so the registry no longer lists it.
        let shown = state.track_nodes(vec![node("a", 1, true)]);
        assert_eq!(shown.len(), 2);
        let b = shown.iter().find(|n| n.name == "b").expect("b");
        assert!(!b.is_alive);
        assert_eq!(b.pid, 2);

        // `b` joins again in a new process: it is shown once, alive.
        let shown = state.track_nodes(vec![node("a", 1, true), node("b", 3, true)]);
        let bs: Vec<&hypha_node::sdk::NodeInfo> = shown.iter().filter(|n| n.name == "b").collect();
        assert_eq!(bs.len(), 1);
        assert!(bs[0].is_alive);
        assert_eq!(bs[0].pid, 3);
    }

    #[test]
    fn cli_tool_nodes_are_not_shown() {
        let mut state = AppState::new();
        let registry = vec![
            node("a", 1, true),
            node("_hyphacli_top", 2, true),
            node("_hyphacli_top_schema_resolver_0", 2, true),
            node("_hyphacli_topic", 3, false),
        ];
        let shown = state.track_nodes(registry);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].name, "a");
    }

    #[test]
    fn topics_are_sorted_by_name_by_default() {
        let mut state = AppState::new();
        for topic in ["/zeta", "/alpha", "/mid"] {
            state.on_sample(topic, 1, &[0]);
        }
        let names: Vec<&str> = state.visible_topics().iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(names, ["/alpha", "/mid", "/zeta"]);
        assert_eq!(state.sort_by.next(), SortColumn::Bandwidth);
    }

    #[test]
    fn on_sample_tracks_scoped_topics() {
        let mut state = AppState::new();
        state.on_sample("/gnss_node/fix", 5678, &[1, 2, 3]);
        state.on_sample("/gnss_node/velocity", 5679, &[4, 5, 6]);
        state.on_sample("/other_node/data", 9999, &[7, 8, 9]);

        let scoped = state.node_scoped_topics("gnss_node");
        assert_eq!(scoped.len(), 2);
    }

    #[test]
    fn hypha_top_ignores_a_state_sample_of_another_layout() {
        use hypha_node::sdk::message::EncodeInto as _;
        use hypha_node::sdk::msgs::hypha_msgs::{NodeState, RequiredInput};

        let mut state = AppState::new();
        let node_st = NodeState { node: "old_layout".to_string(), ..NodeState::default() };
        let mut buffer = vec![0_u8; node_st.encoded_len()];
        let len = node_st.encode_into(&mut buffer).expect("encode should succeed");
        buffer.truncate(len);
        // A recording made before the layout changed carries another schema hash.
        state.on_sample("/old_layout/state", identity_hash::<RequiredInput>(), &buffer);
        assert!(state.node_states.is_empty());
    }

    #[test]
    fn on_sample_tracks_node_state() {
        use hypha_node::sdk::message::EncodeInto as _;
        use hypha_node::sdk::msgs::hypha_msgs::{NodeState, RequiredInput};

        let mut state = AppState::new();
        let node_st = NodeState {
            node: "controller_node".to_string(),
            state: NodeState::DEGRADED,
            inputs: vec![RequiredInput {
                topic: "/imu/data".to_string(),
                verdict: RequiredInput::STALE,
                age_nanos: 250_000_000,
                ..RequiredInput::default()
            }],
            callbacks_invoked: 1200,
            callback_failures: 1,
            slow_callbacks: 3,
            ..NodeState::default()
        };

        let mut buffer = vec![0_u8; node_st.encoded_len()];
        let len = node_st.encode_into(&mut buffer).expect("encode should succeed");
        buffer.truncate(len);
        state.on_sample("/controller_node/state", identity_hash::<NodeState>(), &buffer);

        assert!(state.node_state("controller_node").is_some());
        let (snapshot, _) = state.node_state("controller_node").expect("node state exists");
        assert_eq!(snapshot.state, NodeState::DEGRADED);
        assert_eq!(snapshot.inputs.len(), 1);
        assert_eq!(snapshot.inputs[0].topic, "/imu/data");
        assert_eq!(snapshot.inputs[0].verdict, RequiredInput::STALE);
        assert_eq!(snapshot.callbacks_invoked, 1200);
        assert_eq!(snapshot.callback_failures, 1);
    }

    #[test]
    fn on_sample_tracks_node_introspection() {
        use hypha_node::sdk::message::EncodeInto as _;
        use hypha_node::sdk::msgs::hypha_msgs::{IntrospectedEndpoint, NodeIntrospection};

        let mut state = AppState::new();
        let intro = NodeIntrospection {
            node: "planner_node".to_string(),
            domain: "default".to_string(),
            pid: 4321,
            created: "2026-11-04T12:00:00Z".to_string(),
            hypha: "0.1.0".to_string(),
            configuration: Vec::new(),
            subscribers: vec![IntrospectedEndpoint {
                id: 1,
                topic: "/plan_request".to_string(),
                data_type: "std_msgs/String".to_string(),
                queue_depth: 5,
                latched: false,
                output_cycle: String::new(),
                output_failure: 0,
            }],
            publishers: vec![IntrospectedEndpoint {
                id: 2,
                topic: "/plan_reply".to_string(),
                data_type: "std_msgs/String".to_string(),
                queue_depth: 0,
                latched: true,
                output_cycle: String::new(),
                output_failure: 0,
            }],
            cycles: Vec::new(),
            input_bindings: Vec::new(),
        };

        let mut buffer = vec![0_u8; intro.encoded_len()];
        let len = intro.encode_into(&mut buffer).expect("encode should succeed");
        buffer.truncate(len);
        state.on_sample(
            "/planner_node/introspection",
            identity_hash::<NodeIntrospection>(),
            &buffer,
        );

        assert!(state.node_introspection("planner_node").is_some());
        let snapshot =
            state.node_introspection("planner_node").expect("introspection snapshot exists");
        assert_eq!(snapshot.pid, 4321);
        assert_eq!(snapshot.publishers.len(), 1);
        assert_eq!(snapshot.publishers[0].topic, "/plan_reply");
        assert_eq!(snapshot.subscribers.len(), 1);
        assert_eq!(snapshot.subscribers[0].topic, "/plan_request");
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tab_tests {
    use super::*;

    const DIGITS: [char; 5] = ['1', '2', '3', '4', '5'];

    #[test]
    fn the_tabs_are_numbered_one_to_five() {
        let expected = [Tab::TopicsBus, Tab::NodeRegistry, Tab::Intercept, Tab::Graph, Tab::Help];
        for (digit, tab) in DIGITS.iter().zip(expected) {
            assert_eq!(Tab::from_digit(*digit), Some(tab), "digit {digit}");
        }
    }

    #[test]
    fn tab_indexes_are_dense_and_unique() {
        let mut indexes: Vec<usize> = Vec::new();
        for digit in DIGITS {
            let tab = Tab::from_digit(digit).expect("a tab for each digit");
            let index = tab.index();
            assert!(index < DIGITS.len(), "index {index} is out of the tab range");
            assert!(!indexes.contains(&index), "index {index} is used twice");
            indexes.push(index);
        }
        indexes.sort_unstable();
        assert_eq!(indexes, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn digits_outside_the_tab_range_select_no_tab() {
        for digit in ['0', '6', '7', '9'] {
            assert_eq!(Tab::from_digit(digit), None, "digit {digit}");
        }
    }

    #[test]
    fn a_rate_tick_computes_the_bus_numbers_without_history() {
        let mut state = AppState::new();
        state.window_bus_messages = 10;
        state.window_bus_bytes = 2048;
        state.calculate_rates(1.0);
        assert!((state.bus_rate_hz - 10.0).abs() < f64::EPSILON);
        assert!((state.bus_bandwidth_kbs - 2.0).abs() < f64::EPSILON);

        state.window_bus_messages = 4;
        state.window_bus_bytes = 1024;
        state.calculate_rates(1.0);
        assert!((state.bus_rate_hz - 4.0).abs() < f64::EPSILON);
        assert!((state.bus_bandwidth_kbs - 1.0).abs() < f64::EPSILON);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod identity_tests {
    use super::*;
    use hypha_node::sdk::msgs::hypha_msgs::{NodeIntrospection, NodeState};
    use hypha_node::sdk::msgs::std_msgs::String as StdString;
    use std::path::Path;

    fn identity_hash<M: hypha_node::sdk::message::MessageIdentity>() -> u64 {
        hypha_node::sdk::schema_hash_from_md5(M::SCHEMA_HASH).expect("a valid MD5")
    }

    fn encoded<M: hypha_node::sdk::message::EncodeInto>(message: &M) -> Vec<u8> {
        let mut buffer = vec![0_u8; message.encoded_len()];
        let length = message.encode_into(&mut buffer).expect("the message encodes");
        buffer.truncate(length);
        buffer
    }

    fn node_state_frame(node: &str) -> Vec<u8> {
        encoded(&NodeState { node: node.to_string(), ..NodeState::default() })
    }

    #[test]
    fn a_state_topic_of_another_type_creates_no_node() {
        let mut state = AppState::new();
        let frame = node_state_frame("door");
        state.on_sample("/door/state", identity_hash::<StdString>(), &frame);
        assert!(state.node_states.is_empty(), "a std_msgs/String topic is no node");
    }

    #[test]
    fn a_node_state_with_the_right_hash_creates_the_node() {
        let mut state = AppState::new();
        let frame = node_state_frame("planner");
        state.on_sample("/planner/state", identity_hash::<NodeState>(), &frame);
        assert_eq!(state.node_states.len(), 1);
        assert!(state.node_state("planner").is_some());
    }

    #[test]
    fn introspection_needs_the_right_hash() {
        let manifest =
            NodeIntrospection { node: "planner".to_string(), ..NodeIntrospection::default() };
        let frame = encoded(&manifest);

        let mut wrong = AppState::new();
        wrong.on_sample("/planner/introspection", identity_hash::<StdString>(), &frame);
        assert!(wrong.node_introspections.is_empty());

        let mut right = AppState::new();
        right.on_sample("/planner/introspection", identity_hash::<NodeIntrospection>(), &frame);
        assert!(right.node_introspection("planner").is_some());
    }

    #[test]
    fn a_zero_hash_is_not_decoded() {
        let mut state = AppState::new();
        let state_frame = node_state_frame("planner");
        state.on_sample("/planner/state", 0, &state_frame);
        assert!(state.node_states.is_empty(), "a zero hash names no type");

        let manifest =
            NodeIntrospection { node: "planner".to_string(), ..NodeIntrospection::default() };
        state.on_sample("/planner/introspection", 0, &encoded(&manifest));
        assert!(state.node_introspections.is_empty(), "a zero hash names no type");
    }

    #[test]
    fn a_stdstring_state_topic_is_an_intercept_state_only_with_its_own_type() {
        let config_path = Path::new("/nonexistent/intercept.toml");
        let document = crate::top::intercept::tests::state_doc(config_path, 1);

        let mut wrong = AppState::new();
        wrong.on_sample("/hypha/intercept/bench/state", identity_hash::<NodeState>(), &document);
        assert!(wrong.intercept.backend().is_none(), "another type is no backend");
        assert_eq!(wrong.intercept.bad_state_docs, 0, "and is not decoded at all");

        let mut right = AppState::new();
        right.on_sample("/hypha/intercept/bench/state", identity_hash::<StdString>(), &document);
        assert!(right.intercept.backend().is_some(), "a std_msgs/String state is a backend");
    }

    #[test]
    fn topic_counts_still_include_non_node_state_samples() {
        let mut state = AppState::new();
        let frame = node_state_frame("door");
        state.on_sample("/door/state", identity_hash::<StdString>(), &frame);
        let counted = state.topics.get("/door/state").expect("the topic is counted");
        assert_eq!(counted.total_messages, 1);
        assert_eq!(counted.total_bytes, u64::try_from(frame.len()).expect("a small frame"));
    }
}
