//! Input handling: map a key to an App transition per the three-mode model
//! (Normal / Interact / Focus — §5.2). The power bar always lists the bindings
//! valid in the current mode/run-state.

use crossterm::event::{KeyCode, KeyEvent};

use crate::tui::app::{App, Mode, Pane};

pub fn handle_key(app: &mut App, key: KeyEvent) {
    // A blocking failure pop-over swallows all input but its dismissal (§5.1).
    if app.error_popover.is_some() {
        if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
            app.error_popover = None;
        }
        return;
    }

    match app.mode {
        Mode::Normal => normal(app, key),
        Mode::Interact => interact(app, key),
        Mode::Focus => focus(app, key),
    }
}

fn normal(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Enter => app.mode = Mode::Interact,
        KeyCode::Char('z') => app.mode = Mode::Focus,
        KeyCode::Left | KeyCode::Up => move_focus(app, -1),
        KeyCode::Right | KeyCode::Down => move_focus(app, 1),
        _ => {}
    }
}

fn interact(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.mode = Mode::Normal,
        _ => pane_action(app, key),
    }
}

fn focus(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('z') | KeyCode::Char('q') => app.mode = Mode::Normal,
        // Pane interactions still work inside the zoom (§5.2).
        _ => pane_action(app, key),
    }
}

/// The focused pane's Interact bindings — shared by Interact and Focus modes.
fn pane_action(app: &mut App, key: KeyEvent) {
    match app.focus {
        Pane::Workflows => match key.code {
            KeyCode::Up => select(app, -1),
            KeyCode::Down => select(app, 1),
            KeyCode::Char('p') => app.refresh_plan(),
            KeyCode::Char('r') => app.launch_run(false),
            KeyCode::Char('R') => app.launch_run(true),
            _ => {}
        },
        Pane::Run => {
            if key.code == KeyCode::Char('x') {
                app.stop_run();
            }
        }
        Pane::Inventory => match key.code {
            KeyCode::Up => app.inventory_scroll = app.inventory_scroll.saturating_sub(1),
            KeyCode::Down => app.inventory_scroll = app.inventory_scroll.saturating_add(1),
            _ => {}
        },
        Pane::Stats => {}
    }
}

fn move_focus(app: &mut App, delta: isize) {
    let cur = Pane::ORDER
        .iter()
        .position(|&p| p == app.focus)
        .unwrap_or(0);
    let n = Pane::ORDER.len() as isize;
    let next = (cur as isize + delta).rem_euclid(n) as usize;
    app.focus = Pane::ORDER[next];
}

fn select(app: &mut App, delta: isize) {
    if app.workflows.is_empty() {
        return;
    }
    let n = app.workflows.len() as isize;
    app.selected = (app.selected as isize + delta).rem_euclid(n) as usize;
    app.inventory_scroll = 0;
    app.refresh_plan();
}
