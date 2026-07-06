//! Plan summary: the browsing `PlanResult` for the selected workflow (offline
//! planner, always safe — §4.4, §5.1). No longer its own tile — these lines are
//! folded into the bottom of the Workflows pane so the plan sits next to the
//! selection that drives it.

use ratatui::style::Stylize;
use ratatui::text::{Line, Span};

use crate::tui::app::App;
use crate::tui::theme;

use super::truncate;

/// The plan summary as a block of lines, clipped to `width` columns. Owned
/// (`'static`) so it can be dropped into a `Paragraph` without borrowing `app`.
pub fn plan_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    match app.plan() {
        None => vec![Line::from(Span::from("select a workflow").fg(theme::DIM))],
        Some(Err(e)) => vec![Line::from(
            Span::from(truncate(&format!("plan error: {e}"), width)).fg(theme::BAD),
        )],
        Some(Ok(p)) => {
            let mut lines = vec![Line::from(vec![
                if p.feasible {
                    Span::from("feasible").fg(theme::OK).bold()
                } else {
                    Span::from("infeasible").fg(theme::BAD).bold()
                },
                Span::raw(format!("  ~{:.0}s   {} actions", p.seconds, p.actions)),
            ])];
            if let Some((label, n)) = p.assumptions.first() {
                lines.push(Line::from(
                    Span::from(format!("{label}: {n}")).fg(theme::DIM),
                ));
            }
            for b in p.blockers.iter().take(2) {
                lines.push(Line::from(
                    Span::from(truncate(&format!("✗ {b}"), width)).fg(theme::BAD),
                ));
            }
            for w in p.warnings.iter().take(1) {
                lines.push(Line::from(
                    Span::from(truncate(&format!("! {w}"), width)).fg(theme::WARN),
                ));
            }
            lines
        }
    }
}
