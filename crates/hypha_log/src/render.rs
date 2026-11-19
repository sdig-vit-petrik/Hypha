//! One log entry as bytes.

use std::io::Write;
use std::ops::Range;

use crate::level::Level;

/// The longest message body. A longer body is cut.
pub(crate) const MAX_BODY: usize = 16 * 1024;

const COLOUR_RESET: &[u8] = b"\x1b[0m";

/// One rendered entry, without colour. `label` is where the level label sits.
pub(crate) struct Line {
    pub(crate) bytes: Vec<u8>,
    label: Range<usize>,
}

impl Line {
    /// The line with the level label in its ANSI colour.
    pub(crate) fn coloured(&self, level: Level) -> Vec<u8> {
        let colour = level.colour().as_bytes();
        let (head, rest) = self.bytes.split_at(self.label.start.min(self.bytes.len()));
        let (label, tail) = rest.split_at(self.label.len().min(rest.len()));
        [head, colour, label, COLOUR_RESET, tail].concat()
    }
}

/// Render an entry: wall time, optional simulated time, level, module, body. The clocks
/// are read once.
pub(crate) fn render_line(level: Level, module: &str, text: &[u8]) -> Line {
    let mut line = Vec::with_capacity(text.len().min(MAX_BODY).saturating_add(96));
    // Writing into a Vec cannot fail.
    let _ = write!(line, "[{}]", hypha_time::wall_now().utc());
    if hypha_time::is_sim_time() {
        let _ = write!(line, " [sim {:.6}]", hypha_time::framework_now().seconds());
    }
    line.extend_from_slice(b" [");
    let start = line.len();
    line.extend_from_slice(level.label().as_bytes());
    let label = start..line.len();
    let _ = write!(line, "] [{module}] ");

    let kept = text.get(..MAX_BODY).unwrap_or(text);
    line.extend_from_slice(kept);
    let dropped = text.len().saturating_sub(kept.len());
    if dropped > 0 {
        let _ = write!(line, " [truncated {dropped} bytes]");
    }
    if !line.ends_with(b"\n") {
        line.push(b'\n');
    }
    Line { bytes: line, label }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_coloured_line_is_the_plain_line_with_the_label_wrapped() {
        let line = render_line(Level::Warn, "unit", b"body");
        let plain = String::from_utf8(line.bytes.clone()).unwrap_or_default();
        let coloured = String::from_utf8(line.coloured(Level::Warn)).unwrap_or_default();
        let expected = plain.replacen("[WARN]", "[\x1b[33mWARN\x1b[0m]", 1);
        assert_eq!(coloured, expected);
        assert!(plain.ends_with(" [WARN] [unit] body\n"), "{plain}");
    }
}
