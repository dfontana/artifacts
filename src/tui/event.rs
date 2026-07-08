//! Input routing. The tiling runtime has two input modes: `Layout` (keys drive
//! focus / rearrange / resize) and `PluginInput` (keys go to the focused pane).
//! We keep the layout fixed by dropping the runtime's split/close keys and its
//! vim focus keys; `z`/`q` stay app-level (zoom + quit) and `p` opens the
//! command palette.
//!
//! Key point: forwarding a key into the runtime can re-enter the shared `App`
//! (a focused plugin's `on_event` borrows it), so we only ever borrow the `App`
//! for the brief app-level mutations here — never while forwarding.

use std::cell::RefCell;
use std::rc::Rc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui_hypertile_extras::{hypertile_event_from_crossterm, HypertileRuntime, InputMode};

use crate::tui::app::{App, Palette};
use crate::tui::palette;
use crate::tui::plugins::Panes;

pub fn handle_key(
    app: &Rc<RefCell<App>>,
    runtime: &mut HypertileRuntime,
    panes: &Panes,
    key: KeyEvent,
) {
    // A blocking failure pop-over swallows all input but its dismissal (§5.1).
    {
        let mut a = app.borrow_mut();
        if a.error_popover.is_some() {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                a.error_popover = None;
            }
            return;
        }
    }

    // The param form (M6), when open, captures everything until submit/cancel.
    if app.borrow().form.is_some() {
        form_key(app, key);
        return;
    }

    // The command palette, when open, captures everything until it closes.
    if app.borrow().palette.is_some() {
        palette_key(app, runtime, panes, key);
        return;
    }

    // Ctrl-C always quits, whatever the mode.
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.borrow_mut().should_quit = true;
        return;
    }

    // Interacting with a pane: forward everything to the focused plugin (which
    // borrows the `App` itself). The runtime turns Esc back into Layout mode.
    if runtime.mode() == InputMode::PluginInput {
        forward(runtime, key);
        return;
    }

    // Layout mode.
    match key.code {
        KeyCode::Char('q') => app.borrow_mut().should_quit = true,
        KeyCode::Char('p') => app.borrow_mut().palette = Some(Palette::default()),
        KeyCode::Char('z') => {
            let mut a = app.borrow_mut();
            a.zoom = !a.zoom;
        }
        KeyCode::Esc => app.borrow_mut().zoom = false,
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
        KeyCode::Esc => app.borrow_mut().palette = None,
        KeyCode::Enter => {
            let mut a = app.borrow_mut();
            let cmd = a.palette.as_ref().and_then(|p| {
                let items = palette::filtered(&p.query);
                items
                    .get(p.selected.min(items.len().saturating_sub(1)))
                    .copied()
            });
            a.palette = None;
            if let Some(cmd) = cmd {
                palette::execute(cmd, &mut a, runtime, panes);
            }
        }
        KeyCode::Up => {
            if let Some(p) = &mut app.borrow_mut().palette {
                p.selected = p.selected.saturating_sub(1);
            }
        }
        KeyCode::Down => {
            let mut a = app.borrow_mut();
            let len = a
                .palette
                .as_ref()
                .map(|p| palette::filtered(&p.query).len())
                .unwrap_or(0);
            if let Some(p) = &mut a.palette {
                if len > 0 {
                    p.selected = (p.selected + 1).min(len - 1);
                }
            }
        }
        KeyCode::Backspace => {
            if let Some(p) = &mut app.borrow_mut().palette {
                p.query.pop();
                p.selected = 0;
            }
        }
        KeyCode::Char(c) => {
            if let Some(p) = &mut app.borrow_mut().palette {
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
            if let Some(f) = &mut a.form {
                f.focus_prev();
            }
        }
        KeyCode::Down => {
            if let Some(f) = &mut a.form {
                f.focus_next();
            }
        }
        KeyCode::Tab => {
            if let Some(f) = &mut a.form {
                f.accept_suggestion();
            }
        }
        KeyCode::Backspace => {
            if let Some(f) = &mut a.form {
                f.backspace();
            }
        }
        KeyCode::Char(c) => {
            if let Some(f) = &mut a.form {
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
