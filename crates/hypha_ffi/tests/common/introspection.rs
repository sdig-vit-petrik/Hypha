//! Read a node's introspection snapshot through the C ABI, and dump it as plain lines
//! for assertions: a test aid, not a table format.

use std::ptr;

use hypha_core::message::DecodeOwned as _;
use hypha_ffi::{HyphaNode, HyphaStatus, hypha_node_introspect};
use hypha_msgs::hypha_msgs::{
    IntrospectedConfig, IntrospectedCycle, IntrospectedEndpoint, IntrospectedInput,
    NodeIntrospection,
};

/// The snapshot of `node`, read with one size probe and one copy.
#[allow(clippy::expect_used, reason = "a refused snapshot is a test failure")]
pub(crate) fn snapshot(node: *const HyphaNode) -> NodeIntrospection {
    let mut length = 0_usize;
    let probe = unsafe { hypha_node_introspect(node, ptr::null_mut(), 0, &raw mut length) };
    assert_eq!(probe, HyphaStatus::TooLarge);
    let mut buffer = vec![0_u8; length];
    let status =
        unsafe { hypha_node_introspect(node, buffer.as_mut_ptr(), buffer.len(), &raw mut length) };
    assert_eq!(status, HyphaStatus::Ok);
    buffer.truncate(length);
    NodeIntrospection::decode_from(&buffer).expect("a NodeIntrospection")
}

pub(crate) fn introspection_text(node: *const HyphaNode) -> String {
    dump(&snapshot(node))
}

fn cell(text: &str) -> String {
    if text.is_empty() { "-".to_owned() } else { text.to_owned() }
}

fn value_cell(text: &str) -> String {
    if text.is_empty() { "\"\"".to_owned() } else { text.to_owned() }
}

fn source_text(source: u8) -> &'static str {
    match source {
        IntrospectedConfig::SOURCE_PARAMETER => "parameter",
        IntrospectedConfig::SOURCE_APPLICATION => "application",
        IntrospectedConfig::SOURCE_COMMAND_LINE => "command line",
        IntrospectedConfig::SOURCE_DEFAULT => "default",
        IntrospectedConfig::SOURCE_IGNORED => "ignored",
        _ => "?",
    }
}

fn push_section(out: &mut String, header: &str, rows: &[Vec<String>]) {
    out.push_str(header);
    out.push('\n');
    if rows.is_empty() {
        out.push_str("  (none)\n");
    }
    for row in rows {
        out.push_str("  ");
        out.push_str(&row.join("  "));
        out.push('\n');
    }
}

/// The typed facts of one endpoint as comma-joined tokens, or `-` when it has none.
fn endpoint_facts(endpoint: &IntrospectedEndpoint) -> String {
    let mut facts = Vec::new();
    if endpoint.queue_depth > 0 {
        facts.push(format!("depth={}", endpoint.queue_depth));
    }
    if endpoint.latched {
        facts.push("latched".to_owned());
    }
    let failure = match endpoint.output_failure {
        IntrospectedEndpoint::OUTPUT_FAIL_RUN => Some("fail_run"),
        IntrospectedEndpoint::OUTPUT_DROP_POOL_EXHAUSTED => Some("drop_pool_exhausted"),
        _ => None,
    };
    if let Some(failure) = failure {
        facts.push(format!("output={}/{failure}", endpoint.output_cycle));
    }
    if facts.is_empty() {
        return "-".to_owned();
    }
    facts.join(",")
}

fn endpoint_rows(endpoints: &[IntrospectedEndpoint]) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    for endpoint in endpoints {
        rows.push(vec![cell(&endpoint.topic), cell(&endpoint.data_type), endpoint_facts(endpoint)]);
    }
    rows
}

/// The capture facts of one binding as comma-joined tokens, such as
/// `queued=16,silence=50000000,stagnation=0,counted`.
fn capture_facts(input: &IntrospectedInput) -> String {
    let mut facts = Vec::new();
    if input.mode == IntrospectedInput::MODE_QUEUED {
        facts.push(format!("queued={}", input.max_samples_per_iteration));
    } else {
        facts.push("latest".to_owned());
    }
    if input.supervised {
        facts.push(format!("silence={}", input.silence_nanos));
        facts.push(format!("stagnation={}", input.stagnation_nanos));
        let state = if input.ignored { "ignored" } else { "counted" };
        facts.push(state.to_owned());
    }
    facts.join(",")
}

pub(crate) fn subscriber(node: *const HyphaNode, topic: &str) -> Option<IntrospectedEndpoint> {
    snapshot(node).subscribers.into_iter().find(|row| row.topic == topic)
}

pub(crate) fn publisher(node: *const HyphaNode, topic: &str) -> Option<IntrospectedEndpoint> {
    snapshot(node).publishers.into_iter().find(|row| row.topic == topic)
}

pub(crate) fn cycle(node: *const HyphaNode, name: &str) -> Option<IntrospectedCycle> {
    snapshot(node).cycles.into_iter().find(|row| row.name == name)
}

pub(crate) fn binding(
    node: *const HyphaNode,
    cycle: &str,
    index: u32,
) -> Option<IntrospectedInput> {
    let bindings = snapshot(node).input_bindings;
    bindings.into_iter().find(|row| row.cycle == cycle && row.index == index)
}

pub(crate) fn dump(snapshot: &NodeIntrospection) -> String {
    let mut out = format!("hypha node {}\n", cell(&snapshot.node));
    let mut configuration = Vec::new();
    for row in &snapshot.configuration {
        let default = if row.has_default { value_cell(&row.default_value) } else { "-".to_owned() };
        configuration.push(vec![
            cell(&row.key),
            value_cell(&row.value),
            default,
            source_text(row.source).to_owned(),
        ]);
    }
    push_section(&mut out, "configuration  value  default  source", &configuration);
    push_section(&mut out, "subscribers  type  facts", &endpoint_rows(&snapshot.subscribers));
    push_section(&mut out, "publishers  type  facts", &endpoint_rows(&snapshot.publishers));
    let mut cycles = Vec::new();
    for cycle in &snapshot.cycles {
        let tenths = if cycle.has_period {
            10_000_000_000_u64.checked_div(cycle.period_nanos)
        } else {
            None
        };
        let rate = tenths.map_or_else(String::new, |tenths| {
            format!(
                "{}.{} Hz",
                tenths.checked_div(10).unwrap_or(0),
                tenths.checked_rem(10).unwrap_or(0)
            )
        });
        cycles.push(vec![
            cell(&cycle.name),
            cell(&rate),
            cycle.inputs.to_string(),
            cycle.outputs.to_string(),
        ]);
    }
    push_section(&mut out, "cycles  rate  inputs  outputs", &cycles);
    if !snapshot.input_bindings.is_empty() {
        let mut bindings = Vec::new();
        for input in &snapshot.input_bindings {
            let role = if input.required { "required" } else { "optional" };
            bindings.push(vec![
                cell(&input.cycle),
                input.index.to_string(),
                cell(&input.topic),
                role.to_owned(),
            ]);
        }
        push_section(&mut out, "cycle  input  topic  role", &bindings);
        let mut captures = Vec::new();
        for input in &snapshot.input_bindings {
            captures.push(vec![cell(&input.cycle), input.index.to_string(), capture_facts(input)]);
        }
        push_section(&mut out, "cycle  input  capture", &captures);
    }
    out
}
