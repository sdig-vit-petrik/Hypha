//! The Mycelium palette: the colours of every terminal surface the tools draw.

use ratatui::style::Color;

/// Screen background.
pub const BG: Color = Color::Rgb(0x0f, 0x0c, 0x17);
/// Panels, gauge tracks, the tmux status line.
pub const SURFACE: Color = Color::Rgb(0x1a, 0x16, 0x26);
/// Badges, the second status segment.
pub const RAISED: Color = Color::Rgb(0x2a, 0x24, 0x40);
/// Text.
pub const FG: Color = Color::Rgb(0xeb, 0xe6, 0xf7);
/// Labels, borders, idle windows.
pub const MUTED: Color = Color::Rgb(0x7a, 0x73, 0x94);
/// Selection, headers, the main badge.
pub const ACCENT: Color = Color::Rgb(0xb4, 0x8c, 0xff);
/// Keys, rates, secondary figures.
pub const ACCENT2: Color = Color::Rgb(0x5e, 0xc8, 0xff);
/// Running, at rate, playing.
pub const OK: Color = Color::Rgb(0x7c, 0xf5, 0xb4);
/// Slow, paused, filters.
pub const WARN: Color = Color::Rgb(0xff, 0xc4, 0x6b);
/// Stale, dead, failed.
pub const ERR: Color = Color::Rgb(0xff, 0x5c, 0x8a);
/// Text on an [`ACCENT`] or [`OK`] background.
pub const ON_ACCENT: Color = Color::Rgb(0x12, 0x0d, 0x1f);

/// Escape code that resets all terminal attributes.
pub const RESET: &str = "\x1b[0m";

/// `#rrggbb` for a tmux style. Every role is RGB; any other colour gives tmux's `default`.
#[must_use]
pub fn hex(color: Color) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => "default".to_string(),
    }
}

/// Truecolor foreground escape code, bold when `bold`. Every role is RGB; any other
/// colour gives the terminal's default foreground.
#[must_use]
pub fn fg(color: Color, bold: bool) -> String {
    let weight = if bold { "1;" } else { "" };
    match color {
        Color::Rgb(r, g, b) => format!("\x1b[{weight}38;2;{r};{g};{b}m"),
        _ => format!("\x1b[{weight}39m"),
    }
}

#[cfg(test)]
mod tests {
    use super::{ACCENT, ON_ACCENT, fg, hex};

    #[test]
    fn derives_tmux_and_escape_formats() {
        assert_eq!(hex(ACCENT), "#b48cff");
        assert_eq!(hex(ON_ACCENT), "#120d1f");
        assert_eq!(fg(ACCENT, true), "\x1b[1;38;2;180;140;255m");
        assert_eq!(fg(ACCENT, false), "\x1b[38;2;180;140;255m");
    }
}
