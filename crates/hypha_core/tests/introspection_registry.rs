//! Introspection rows follow their handle lifetime, and configuration rows are replaced by key.

use hypha_core::introspection::{
    ConfigRow, ConfigSource, CycleOutputRow, EndpointRow, Identity, InputBindingRow, InputMode,
    InputRole, IntrospectionRegistry, IntrospectionRow, OutputFailure, Row, Supervision,
};

#[test]
fn endpoint_rows_follow_handle_lifetime_and_updates_replace_by_key() {
    let registry =
        IntrospectionRegistry::shared(Identity { node: "test_node".into(), ..Identity::default() });
    registry.record_config(ConfigRow {
        key: "rate".into(),
        value: "10".into(),
        default: Some("10".into()),
        source: ConfigSource::Default,
    });
    registry.record_config(ConfigRow {
        key: "rate".into(),
        value: "20".into(),
        default: None,
        source: ConfigSource::Parameter,
    });
    let row = IntrospectionRow::new(
        &registry,
        Row::Publisher(EndpointRow::new("/out", "std_msgs/Float64")),
    );
    row.bind_output(CycleOutputRow { cycle: "drive".into(), failure: OutputFailure::FailRun });
    let snapshot = registry.snapshot();
    assert_eq!(snapshot.configuration.len(), 1);
    assert_eq!(snapshot.configuration.first().map(|row| row.value.as_str()), Some("20"));
    let publisher = snapshot.publishers.first();
    assert_eq!(publisher.map(|row| row.id), Some(row.id()));
    assert_eq!(
        publisher.and_then(|row| row.output.clone()),
        Some(CycleOutputRow { cycle: "drive".into(), failure: OutputFailure::FailRun })
    );
    drop(row);
    assert_eq!(registry.snapshot().publishers.len(), 0);
}

#[test]
fn snapshot_keeps_the_node_identity() {
    let identity = Identity {
        node: "test_node".into(),
        domain: "d".into(),
        pid: 7,
        created: "2026-01-01T00:00:00Z".into(),
        hypha: "1.0.0".into(),
    };
    let registry = IntrospectionRegistry::new(identity.clone());
    assert_eq!(registry.snapshot().identity, identity);
}

#[test]
fn an_input_binding_names_its_subscriber_and_holds_its_capture_and_supervision() {
    let registry = IntrospectionRegistry::shared(Identity::default());
    let subscriber =
        IntrospectionRow::new(&registry, Row::Subscriber(EndpointRow::new("/in", "x/Y")));
    let binding = IntrospectionRow::new(
        &registry,
        Row::InputBinding(InputBindingRow {
            cycle: "control".into(),
            index: 0,
            topic: "/in".into(),
            subscriber: Some(subscriber.id()),
            role: InputRole::Required,
            mode: InputMode::Latest,
            supervision: None,
        }),
    );
    let queued = InputMode::Queued { max_samples_per_iteration: 4 };
    binding.set_input_capture(queued);
    let limits = Supervision { silence_nanos: 50_000_000, stagnation_nanos: 0, ignored: true };
    binding.set_input_supervision(Some(limits));

    let snapshot = registry.snapshot();
    let subscriber_id = snapshot.subscribers.first().map(|row| row.id);
    let input = snapshot.input_bindings.first();
    assert_eq!(input.and_then(|row| row.subscriber), subscriber_id);
    assert_eq!(input.map(|row| row.mode), Some(queued));
    assert_eq!(input.and_then(|row| row.supervision), Some(limits));
}
