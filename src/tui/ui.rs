//! Layout + render dispatch. Composes the header, the two-column body, and the
//! power bar (§4.1), overlays the Focus-mode zoom pop-over and the blocking
//! failure pop-over, and renders the power-bar bindings for the current mode.

use artifacts_core::step::CharacterView;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::tui::app::{App, Mode, Pane, RunState};
use crate::tui::theme;
use crate::tui::widgets::{self, Scale};

pub fn render(f: &mut Frame, app: &App) {
    // Snapshot the shared view once per frame — every pane that needs it borrows
    // this rather than each cloning the whole `CharacterView` independently.
    let view = app.view.get();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(0),    // body
            Constraint::Length(3), // power bar
        ])
        .split(f.area());

    widgets::header::render(f, chunks[0], &view);
    render_body(f, chunks[1], app, &view);
    render_power_bar(f, chunks[2], app);

    // Focus mode: zoom the focused pane as a centered modal (pane interactions
    // still work — §5.2).
    if app.mode == Mode::Focus {
        let area = centered_rect(70, 70, f.area());
        f.render_widget(Clear, area);
        render_pane(f, area, app, &view, app.focus, true, Scale::Modal);
    }

    // The blocking failure pop-over sits on top of everything (§5.1).
    if let Some(err) = &app.error_popover {
        render_error_popover(f, err);
    }
}

fn render_body(f: &mut Frame, area: Rect, app: &App, view: &CharacterView) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(area);

    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(cols[0]);
    render_pane(
        f,
        left[0],
        app,
        view,
        Pane::Stats,
        focused(app, Pane::Stats),
        Scale::Compact,
    );
    render_pane(
        f,
        left[1],
        app,
        view,
        Pane::Inventory,
        focused(app, Pane::Inventory),
        Scale::Compact,
    );

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(38), // workflows
            Constraint::Length(5),      // plan
            Constraint::Min(0),         // run
        ])
        .split(cols[1]);
    render_pane(
        f,
        right[0],
        app,
        view,
        Pane::Workflows,
        focused(app, Pane::Workflows),
        Scale::Compact,
    );
    widgets::plan::render(f, right[1], app);
    render_pane(
        f,
        right[2],
        app,
        view,
        Pane::Run,
        focused(app, Pane::Run),
        Scale::Compact,
    );
}

/// A focusable pane is highlighted when it holds focus and we're not inside the
/// zoom modal (which draws its own accented border).
fn focused(app: &App, pane: Pane) -> bool {
    app.focus == pane && app.mode != Mode::Focus
}

fn render_pane(
    f: &mut Frame,
    area: Rect,
    app: &App,
    view: &CharacterView,
    pane: Pane,
    focused: bool,
    scale: Scale,
) {
    match pane {
        Pane::Stats => widgets::stats::render(f, area, view, focused, scale),
        Pane::Inventory => widgets::inventory::render(f, area, app, view, focused, scale),
        Pane::Workflows => widgets::workflows::render(f, area, app, focused, scale),
        Pane::Run => widgets::run::render(f, area, app, focused, scale),
    }
}

fn render_power_bar(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mode = match app.mode {
        Mode::Normal => "NORMAL",
        Mode::Interact => "INTERACT",
        Mode::Focus => "FOCUS",
    };
    let mut spans = vec![
        Span::from(format!(" {mode} ")).fg(theme::ACCENT).bold(),
        Span::raw("  "),
    ];
    spans.push(Span::raw(bindings(app)));
    if let Some(msg) = &app.status_msg {
        spans.push(Span::raw("   "));
        let color = if app.infeasible_prompt {
            theme::WARN
        } else {
            theme::DIM
        };
        spans.push(Span::from(msg.clone()).fg(color));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), inner);
}

/// The bindings valid in the current mode/run-state (§5.2).
fn bindings(app: &App) -> String {
    if app.error_popover.is_some() {
        return "⏎/esc dismiss".into();
    }
    match app.mode {
        Mode::Normal => "←→↑↓ focus   ⏎ interact   z zoom   q quit".into(),
        Mode::Focus => "pane keys work   z/esc close   q quit".into(),
        Mode::Interact => match app.focus {
            Pane::Workflows => {
                let mut s = String::from("↑/↓ select   p plan   r run");
                if app.infeasible_prompt {
                    s.push_str("   R override");
                }
                s.push_str("   esc normal");
                s
            }
            Pane::Run => {
                let mut s = String::new();
                if app.run_state == RunState::Running {
                    s.push_str("x stop   ");
                }
                s.push_str("esc normal");
                s
            }
            Pane::Inventory => "↑/↓ scroll   esc normal".into(),
            Pane::Stats => "esc normal".into(),
        },
    }
}

fn render_error_popover(f: &mut Frame, err: &str) {
    let area = centered_rect(60, 40, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(ratatui::style::Style::default().fg(theme::BAD))
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

/// A centered rect `percent_x` × `percent_y` of `r`.
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}
