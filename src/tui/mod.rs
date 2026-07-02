//! Terminal UI: a live single-character dashboard that plans and runs workflows
//! with a truthful per-step progress cursor. Launched as `artifacts tui <name>`.
//!
//! See `plans/TUI.md` for the full design. The load-bearing pieces are the pure
//! [`reducer`] (turns an append-only id-log into per-row glyph states) and the
//! [`skeleton`] marshaling (one owned, `Send` `Vec<PlanStep>` handed off from the
//! Lua run worker to the UI thread).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use artifacts_core::map::GameMap;
use artifacts_core::step::CharacterView;
use crossterm::event::{Event, KeyEventKind};

use crate::data::MonsterData;
use crate::driver::http::HttpDriver;
use crate::view::SharedView;

pub mod app;
pub mod event;
pub mod glyphs;
pub mod reducer;
pub mod run_worker;
pub mod skeleton;
pub mod theme;
pub mod ui;
pub mod widgets;
pub mod workflows;

/// A workflow AST node's identity. Stamped by the Fennel `number-nodes` walk in
/// pre-order (visit order), it is the join key between the skeleton, the plan's
/// loop counts, and the run's progress log — see `plans/TUI.md` §3.1.
pub type NodeId = u64;

/// The append-only ordered log the `run` pass appends a `NodeId` to on entry to
/// every node (via `host.progress`). Named `ProgressLog` — not `Progress` — to
/// avoid confusion with `core::machine::Progress` (§9). A `Vec`, not a single
/// slot, so microsecond-apart fires (a when-skip immediately followed by its
/// sibling) are never lost between UI frames.
pub type ProgressLog = Arc<Mutex<Vec<NodeId>>>;

/// Build a fresh, empty progress log.
pub fn new_progress_log() -> ProgressLog {
    Arc::new(Mutex::new(Vec::new()))
}

/// Enter the alternate screen, run the TUI event loop until quit, and restore the
/// terminal. `poll_driver` is the driver `load_live_context` already built — kept
/// for the initial fetch + idle polls (§3.5). The render loop never blocks: it
/// polls input with a ~100 ms timeout and reads cheap shared cells each frame.
pub fn run(
    character: String,
    initial_view: CharacterView,
    map: Option<Arc<GameMap>>,
    monsters: Option<Arc<MonsterData>>,
    poll_driver: HttpDriver,
) -> Result<()> {
    let view = SharedView::new(initial_view);
    let mut app = app::App::new(character, view, map, monsters, poll_driver);

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    // Signal the idle-poll thread to exit (non-blocking; never joins, so a slow
    // in-flight fetch can't hang quit).
    app.shutdown();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut app::App) -> Result<()> {
    while !app.should_quit {
        terminal.draw(|f| ui::render(f, app))?;
        // Input poll with a timeout keeps the spinner/cooldown ticking and the
        // id-log draining even when the user is idle.
        if crossterm::event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = crossterm::event::read()? {
                // Filter key-release events (Windows emits both press and release).
                if key.kind == KeyEventKind::Press {
                    event::handle_key(app, key);
                }
            }
        }
        app.tick();
    }
    Ok(())
}
