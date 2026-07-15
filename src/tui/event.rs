//! Input routing. The tiling runtime has two input modes: `Layout` (keys drive
//! focus / rearrange / resize) and `PluginInput` (keys go to the focused pane).
//! We keep the layout fixed by dropping the runtime's split/close keys and its
//! vim focus keys; `z`/`q` stay app-level (zoom + quit) and `p` opens the
//! command palette.
//!
//! Routing order: true globals (Ctrl-C) first, then the single open overlay
//! (`App::overlay` — error pop-over, param form, or palette; at most one
//! exists, so exactly one handler owns the key), then base layout/pane keys.
//!
//! Key point: forwarding a key into the runtime can re-enter the shared `App`
//! (a focused plugin's `on_event` borrows it), so we only ever borrow the `App`
//! for the brief app-level mutations here — never while forwarding.

use std::cell::RefCell;
use std::rc::Rc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui_hypertile_extras::{hypertile_event_from_crossterm, HypertileRuntime, InputMode};

use crate::tui::app::{App, Overlay, Palette};
use crate::tui::palette;
use crate::tui::plugins::Panes;

/// True for a `Char` event carrying Ctrl/Alt: those are shortcuts (taken or
/// not), never text — without this guard a modal would insert the literal
/// character instead.
fn is_shortcut_char(key: &KeyEvent) -> bool {
    key.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

pub fn handle_key(
    app: &Rc<RefCell<App>>,
    runtime: &mut HypertileRuntime,
    panes: &Panes,
    key: KeyEvent,
) {
    // Ctrl-C is a true global: it quits from every mode and overlay, checked
    // BEFORE any modal handler can swallow it or insert a literal `c`.
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.borrow_mut().should_quit = true;
        return;
    }

    // The single open overlay owns everything below the globals. Match on a
    // discriminant so no `App` borrow is held while the handlers run.
    enum Owner {
        Error,
        Form,
        Palette,
        Base,
    }
    let owner = match app.borrow().overlay {
        Overlay::Error(_) => Owner::Error,
        Overlay::Form(_) => Owner::Form,
        Overlay::Palette(_) => Owner::Palette,
        Overlay::None => Owner::Base,
    };
    match owner {
        // A blocking failure pop-over swallows all input but its dismissal (§5.1).
        Owner::Error => {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                app.borrow_mut().overlay = Overlay::None;
            }
        }
        Owner::Form => form_key(app, key),
        Owner::Palette => palette_key(app, runtime, panes, key),
        Owner::Base => base_key(app, runtime, key),
    }
}

/// No overlay open: app-level layout keys, everything else to the runtime.
fn base_key(app: &Rc<RefCell<App>>, runtime: &mut HypertileRuntime, key: KeyEvent) {
    // Interacting with a pane: forward everything to the focused plugin (which
    // borrows the `App` itself). The runtime turns Esc back into Layout mode.
    if runtime.mode() == InputMode::PluginInput {
        forward(runtime, key);
        return;
    }

    // Layout mode.
    match key.code {
        KeyCode::Char('q') => app.borrow_mut().should_quit = true,
        KeyCode::Char('p') => {
            app.borrow_mut().overlay = Overlay::Palette(Palette::default());
        }
        KeyCode::Char('z') => {
            let mut a = app.borrow_mut();
            a.zoom = !a.zoom;
        }
        KeyCode::Esc => {
            let mut a = app.borrow_mut();
            a.zoom = false;
            a.tooltip = false;
        }
        // Keep the dashboard fixed: drop the runtime's split (`s`/`v`) and close
        // (`d`) shortcuts, plus its vim focus/move keys.
        KeyCode::Char('s' | 'v' | 'd')
        | KeyCode::Char('h' | 'j' | 'k' | 'l')
        | KeyCode::Char('H' | 'J' | 'K' | 'L') => {}
        // Everything else — arrows (focus), Shift+Arrows (rearrange), `[`/`]`
        // (resize), Enter/`i` (interact) — goes to the runtime.
        _ => forward(runtime, key),
    }
}

