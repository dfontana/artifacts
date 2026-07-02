//! Plan pane: the browsing `PlanResult` for the selected workflow (offline
//! planner, always safe — §4.4, §5.1). Display-only, so it carries a plain
//! border rather than a focus border.

use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::tui::app::App;
use crate::tui::theme;

use super::truncate;

pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let name = app
        .selected_workflow()
        .map(|w| w.name.as_str())
        .unwrap_or_default();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::from(format!("PLAN  {name}")).fg(theme::TITLE).bold());
    let inner = block.inner(area);
    f.render_widget(block, area);

    let width = inner.width as usize;
    let lines: Vec<Line> = match &app.plan {
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
    };
    f.render_widget(Paragraph::new(lines), inner);
}
