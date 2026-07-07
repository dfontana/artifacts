//! Live execution helper: wire a `Driver` to the scheduler + `Character` and run
//! a workflow's `run` pass against the real game. Keeps `mlua` and the threading
//! bridge encapsulated so callers (the CLI and the TUI) stay thin — the TUI's
//! `RunOptions::pre_run` hook is the seam that lets it splice in the
//! plan-for-skeleton walk without re-implementing the driver/scheduler/setup_lua
//! wiring (`plans/TUI.md` §3.1).

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::{anyhow, Result};
use artifacts_core::map::GameMap;
use artifacts_core::step::CharacterView;
use mlua::prelude::*;
use tokio::sync::mpsc;

use crate::character::{Character, SharedView};
use crate::data::{MonsterData, ResourceData};
use crate::driver::Driver;
use crate::lua::{eval_fennel, require_module, setup_lua, LuaSetupOptions};
use crate::progress::ProgressLog;
use crate::scheduler::Scheduler;

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

/// A hook spliced between loading the workflow and firing its `run` pass —
/// the TUI uses this to run `number-nodes`/`skeleton`/`plan` on the same Lua
/// state/AST and publish the skeleton before the blocking `run` call, without
/// `run_workflow` itself knowing anything about run panels or skeletons.
pub type PreRunHook<'a> = Box<dyn FnOnce(&Lua, &LuaValue, &LuaTable) -> Result<()> + 'a>;

/// Knobs `run_workflow` needs beyond "run this workflow": a cancel flag, a
/// progress sink, and an optional pre-run hook. Defaults match the plain CLI
/// `run` — uncancelable, no progress log, no hook — so passing extra fields is
/// the TUI's opt-in, not overhead the CLI path pays for.
#[derive(Default)]
pub struct RunOptions<'a> {
    /// Checked by the scheduler; set this to stop a run early. The CLI's
    /// default (`AtomicBool::new(false)`) is never set, so its run cannot be
    /// cancelled.
    pub abort: Arc<AtomicBool>,
    /// When `Some`, `host.progress` appends each node id `run-node` enters.
    pub progress: Option<ProgressLog>,
    pub pre_run: Option<PreRunHook<'a>>,
}

/// Run a workflow's `run` pass against a live driver.
///
/// `shared_view` seeds the synchronously-readable `CharacterView` (fetch it
/// from the server first) and is also where callers can keep reading live
/// position updates while the run is in flight (the TUI passes its
/// `RunSession::view` here instead of a fresh one). `map` powers
/// `host.path_hops`. Returns the final view.
pub fn run_workflow(
    driver: Box<dyn Driver>,
    workflow_src: &str,
    shared_view: SharedView,
    map: Option<Arc<GameMap>>,
    monsters: Option<Arc<MonsterData>>,
    resources: Option<Arc<ResourceData>>,
    options: RunOptions,
) -> Result<CharacterView> {
    let origin = {
        let v = shared_view.get();
        (v.x, v.y)
    };
    let (character, scheduler_handle) = spawn_scheduler(driver, shared_view.clone(), options.abort);

    let result = (|| -> Result<()> {
        let lua = setup_lua(LuaSetupOptions {
            character: Some(character),
            map,
            monsters,
            resources,
            origin: Some(origin),
            progress: options.progress,
        })
        .map_err(|e| anyhow!("setup_lua: {e}"))?;
        let wf = eval_fennel(&lua, workflow_src, "workflow.fnl")
            .map_err(|e| anyhow!("load workflow: {e}"))?;
        let interp = require_module(&lua, "fennel.lib.interp")
            .map_err(|e| anyhow!("require interp: {e}"))?;

        if let Some(pre_run) = options.pre_run {
            pre_run(&lua, &wf, &interp).map_err(|e| anyhow!("pre-run: {e}"))?;
        }

        let run_fn: LuaFunction = interp.get("run").map_err(|e| anyhow!("{e}"))?;
        run_fn
            .call::<()>(wf)
            .map_err(|e| anyhow!("run pass: {e}"))?;
        // `lua` drops here, dropping the Character → closing the scheduler channel.
        Ok(())
    })();

    // Ensure the scheduler thread is joined even if the run failed.
    let _ = scheduler_handle.join();

    result?;
    Ok((*shared_view.get()).clone())
}