/// Drive the open command palette: type to filter, `↑/↓` to move, `Enter` to run
/// the highlighted command, `Esc` to close.
fn palette_key(
    app: &Rc<RefCell<App>>,
    runtime: &mut HypertileRuntime,
    panes: &Panes,
    key: KeyEvent,
) {
    match key.code {
        KeyCode::Esc => app.borrow_mut().overlay = Overlay::None,
        KeyCode::Enter => {
            let mut a = app.borrow_mut();
            let cmd = a.palette_mut().and_then(|p| {
                let items = palette::filtered(&p.query);
                items
                    .get(p.selected.min(items.len().saturating_sub(1)))
                    .copied()
            });
            a.overlay = Overlay::None;
            if let Some(cmd) = cmd {
                palette::execute(cmd, &mut a, runtime, panes);
            }
        }
        KeyCode::Up => {
            if let Some(p) = app.borrow_mut().palette_mut() {
                p.selected = p.selected.saturating_sub(1);
            }
        }
        KeyCode::Down => {
            if let Some(p) = app.borrow_mut().palette_mut() {
                let len = palette::filtered(&p.query).len();
                if len > 0 {
                    p.selected = (p.selected + 1).min(len - 1);
                }
            }
        }
        KeyCode::Backspace => {
            if let Some(p) = app.borrow_mut().palette_mut() {
                p.query.pop();
                p.selected = 0;
            }
        }
        KeyCode::Char(c) if !is_shortcut_char(&key) => {
            if let Some(p) = app.borrow_mut().palette_mut() {
                p.query.push(c);
                p.selected = 0;
            }
        }
        _ => {}
    }
}

/// Drive the open param form: `↑/↓` move between fields, typing edits the
/// focused one (Space/`t`/`f` on a `:bool`), `Backspace` deletes, `Tab` accepts
/// the top suggestion (the dedicated completion key — chosen over Right-at-end
/// because crossterm delivers a clean `KeyCode::Tab` here at the app level,
/// leaving the arrows purely for field/cursor movement), `Enter` submits
/// (validate → launch), `Esc` closes without running. The form widget's help
/// line documents the same keys.
fn form_key(app: &Rc<RefCell<App>>, key: KeyEvent) {
    let mut a = app.borrow_mut();
    match key.code {
        KeyCode::Esc => a.cancel_form(),
        KeyCode::Enter => a.submit_form(),
        KeyCode::Up => {
            if let Some(f) = a.form_mut() {
                f.focus_prev();
            }
        }
        KeyCode::Down => {
            if let Some(f) = a.form_mut() {
                f.focus_next();
            }
        }
        KeyCode::Left => {
            if let Some(f) = a.form_mut() {
                f.cursor_left();
            }
        }
        KeyCode::Right => {
            if let Some(f) = a.form_mut() {
                f.cursor_right();
            }
        }
        KeyCode::Home => {
            if let Some(f) = a.form_mut() {
                f.cursor_home();
            }
        }
        KeyCode::End => {
            if let Some(f) = a.form_mut() {
                f.cursor_end();
            }
        }
        KeyCode::Tab => {
            if let Some(f) = a.form_mut() {
                f.accept_suggestion();
            }
        }
        KeyCode::Backspace => {
            if let Some(f) = a.form_mut() {
                f.backspace();
            }
        }
        KeyCode::Char(c) if !is_shortcut_char(&key) => {
            if let Some(f) = a.form_mut() {
                f.input(c);
            }
        }
        _ => {}
    }
}

fn forward(runtime: &mut HypertileRuntime, key: KeyEvent) {
    if let Some(ev) = hypertile_event_from_crossterm(Event::Key(key)) {
        runtime.handle_event(ev);
    }
}
