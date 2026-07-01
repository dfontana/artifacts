//! Reusable panes. Each is a self-contained widget rendered at two scales —
//! *compact* (its grid cell) and *modal* (a centered zoom pop-over) — per
//! `plans/TUI.md` §4.2. Shared chrome/formatting helpers live here.

pub mod header;
pub mod inventory;
pub mod plan;
pub mod run;
pub mod stats;
pub mod workflows;

use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::style::{Style, Stylize};
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
        .title(title.to_string().fg(theme::TITLE).bold())
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
/// empty or unparseable/past timestamp is `0`.
pub fn cooldown_remaining(expiration: &str) -> f64 {
    let Some(exp) = parse_rfc3339_epoch(expiration) else {
        return 0.0;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    (exp - now).max(0.0)
}

/// Minimal RFC3339 → Unix-epoch-seconds parser for `YYYY-MM-DDTHH:MM:SS[.fff][Z]`
/// (UTC). Returns `None` on anything it doesn't recognize — the cooldown bar then
/// simply reads empty rather than erroring. Kept dependency-free (display-only).
fn parse_rfc3339_epoch(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let b = s.as_bytes();
    // Positions are fixed: 0000-00-00T00:00:00
    let num = |lo: usize, hi: usize| -> Option<i64> { s.get(lo..hi)?.parse().ok() };
    if b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') {
        return None;
    }
    let year = num(0, 4)?;
    let month = num(5, 7)?;
    let day = num(8, 10)?;
    let hour = num(11, 13)?;
    let min = num(14, 16)?;
    let sec = num(17, 19)?;
    // Optional fractional seconds after a '.'.
    let frac = if s.as_bytes().get(19) == Some(&b'.') {
        let rest = &s[20..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            0.0
        } else {
            format!("0.{digits}").parse().unwrap_or(0.0)
        }
    } else {
        0.0
    };
    let days = days_from_civil(year, month, day);
    Some((days * 86400 + hour * 3600 + min * 60 + sec) as f64 + frac)
}

/// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`). Correct for all
/// Gregorian dates; no external date dependency.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
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
