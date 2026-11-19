//! Safe registry for one node's introspection rows.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::{
    ConfigRow, CycleOutputRow, CycleRow, EndpointRow, Identity, InputBindingRow, InputMode,
    Snapshot, Supervision,
};

/// One row the registry holds, by the section it belongs to.
#[derive(Debug, Clone)]
pub enum Row {
    /// Subscriber endpoint.
    Subscriber(EndpointRow),
    /// Publisher endpoint.
    Publisher(EndpointRow),
    /// Cycle registration.
    Cycle(CycleRow),
    /// One cycle input binding.
    InputBinding(InputBindingRow),
}

#[derive(Default)]
struct State {
    configuration: Vec<ConfigRow>,
    rows: Vec<(u64, Row)>,
    next_id: u64,
}

/// A safe, synchronized registry of one node's introspection rows.
pub struct IntrospectionRegistry {
    identity: Identity,
    state: Mutex<State>,
}

impl IntrospectionRegistry {
    /// Create an empty registry for one node identity.
    #[must_use]
    pub fn new(identity: Identity) -> Self {
        Self { identity, state: Mutex::new(State::default()) }
    }

    /// Create an empty shared registry wrapped in an [`Arc`].
    #[must_use]
    pub fn shared(identity: Identity) -> Arc<Self> {
        Arc::new(Self::new(identity))
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn add(&self, row: Row) -> u64 {
        let mut state = self.state();
        state.next_id = state.next_id.wrapping_add(1);
        let id = state.next_id;
        state.rows.push((id, row));
        id
    }

    fn update(&self, id: u64, change: impl FnOnce(&mut Row)) {
        let mut state = self.state();
        if let Some((_, row)) = state.rows.iter_mut().find(|(row_id, _)| *row_id == id) {
            change(row);
        }
    }

    fn remove(&self, id: u64) {
        self.state().rows.retain(|(row_id, _)| *row_id != id);
    }

    /// Record or replace a configuration value by key.
    pub fn record_config(&self, row: ConfigRow) {
        let mut state = self.state();
        match state.configuration.iter_mut().find(|known| known.key == row.key) {
            Some(known) => *known = row,
            None => state.configuration.push(row),
        }
    }

    /// Take a point-in-time copy of the current rows.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        let state = self.state();
        let mut snapshot = Snapshot {
            identity: self.identity.clone(),
            configuration: state.configuration.clone(),
            ..Snapshot::default()
        };
        for (id, row) in &state.rows {
            match row {
                Row::Subscriber(endpoint) => snapshot.subscribers.push(with_id(endpoint, *id)),
                Row::Publisher(endpoint) => snapshot.publishers.push(with_id(endpoint, *id)),
                Row::Cycle(cycle) => snapshot.cycles.push(cycle.clone()),
                Row::InputBinding(input) => snapshot.input_bindings.push(input.clone()),
            }
        }
        snapshot
    }
}

/// A copy of `endpoint` that carries the registry id of its row.
fn with_id(endpoint: &EndpointRow, id: u64) -> EndpointRow {
    let mut row = endpoint.clone();
    row.id = id;
    row
}

/// An introspection row whose lifetime follows the endpoint that owns it.
pub struct IntrospectionRow {
    registry: Arc<IntrospectionRegistry>,
    id: u64,
}

impl IntrospectionRow {
    /// Add a row that is removed when this handle is dropped.
    #[must_use]
    pub fn new(registry: &Arc<IntrospectionRegistry>, row: Row) -> Self {
        let id = registry.add(row);
        Self { registry: Arc::clone(registry), id }
    }

    /// The id of this row, as a snapshot reports it. Never 0.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Record the cycle output that owns a publisher row.
    pub fn bind_output(&self, output: CycleOutputRow) {
        self.registry.update(self.id, |row| {
            if let Row::Publisher(endpoint) = row {
                endpoint.output = Some(output);
            }
        });
    }

    /// Set the capture mode of an input binding row.
    pub fn set_input_capture(&self, mode: InputMode) {
        self.registry.update(self.id, |row| {
            if let Row::InputBinding(input) = row {
                input.mode = mode;
            }
        });
    }

    /// Set the supervision of an input binding row, or clear it with `None`.
    pub fn set_input_supervision(&self, supervision: Option<Supervision>) {
        self.registry.update(self.id, |row| {
            if let Row::InputBinding(input) = row {
                input.supervision = supervision;
            }
        });
    }

    /// Set the binding counts of a cycle row.
    pub fn set_cycle_bindings(&self, inputs: usize, outputs: usize) {
        self.registry.update(self.id, |row| {
            if let Row::Cycle(cycle) = row {
                cycle.inputs = inputs;
                cycle.outputs = outputs;
            }
        });
    }

    /// Record the trigger topic of a cycle row. Other rows are left unchanged.
    pub fn set_cycle_trigger(&self, topic: &str) {
        self.registry.update(self.id, |row| {
            if let Row::Cycle(cycle) = row {
                cycle.trigger = Some(topic.to_owned());
            }
        });
    }
}

impl Drop for IntrospectionRow {
    fn drop(&mut self) {
        self.registry.remove(self.id);
    }
}
