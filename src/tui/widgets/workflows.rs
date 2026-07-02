//! Workflows pane: the selectable list scanned from `fennel/workflows/*.fnl`
//! (§4.4). Selection drives the Plan pane and is the launch target for `r`.

use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::tui::app::App;
use crate::tui::glyphs;
use crate::tui::theme;

use super::{pane_block, Scale};

pub fn render(f: &mut Frame, area: Rect, app: &App, focused: bool, _scale: Scale) {
    let block = pane_block("WORKFLOWS", focused);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.workflows.is_empty() {
        f.render_widget(
            Paragraph::new(Span::from("no fennel/workflows/*.fnl found").fg(theme::DIM)),
            inner,
        );
        return;
    }

    let rows = inner.height as usize;
    // Keep the selection visible with a simple window.
    let start = app.selected.saturating_sub(rows.saturating_sub(1));
    let lines: Vec<Line> = app
        .workflows
        .iter()
        .enumerate()
        .skip(start)
        .take(rows)
        .map(|(i, wf)| {
            if i == app.selected {
                Line::from(vec![
                    Span::from(format!("{} ", glyphs::SELECTED)).fg(theme::ACCENT),
                    Span::from(wf.name.clone()).fg(theme::ACCENT).bold(),
                ])
            } else {
                Line::from(format!("  {}", wf.name))
            }
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}
