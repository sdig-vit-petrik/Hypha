//! Property tests of the dynamic wire walker: for any payload it never panics.

use std::fmt::Write as _;

use hypha_tools::msg_reflect::{MessageSchema, WalkEvent, decode_message, walk};
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

/// Shrunk failures are kept here, relative to the crate directory, and replayed first.
const REGRESSIONS: &str = "proptest-regressions/msg_reflect_properties.txt";

fn config(cases: u32) -> ProptestConfig {
    ProptestConfig {
        cases,
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(REGRESSIONS))),
        ..ProptestConfig::default()
    }
}

const BUILTINS: [&str; 16] = [
    "bool", "byte", "char", "int8", "uint8", "int16", "uint16", "int32", "uint32", "int64",
    "uint64", "float32", "float64", "string", "time", "duration",
];

/// One field: a built-in (by index) or a later type (picked among those after its
/// owner), and its brackets.
fn field() -> impl Strategy<Value = (Option<usize>, usize, &'static str)> {
    (
        proptest::option::of(0..BUILTINS.len()),
        any::<usize>(),
        prop_oneof![3 => Just(""), 1 => Just("[]"), 1 => Just("[2]"), 1 => Just("[0]")],
    )
}

/// The definition text of types `T0` (the root) to `Tn`: `T0`'s body, then one `MSG:`
/// section per later type. A type with no later type holds built-ins only.
fn definition() -> impl Strategy<Value = String> {
    vec(vec(field(), 0..6), 1..5).prop_map(|types| {
        let count = types.len();
        let mut sections = Vec::new();
        for (owner, fields) in types.iter().enumerate() {
            let later = count.saturating_sub(owner.saturating_add(1));
            let lines: Vec<String> = fields
                .iter()
                .enumerate()
                .map(|(index, (builtin, target, brackets))| {
                    let nested = target
                        .checked_rem(later)
                        .map(|offset| owner.saturating_add(1).saturating_add(offset));
                    let type_text = match (builtin, nested) {
                        (Some(builtin), _) => {
                            String::from(BUILTINS.get(*builtin).copied().unwrap_or("int32"))
                        }
                        (None, Some(nested)) => format!("T{nested}"),
                        (None, None) => String::from("uint8"),
                    };
                    format!("{type_text}{brackets} f{index}")
                })
                .collect();
            let body = lines.join("\n");
            if owner == 0 {
                sections.push(body);
            } else {
                sections.push(format!("{}\nMSG: p/T{owner}\n{body}", "=".repeat(80)));
            }
        }
        sections.join("\n")
    })
}

/// A payload made of small counts, which keep the walk going, and other bytes.
fn payload() -> impl Strategy<Value = Vec<u8>> {
    let chunk = prop_oneof![
        3 => (0..4_u32).prop_map(|count| count.to_le_bytes().to_vec()),
        1 => Just(u32::MAX.to_le_bytes().to_vec()),
        3 => vec(any::<u8>(), 0..9),
    ];
    prop_oneof![vec(any::<u8>(), 0..128), vec(chunk, 0..24).prop_map(|chunks| chunks.concat())]
}

/// Counts of the bracketing events, and the first span outside the payload.
#[derive(Default)]
struct Tally {
    enter: usize,
    leave: usize,
    array_start: usize,
    array_end: usize,
    outside: Option<String>,
}

impl Tally {
    fn on(&mut self, event: &WalkEvent<'_>, len: usize) {
        let span = match event {
            WalkEvent::Enter { .. } => {
                self.enter = self.enter.saturating_add(1);
                None
            }
            WalkEvent::Leave => {
                self.leave = self.leave.saturating_add(1);
                None
            }
            WalkEvent::ArrayStart { prefix, .. } => {
                self.array_start = self.array_start.saturating_add(1);
                Some(prefix.clone())
            }
            WalkEvent::ArrayEnd => {
                self.array_end = self.array_end.saturating_add(1);
                None
            }
            WalkEvent::Scalar { span, .. } => Some(span.clone()),
            WalkEvent::Bytes { count, span } => {
                if span.len() != *count {
                    self.outside.get_or_insert_with(|| format!("{count} bytes in {span:?}"));
                }
                Some(span.clone())
            }
            WalkEvent::Element { .. } | WalkEvent::Unresolved { .. } => None,
        };
        let inside = span.as_ref().is_none_or(|span| span.start <= span.end && span.end <= len);
        if !inside {
            self.outside.get_or_insert_with(|| format!("{event:?}"));
        }
    }
}

proptest! {
    #![proptest_config(config(512))]

    #[test]
    fn a_walk_reads_within_the_payload_or_stops_with_an_error(
        definition in definition(),
        bytes in payload(),
    ) {
        let mut schema = MessageSchema::from_definition("p/T0", &definition)
            .map_err(|error| TestCaseError::fail(format!("{error}\n{definition}")))?;
        let mut tally = Tally::default();
        let walked = walk(&mut schema, "p/T0", &bytes, &mut |event| tally.on(&event, bytes.len()));
        prop_assert_eq!(tally.outside, None);
        if let Ok(read) = walked {
            prop_assert!(read <= bytes.len());
            prop_assert_eq!(tally.enter, tally.leave);
            prop_assert_eq!(tally.array_start, tally.array_end);
        }
        let mut out = String::new();
        let printed = decode_message(&mut out, "p/T0", &mut schema, &bytes);
        prop_assert_eq!(printed, walked.is_ok(), "{}", out);
        let _ = writeln!(out);
    }

    #[test]
    fn any_definition_text_builds_a_schema_or_is_refused(
        text in prop_oneof!["\\PC{0,64}", definition().prop_map(|text| text.replace("T1", "T0"))],
        bytes in vec(any::<u8>(), 0..32),
    ) {
        if let Ok(mut schema) = MessageSchema::from_definition("p/T0", &text) {
            let mut out = String::new();
            let _ = decode_message(&mut out, "p/T0", &mut schema, &bytes);
        }
    }
}
