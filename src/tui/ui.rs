//! Frame composition. The screen is fixed chrome — a header line and a slim
//! footer — wrapped around the tiling runtime's body (§4.1). Focus mode zooms
//! the focused pane as a centered modal, and a blocking failure pop-over sits on
//! top of everything.

use std::borrow::Cow;

use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use ratatui_hypertile_extras::{HypertileRuntime, InputMode};

use crate::tui::app::{App, Pane, RunState};
use crate::tui::palette;
use crate::tui::plugins::Panes;
use crate::tui::theme;
use crate::tui::widgets::{self, Scale};

pub fn render(f: &mut Frame, app: &App, runtime: &mut HypertileRuntime, panes: &Panes) {
    // Snapshot the shared view once per frame — the header and the zoom overlay
    // borrow this rather than each re-fetching it.
    let view = app.view.get();
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(3), // header
        Constraint::Min(0),    // tiled body
        Constraint::Length(1), // footer
    ])
    .areas(f.area());

    widgets::header::render(f, header, &view);
    runtime.render(body, f.buffer_mut());
    render_footer(f, footer, app, runtime, panes);

    // Zoom (`z`): the focused pane again, large and centered, over the body.
    if app.zoom {
        if let Some(kind) = runtime.focused_pane().and_then(|id| panes.kind(id)) {
            let area = centered_rect(70, 70, f.area());
            f.render_widget(Clear, area);
            let buf = f.buffer_mut();
            match kind {
                Pane::Stats => widgets::stats::render(buf, area, &view, true, Scale::Modal),
                Pane::Inventory => {
                    widgets::inventory::render(buf, area, app, &view, true, Scale::Modal)
                }
                Pane::Workflows => widgets::workflows::render(buf, area, app, true, Scale::Modal),
                Pane::Run => widgets::run::render(buf, area, app, true, Scale::Modal),
            }
        }
    }

    // The command palette floats over the dashboard while open.
    if app.palette.is_some() {
        render_palette(f, app);
    }

    // The param form (M6) floats over the dashboard while open.
    if app.form.is_some() {
        widgets::form::render(f, app);
    }

    // The workflow description tooltip (`t`) floats over the dashboard, showing
    // the selected workflow's full wrapped `:doc` — the list row truncates it.
    if app.tooltip {
        render_workflow_tooltip(f, app);
    }

    // The blocking failure pop-over sits on top of everything (§5.1).
    if let Some(err) = &app.error_popover {
        render_error_popover(f, err);
    }
}

/// A slim, borderless footer: a mode badge plus the bindings valid right now,
/// followed by any transient status/prompt.
fn render_footer(f: &mut Frame, area: Rect, app: &App, runtime: &HypertileRuntime, panes: &Panes) {
    let mode = runtime.mode();
    let focused = runtime.focused_pane().and_then(|id| panes.kind(id));

    let (label, badge) = match mode {
        InputMode::Layout => (
            " NAV ",
            Style::default().fg(Color::Black).bg(theme::ACCENT).bold(),
        ),
        InputMode::PluginInput => (
            " EDIT ",
            Style::default().fg(Color::Black).bg(theme::OK).bold(),
        ),
    };

    let mut spans = vec![
        Span::styled(label, badge),
        Span::raw("  "),
        Span::from(bindings(mode, focused, app)).fg(theme::DIM),
    ];
    if let Some(msg) = &app.status_msg {
        spans.push(Span::raw("   "));
        let color = if app.infeasible_prompt {
            theme::WARN
        } else {
            theme::DIM
        };
        spans.push(Span::from(msg.as_str()).fg(color));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The bindings valid in the current mode / focused pane. `Cow` because almost
/// every arm is fixed text — no need to allocate it per frame.
fn bindings(mode: InputMode, focused: Option<Pane>, app: &App) -> Cow<'static, str> {
    match mode {
        InputMode::Layout => "p commands   ⏎ edit   ⇧+arrows move   [ ] resize".into(),
        InputMode::PluginInput => match focused {
            Some(Pane::Workflows) if app.infeasible_prompt => {
                "↑↓ select   p plan   r run   R override   t tip   esc back".into()
            }
            Some(Pane::Workflows) => "↑↓ select   p plan   r run   t tip   esc back".into(),
            Some(Pane::Run) if app.run_state == RunState::Running => "x stop   esc back".into(),
            Some(Pane::Inventory) => "↑↓ scroll   esc back".into(),
            _ => "esc back".into(),
        },
    }
}

fn render_error_popover(f: &mut Frame, err: &str) {
    let area = centered_rect(60, 40, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BAD))
        .title(Span::from(" run failed ").fg(theme::BAD).bold());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let text = vec![
        Line::from(Span::from(err.to_string()).fg(theme::BAD)),
        Line::raw(""),
        Line::from(Span::from("press Esc to dismiss").fg(theme::DIM)),
    ];
    f.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), inner);
}

/// The workflow description tooltip (`t`): a centered, wrapped box showing the
/// selected workflow's param hint and full `:doc` — everything the single-line
/// list row truncates. A workflow whose schema failed to marshal shows that
/// error instead, so the same key reveals *why* a row can't run.
fn render_workflow_tooltip(f: &mut Frame, app: &App) {
    let Some(wf) = app.selected_workflow() else {
        return;
    };
    let area = centered_rect(60, 40, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::ACCENT))
        .title(Span::from(format!(" {} ", wf.name)).fg(theme::TITLE).bold());
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    match &wf.info {
        Ok(info) => {
            if let Some(hint) = crate::tui::form::param_hint(info) {
                lines.push(Line::from(Span::from(hint).fg(theme::DIM)));
                lines.push(Line::raw(""));
            }
            match &info.doc {
                Some(doc) => lines.push(Line::from(Span::raw(doc.clone()))),
                None => lines.push(Line::from(Span::from("no :doc").fg(theme::DIM))),
            }
        }
        Err(e) => lines.push(Line::from(Span::from(format!("schema error: {e}")).fg(theme::BAD))),
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// The command palette: a centered box with the fuzzy query on top and the
/// filtered command list below, the highlighted row marked with `›`.
fn render_palette(f: &mut Frame, app: &App) {
    let Some(pal) = &app.palette else {
        return;
    };
    let items = palette::filtered(&pal.query);

    let area = centered_rect(50, 55, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::ACCENT))
        .title(Span::from(" commands ").fg(theme::TITLE).bold());
    let inner = block.inner(area);
    f.render_widget(block, area);

    let [query_area, list_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);

    let query = Line::from(vec![
        Span::from("› ").fg(theme::ACCENT),
        Span::raw(pal.query.clone()),
        Span::from("▏").fg(theme::DIM),
    ]);
    f.render_widget(Paragraph::new(query), query_area);

    if items.is_empty() {
        f.render_widget(
            Paragraph::new(Span::from("  no match").fg(theme::DIM)),
            list_area,
        );
        return;
    }
    let rows: Vec<ListItem> = items.iter().map(|c| ListItem::new(c.label())).collect();
    let list = List::new(rows)
        .highlight_style(Style::default().fg(theme::ACCENT).bold())
        .highlight_symbol("› ");
    let mut state = ListState::default();
    state.select(Some(pal.selected.min(items.len() - 1)));
    f.render_stateful_widget(list, list_area, &mut state);
}

/// A centered rect `percent_x` × `percent_y` of `r`.
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let [mid] = Layout::vertical([Constraint::Percentage(percent_y)])
        .flex(Flex::Center)
        .areas(r);
    let [rect] = Layout::horizontal([Constraint::Percentage(percent_x)])
        .flex(Flex::Center)
        .areas(mid);
    rect
}
