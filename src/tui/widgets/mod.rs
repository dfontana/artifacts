//! Reusable panes. Each is a self-contained widget rendered at two scales —
//! *compact* (its grid cell) and *modal* (a centered zoom pop-over) — per
//! `plans/TUI.md` §4.2. Shared chrome/formatting helpers live here.

pub mod form;
pub mod header;
pub mod inventory;
pub mod plan;
pub mod run;
pub mod stats;
pub mod workflows;

use ratatui::style::{Style, Stylize};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders};

use crate::tui::theme;

/// The scale a pane renders at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    /// The pane's grid cell.
    Compact,
    /// A centered zoom pop-over (Focus mode / `z`).
    Modal,
}

/// A bordered pane block whose border is accented when focused (Normal mode).
pub fn pane_block(title: &str, focused: bool) -> Block<'_> {
    let border = if focused {
        Style::default().fg(theme::ACCENT)
    } else {
        Style::default().fg(theme::DIM)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .title(Span::from(title).fg(theme::TITLE).bold())
}

/// A fixed-width text bar (`▓▓▓░░`) for a 0..=1 ratio — used inline where a full
/// ratatui gauge would not fit on one header line.
pub fn text_bar(ratio: f64, width: usize) -> String {
    let ratio = ratio.clamp(0.0, 1.0);
    let filled = (ratio * width as f64).round() as usize;
    let filled = filled.min(width);
    let mut s = String::with_capacity(width * 3);
    for _ in 0..filled {
        s.push('▓');
    }
    for _ in filled..width {
        s.push('░');
    }
    s
}

/// Compact thousands: `3400 → "3.4k"`, `950 → "950"`.
pub fn fmt_k(n: u32) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

/// Group digits with commas: `1240 → "1,240"`.
pub fn fmt_commas(n: u32) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

/// Seconds of cooldown remaining, derived from an RFC3339 expiration vs the wall
/// clock (§3.8). Display-only, so wall-clock (not the driver clock) is fine; an
/// empty or unparseable/past timestamp is `0`. The parse and the remaining-
/// seconds rule are game-data interpretation, so they live in `core::cooldown`,
/// not this widget module.
pub fn cooldown_remaining(expiration: &str) -> f64 {
    let Some(exp) = artifacts_core::cooldown::parse_rfc3339(expiration) else {
        return 0.0;
    };
    let now = jiff::Timestamp::now();
    artifacts_core::cooldown::remaining(exp, now).as_secs_f64()
}

/// Truncate a string to `width` display columns (best-effort char count), adding
/// an ellipsis when clipped. Keeps labels from overflowing a pane.
pub fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    if width <= 1 {
        return "…".to_string();
    }
    let mut out: String = s.chars().take(width - 1).collect();
    out.push('…');
    out
}
