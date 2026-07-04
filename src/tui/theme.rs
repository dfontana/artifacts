//! Centralized colors. One small module so the palette (and a future ASCII/mono
//! theme) is a single-file change (`plans/TUI.md` §4.3).

use ratatui::style::Color;

/// Focused-pane border + active-step accent.
pub const ACCENT: Color = Color::Cyan;
/// Unreached / structural chrome.
pub const DIM: Color = Color::DarkGray;
/// Feasible plan, done steps.
pub const OK: Color = Color::Green;
/// Infeasible plan, failures, the death step.
pub const BAD: Color = Color::Red;
/// Advisory warnings.
pub const WARN: Color = Color::Yellow;
/// A skipped (guarded-out) step — muted, distinct from a warning.
pub const SKIP: Color = Color::LightYellow;
/// Gold amount.
pub const GOLD: Color = Color::Yellow;
/// The xp bar fill.
pub const XP: Color = Color::Magenta;
/// The cooldown bar fill.
pub const COOLDOWN: Color = Color::Blue;
/// Pane titles.
pub const TITLE: Color = Color::White;
