//! Workflows pane: the selectable list scanned from `fennel/workflows/*.fnl`
//! (§4.4). Selection drives the plan summary shown at the bottom of this same
//! pane and is the launch target for `r`.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::tui::app::App;
use crate::tui::form;
use crate::tui::glyphs;
use crate::tui::theme;

use super::{pane_block, plan, Scale};

pub fn render(buf: &mut Buffer, area: Rect, app: &App, focused: bool, _scale: Scale) {
    let block = pane_block("WORKFLOWS", focused);
    let inner = block.inner(area);
    block.render(area, buf);

    if app.workflows.is_empty() {
        Paragraph::new(Span::from("no fennel/workflows/*.fnl found").fg(theme::DIM))
            .render(inner, buf);
        return;
    }

    // Reserve the bottom of the pane for the selected workflow's plan summary.
    // A plan needs ~4 lines; give it that when the pane is tall enough, else let
    // the list have everything.
    let plan_h = if inner.height >= 8 { 5 } else { 0 };
    let [list_area, plan_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(plan_h)]).areas(inner);

    let rows = list_area.height as usize;
    // Keep the selection visible with a simple window.
    let start = app.selected.saturating_sub(rows.saturating_sub(1));
    let lines: Vec<Line> = app
        .workflows
        .iter()
        .enumerate()
        .skip(start)
        .take(rows)
        .map(|(i, wf)| {
            // A parameterized workflow's compact hint — `(target, qty?)`,
            // required params bare, optional marked `?` — then its one-line
            // `:doc`, both dimmed after the name (a schema that failed to
            // marshal has neither to show).
            let hint = wf
                .info
                .as_ref()
                .ok()
                .and_then(form::param_hint)
                .map(|h| Span::from(format!(" {h}")).fg(theme::DIM));
            let doc = wf
                .info
                .as_ref()
                .ok()
                .and_then(|info| info.doc.clone())
                .map(|d| Span::from(format!("  {d}")).fg(theme::DIM));
            if i == app.selected {
                let mut spans = vec![
                    Span::from(format!("{} ", glyphs::SELECTED)).fg(theme::ACCENT),
                    Span::from(wf.name.clone()).fg(theme::ACCENT).bold(),
                ];
                spans.extend(hint);
                spans.extend(doc);
                Line::from(spans)
            } else {
                let mut spans = vec![Span::from(format!("  {}", wf.name))];
                spans.extend(hint);
                spans.extend(doc);
                Line::from(spans)
            }
        })
        .collect();
    Paragraph::new(lines).render(list_area, buf);

    if plan_area.height > 0 {
        // A thin rule separates the list from the plan summary.
        let rule = Block::default()
            .borders(Borders::TOP)
            .border_style(ratatui::style::Style::default().fg(theme::DIM));
        let plan_inner = rule.inner(plan_area);
        rule.render(plan_area, buf);
        let lines = plan::plan_lines(app, plan_inner.width as usize);
        Paragraph::new(lines).render(plan_inner, buf);
    }
}
