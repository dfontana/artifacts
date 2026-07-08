//! Terminal UI: a live single-character dashboard that plans and runs workflows
//! with a truthful per-step progress cursor. Launched as `artifacts tui <name>`.
//!
//! See `plans/TUI.md` for the full design. The load-bearing pieces are the pure
//! [`reducer`] (turns an append-only id-log into per-row glyph states) and the
//! [`skeleton`] marshaling (one owned, `Send` `Vec<PlanStep>` handed off from the
//! Lua run worker to the UI thread).

use std::cell::RefCell;
use std::io;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use artifacts_core::map::GameMap;
use artifacts_core::step::CharacterView;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use crossterm::execute;
use ratatui_hypertile_extras::HypertileRuntime;

use crate::character::SharedView;
use crate::data::{BankData, MonsterData, RecipeData, ResourceData};
use crate::driver::http::HttpDriver;

pub mod app;
pub mod event;
pub mod glyphs;
pub mod palette;
pub mod plugins;
pub mod reducer;
pub mod run_worker;
pub mod skeleton;
pub mod theme;
pub mod ui;
pub mod widgets;
pub mod workflows;

// The progress-log types live below the Lua bridge (`crate::progress`);
// re-exported here because the run panel is their main consumer.
pub use crate::progress::{new_progress_log, NodeId, ProgressLog};

/// Enter the alternate screen, run the TUI event loop until quit, and restore the
/// terminal. `poll_driver` is the driver `load_live_context` already built — kept
/// for the initial fetch + idle polls (§3.5). The render loop never blocks: it
/// polls input with a ~100 ms timeout and reads cheap shared cells each frame.
// The reference-data inputs are each load-bearing and travel individually
// (same rationale as `setup_lua`/`live::run_workflow`).
#[allow(clippy::too_many_arguments)]
pub fn run(
    character: String,
    initial_view: CharacterView,
    map: Option<Arc<GameMap>>,
    monsters: Option<Arc<MonsterData>>,
    resources: Option<Arc<ResourceData>>,
    recipes: Option<Arc<RecipeData>>,
    bank: Option<Arc<BankData>>,
    poll_driver: HttpDriver,
) -> Result<()> {
    let view = SharedView::new(initial_view);
    // The panes share one `App` behind `Rc<RefCell<..>>`: the tiling runtime
    // owns focus + geometry, the `App` owns everything else (§3.4).
    let app = Rc::new(RefCell::new(app::App::new(
        character,
        view,
        map,
        monsters,
        resources,
        recipes,
        bank,
        poll_driver,
    )));
    let (mut runtime, panes) = plugins::build_runtime(app.clone());

    let mut terminal = ratatui::init();
    // Mouse drives resize (drag a border) and rearrange (drag a pane onto
    // another); best-effort, so a terminal that refuses capture still works.
    let _ = execute!(io::stdout(), EnableMouseCapture);
    let result = event_loop(&mut terminal, &app, &mut runtime, &panes);
    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
    // Signal the idle-poll thread to exit (non-blocking; never joins, so a slow
    // in-flight fetch can't hang quit).
    app.borrow().shutdown();
    result
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &Rc<RefCell<app::App>>,
    runtime: &mut HypertileRuntime,
    panes: &plugins::Panes,
) -> Result<()> {
    loop {
        {
            let mut a = app.borrow_mut();
            if a.should_quit {
                break;
            }
            // Tick BEFORE drawing so the frame reads freshly derived state (the
            // run panel's reduced rows are computed here, not in the render path).
            a.tick();
        }
        {
            let a = app.borrow();
            terminal.draw(|f| ui::render(f, &a, runtime, panes))?;
        }
        // Input poll with a timeout keeps the spinner/cooldown ticking and the
        // id-log draining even when the user is idle.
        if crossterm::event::poll(Duration::from_millis(100))? {
            match crossterm::event::read()? {
                // Filter key-release events (Windows emits both press and release).
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    event::handle_key(app, runtime, panes, key);
                }
                Event::Mouse(mouse) => {
                    if let Some(ev) = plugins::hypertile_event_from_crossterm(Event::Mouse(mouse)) {
                        runtime.handle_event(ev);
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}
