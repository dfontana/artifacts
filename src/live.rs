//! Live execution helper: wire a `Driver` to the scheduler + `Character` and run
//! a workflow's `run` pass against the real game. Keeps `mlua` and the threading
//! bridge encapsulated so callers (the CLI) stay thin.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::Result;
use artifacts_core::map::GameMap;
use artifacts_core::step::CharacterView;
use mlua::prelude::*;
use tokio::sync::mpsc;

use crate::character::Character;
use crate::data::MonsterData;
use crate::driver::Driver;
use crate::lua::{eval_fennel, require_module, setup_lua, LuaSetupOptions};
use crate::scheduler::Scheduler;
use crate::view::SharedView;

/// Spin up a scheduler thread for `driver`, returning the `Character` handle that
/// feeds it and the scheduler's join handle. Shared by the CLI run, the TUI run
/// worker, and the integration tests, so the channel + thread wiring lives in
/// exactly one place. Dropping the `Character` (and every clone) closes the
/// channel and ends the scheduler.
pub fn spawn_scheduler(
    driver: Box<dyn Driver>,
    view: SharedView,
    abort: Arc<AtomicBool>,
) -> (Character, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel(32);
    let scheduler = Scheduler::new(driver, rx, view.clone(), abort);
    // The scheduler blocks internally; run it on a dedicated thread.
    let handle = std::thread::spawn(move || scheduler.run());
    (Character::new(tx, view), handle)
}

/// Run a workflow's `run` pass against a live driver.
///
/// `initial_view` seeds the synchronously-readable `CharacterView` (fetch it from
/// the server first). `map` powers `host.path_hops`. Returns the final view.
pub fn run_workflow(
    driver: Box<dyn Driver>,
    workflow_src: &str,
    initial_view: CharacterView,
    map: Option<Arc<GameMap>>,
    monsters: Option<Arc<MonsterData>>,
) -> Result<CharacterView> {
    let origin = (initial_view.x, initial_view.y);
    let shared_view = SharedView::new(initial_view);
    // The CLI run never cancels; hand the scheduler a flag that is never set.
    let abort = Arc::new(AtomicBool::new(false));
    let (character, scheduler_handle) = spawn_scheduler(driver, shared_view.clone(), abort);

    let result = (|| -> Result<()> {
        let lua = setup_lua(LuaSetupOptions {
            character: Some(character),
            map,
            monsters,
            origin: Some(origin),
            ..Default::default()
        })
        .map_err(|e| anyhow::anyhow!("setup_lua: {e}"))?;
        let wf = eval_fennel(&lua, workflow_src, "workflow.fnl")
            .map_err(|e| anyhow::anyhow!("load workflow: {e}"))?;
        // `run` is an export of the fennel.lib.interp module, not a global —
        // fetch it from the module like the TUI run worker and planner do.
        let interp = require_module(&lua, "fennel.lib.interp")
            .map_err(|e| anyhow::anyhow!("require interp: {e}"))?;
        let run_fn: LuaFunction = interp.get("run").map_err(|e| anyhow::anyhow!("{e}"))?;
        run_fn
            .call::<()>(wf)
            .map_err(|e| anyhow::anyhow!("run pass: {e}"))?;
        // `lua` drops here, dropping the Character → closing the scheduler channel.
        Ok(())
    })();

    // Ensure the scheduler thread is joined even if the run failed.
    let _ = scheduler_handle.join();

    result?;
    Ok((*shared_view.get()).clone())
}
