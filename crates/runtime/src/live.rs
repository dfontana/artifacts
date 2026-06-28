//! Live execution helper: wire a `Driver` to the scheduler + `Character` and run
//! a workflow's `run` pass against the real game. Keeps `mlua` and the threading
//! bridge encapsulated so callers (the CLI) stay thin.

use std::sync::Arc;

use anyhow::Result;
use artifacts_core::map::GameMap;
use artifacts_core::step::CharacterView;
use artifacts_driver::Driver;
use mlua::prelude::*;
use tokio::sync::mpsc;

use crate::character::Character;
use crate::lua::{eval_fennel, setup_lua};
use crate::scheduler::Scheduler;
use crate::view::SharedView;

/// Run a workflow's `run` pass against a live driver.
///
/// `initial_view` seeds the synchronously-readable `CharacterView` (fetch it from
/// the server first). `map` powers `host.path_hops`. Returns the final view.
pub fn run_workflow(
    driver: Box<dyn Driver>,
    workflow_src: &str,
    initial_view: CharacterView,
    map: Option<Arc<GameMap>>,
) -> Result<CharacterView> {
    let shared_view = SharedView::new(initial_view);
    let (tx, rx) = mpsc::channel(32);
    let scheduler = Scheduler::new(driver, rx, shared_view.clone());

    // The scheduler blocks internally; run it on a dedicated thread.
    let scheduler_handle = std::thread::spawn(move || scheduler.run());

    let character = Character::new(tx, shared_view.clone());

    let result = (|| -> Result<()> {
        let lua = setup_lua(Some(character), map).map_err(|e| anyhow::anyhow!("setup_lua: {e}"))?;
        let wf = eval_fennel(&lua, workflow_src, "workflow.fnl")
            .map_err(|e| anyhow::anyhow!("load workflow: {e}"))?;
        let run_fn: LuaFunction = lua
            .globals()
            .get("run")
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        run_fn
            .call::<()>(wf)
            .map_err(|e| anyhow::anyhow!("run pass: {e}"))?;
        // `lua` drops here, dropping the Character → closing the scheduler channel.
        Ok(())
    })();

    // Ensure the scheduler thread is joined even if the run failed.
    let _ = scheduler_handle.join();

    result?;
    Ok(shared_view.get())
}
