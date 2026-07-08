//! Param-form modal (M6): a centered box over the dashboard, one line per
//! declared param — `name (type)  [value]` — with the focused field accented,
//! its inline shape error in the error color beside it, and the prefix-filtered
//! suggestions underneath (the first is what Tab accepts). The pure state lives
//! in `tui::form` (driven by `event::form_key`); this module only renders it,
//! matching the palette/error pop-over overlay pattern in `tui::ui`.

use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::tui::app::App;
use crate::tui::glyphs;
use crate::tui::theme;

pub fn render(f: &mut Frame, app: &App) {
    let Some(form) = &app.form else {
        return;
    };

    let mut lines: Vec<Line> = Vec::new();
    if let Some(doc) = &form.doc {
        lines.push(Line::from(Span::from(doc.clone()).fg(theme::DIM)));
    }
    lines.push(Line::raw(""));

    for (i, field) in form.fields.iter().enumerate() {
        let focused = i == form.focus;
        // Optional params carry the same `?` mark as the workflow-list hint.
        let opt = if field.spec.required { "" } else { "?" };
        let name = Span::from(format!(
            "{}{} ({})",
            field.spec.name,
            opt,
            field.spec.ptype.as_str()
        ));
        let mut spans = vec![
            if focused {
                Span::from(format!("{} ", glyphs::SELECTED)).fg(theme::ACCENT)
            } else {
                Span::raw("  ")
            },
            if focused {
                name.fg(theme::ACCENT).bold()
            } else {
                name
            },
            Span::raw("  ["),
            Span::raw(field.value.clone()),
        ];
        if focused {
            spans.push(Span::from("▏").fg(theme::DIM));
        }
        spans.push(Span::raw("]"));
        if let Some(err) = &field.error {
            spans.push(Span::raw("  "));
            spans.push(Span::from(err.clone()).fg(theme::BAD));
        }
        lines.push(Line::from(spans));

        // Suggestions render only under the focused field; `›` marks the one
        // Tab accepts.
        if focused {
            for (j, s) in field.suggestions().iter().enumerate() {
                let (marker, color) = if j == 0 {
                    ("› ", theme::ACCENT)
                } else {
                    ("  ", theme::DIM)
                };
                lines.push(Line::from(vec![
                    Span::raw("      "),
                    Span::from(format!("{marker}{s}")).fg(color),
                ]));
            }
        }
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(
        Span::from(
            "↑↓ field   type to edit   ⇥ complete   space toggles bool   ⏎ run   esc cancel",
        )
        .fg(theme::DIM),
    ));

    // Size the box to its content (+2 for the border), centered like the other
    // overlays.
    let area = centered(f.area(), 60, lines.len() as u16 + 2);
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::ACCENT))
        .title(
            Span::from(format!(" params: {} ", form.workflow))
                .fg(theme::TITLE)
                .bold(),
        );
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(lines), inner);
}

/// A centered rect `percent_x` wide and exactly `height` rows tall (clamped to
/// `r`) — the form sizes to its content instead of a fixed percentage.
fn centered(r: Rect, percent_x: u16, height: u16) -> Rect {
    let [mid] = Layout::vertical([Constraint::Length(height.min(r.height))])
        .flex(Flex::Center)
        .areas(r);
    let [rect] = Layout::horizontal([Constraint::Percentage(percent_x)])
        .flex(Flex::Center)
        .areas(mid);
    rect
}
