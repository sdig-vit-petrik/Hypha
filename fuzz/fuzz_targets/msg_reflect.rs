//! The dynamic message walker of the tools on an arbitrary definition and payload.
//! Input: a two-byte length, that many bytes of definition text (invalid UTF-8 is
//! replaced), then the payload.
#![no_main]

use std::fmt::Write as _;

use hypha_tools::msg_reflect::{MessageSchema, WalkEvent, decode_message, walk};
use libfuzzer_sys::fuzz_target;

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
                self.enter += 1;
                None
            }
            WalkEvent::Leave => {
                self.leave += 1;
                None
            }
            WalkEvent::ArrayStart { prefix, .. } => {
                self.array_start += 1;
                Some(prefix.clone())
            }
            WalkEvent::ArrayEnd => {
                self.array_end += 1;
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

fuzz_target!(|input: &[u8]| {
    let Some((len, rest)) = input.split_first_chunk::<2>() else {
        return;
    };
    let len = usize::from(u16::from_le_bytes(*len)).min(rest.len());
    let (definition, bytes) = rest.split_at(len);
    let definition = String::from_utf8_lossy(definition);
    let Ok(mut schema) = MessageSchema::from_definition("p/T0", &definition) else {
        return;
    };
    let mut tally = Tally::default();
    let walked = walk(&mut schema, "p/T0", bytes, &mut |event| tally.on(&event, bytes.len()));
    assert_eq!(tally.outside, None);
    if let Ok(read) = walked {
        assert!(read <= bytes.len(), "read {read} of {} bytes", bytes.len());
        assert_eq!(tally.enter, tally.leave);
        assert_eq!(tally.array_start, tally.array_end);
    }
    let mut out = String::new();
    let printed = decode_message(&mut out, "p/T0", &mut schema, bytes);
    assert_eq!(printed, walked.is_ok(), "{out}");
    let _ = writeln!(out);
});
