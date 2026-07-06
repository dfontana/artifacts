//! The command palette (`p`): an actionable, fuzzy-filterable list of the
//! dashboard's global commands. Opening/driving it lives in [`crate::tui::event`]
//! and rendering in [`crate::tui::ui`]; this module owns the command set and how
//! each one is applied. Commands touch both the `App` (run/plan/zoom/quit) and
//! the tiling runtime (focus), so they run at the event-loop level where both
//! are in hand — not inside a pane plugin.

use ratatui_hypertile_extras::HypertileRuntime;

use crate::tui::app::{App, Pane};
use crate::tui::plugins::Panes;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Run,
    RunOverride,
    Replan,
    ZoomToggle,
    Focus(Pane),
    Quit,
}

impl Command {
    pub fn label(self) -> &'static str {
        match self {
            Command::Run => "run",
            Command::RunOverride => "run (override infeasible)",
            Command::Replan => "re-plan",
            Command::ZoomToggle => "zoom focused pane",
            Command::Focus(Pane::Stats) => "focus: stats",
            Command::Focus(Pane::Inventory) => "focus: inventory",
            Command::Focus(Pane::Workflows) => "focus: workflows",
            Command::Focus(Pane::Run) => "focus: run",
            Command::Quit => "quit",
        }
    }

    /// Every command, in the order shown before any filtering.
    pub fn all() -> [Command; 9] {
        [
            Command::Run,
            Command::RunOverride,
            Command::Replan,
            Command::ZoomToggle,
            Command::Focus(Pane::Stats),
            Command::Focus(Pane::Inventory),
            Command::Focus(Pane::Workflows),
            Command::Focus(Pane::Run),
            Command::Quit,
        ]
    }
}

/// The commands whose label contains `query` (case-insensitive substring).
pub fn filtered(query: &str) -> Vec<Command> {
    let q = query.to_lowercase();
    Command::all()
        .into_iter()
        .filter(|c| c.label().to_lowercase().contains(&q))
        .collect()
}

/// Apply a chosen command. Focus goes through the runtime; everything else is an
/// `App` transition. None of these re-enter a pane plugin, so it is safe to hold
/// the `App` borrow while calling `runtime.focus_pane`.
pub fn execute(cmd: Command, app: &mut App, runtime: &mut HypertileRuntime, panes: &Panes) {
    match cmd {
        Command::Run => app.launch_run(false),
        Command::RunOverride => app.launch_run(true),
        Command::Replan => app.force_refresh_plan(),
        Command::ZoomToggle => app.zoom = !app.zoom,
        Command::Focus(pane) => {
            let _ = runtime.focus_pane(panes.id(pane));
        }
        Command::Quit => app.should_quit = true,
    }
}
