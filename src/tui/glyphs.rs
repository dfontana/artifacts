//! All icon glyphs live here so the theme is one module swap away from an
//! ASCII-only fallback (`plans/TUI.md` §4.3, §9). The row-state glyphs are the
//! centralized surface the run panel renders; words appear only in the zoomed
//! detail view.
//!
//! The step glyphs are widely-supported Unicode (a braille spinner, a check, a
//! ballot mark) rather than nerd-font private-use codepoints, so the UI is
//! legible without the documented nerd font installed; swapping to nerd-font
//! icons is a one-constant change here.

use crate::tui::reducer::Cell;

pub const DONE: &str = "✓";
pub const PENDING: &str = "·";
pub const SKIPPED: &str = "⊘";
pub const LOOP: &str = "↻";
pub const WHEN: &str = "⎇";

/// Animated braille spinner frames for the active step.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn spinner(frame: usize) -> &'static str {
    SPINNER[frame % SPINNER.len()]
}

/// The glyph for a non-loop, non-when row's cell state. `frame` animates the
/// active spinner.
pub fn cell(cell: Cell, frame: usize) -> &'static str {
    match cell {
        Cell::Done => DONE,
        Cell::Active => spinner(frame),
        Cell::Pending => PENDING,
        Cell::Skipped => SKIPPED,
    }
}

// ─── header / stats icons ────────────────────────────────────────────────────
pub const HP: &str = "♥";
pub const ATK: &str = "⚔";
pub const DEF: &str = "⛨";
pub const GOLD: &str = "◆";
pub const POS: &str = "⌖";
pub const SELECTED: &str = "▸";
