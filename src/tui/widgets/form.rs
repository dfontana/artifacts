//! Param-form modal (M6): a centered box over the dashboard, one line per
//! declared param — `name (type)  [value]` — with the focused field accented,
//! its inline shape error in the error color beside it, and the prefix-filtered
//! suggestions underneath (the first is what Tab accepts). The pure state lives
//! in `tui::form` (driven by `event::form_key`); this module only renders it,
//! matching the palette/error pop-over overlay pattern in `tui::ui`.

use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
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
        ];
        if focused {
            // Split the value at the cursor so the caret sits where edits land.
            let cut = field
                .value
                .char_indices()
                .nth(field.cursor)
                .map(|(b, _)| b)
                .unwrap_or(field.value.len());
            spans.push(Span::raw(field.value[..cut].to_string()));
            spans.push(Span::from("▏").fg(theme::ACCENT));
            spans.push(Span::raw(field.value[cut..].to_string()));
        } else {
            spans.push(Span::raw(field.value.clone()));
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
            "↑↓ field   ←→ move cursor   type to edit   ⇥ complete   \
             space toggles bool   ⏎ run   esc cancel",
        )
        .fg(theme::DIM),
    ));

    // The box is 60% of the screen wide (min 24 columns, but never wider than
    // the terminal — `clamp` would panic when the terminal is under 24 wide);
    // wrap long lines (doc, help, enum errors) to the inner width and grow the
    // box to the wrapped row count (+2 border) so nothing is clipped, then
    // center it like the other overlays.
    let box_w = (f.area().width * 3 / 5).max(24).min(f.area().width);
    let inner_w = box_w.saturating_sub(2);
    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    let rows = para.line_count(inner_w).min(u16::MAX as usize) as u16;
    let area = centered(f.area(), box_w, rows.saturating_add(2));
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
    f.render_widget(para, inner);
}

/// A centered rect exactly `width` × `height` (each clamped to `r`) — the form
/// sizes to its wrapped content instead of a fixed percentage.
fn centered(r: Rect, width: u16, height: u16) -> Rect {
    let [mid] = Layout::vertical([Constraint::Length(height.min(r.height))])
        .flex(Flex::Center)
        .areas(r);
    let [rect] = Layout::horizontal([Constraint::Length(width.min(r.width))])
        .flex(Flex::Center)
        .areas(mid);
    rect
}
