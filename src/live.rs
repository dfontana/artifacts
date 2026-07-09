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
use crate::data::{BankData, MonsterData, NpcItemData, RecipeData, ResourceData};
use crate::driver::Driver;
use crate::lua::{require_module, setup_lua, LuaSetupOptions};
use crate::planner::{build_state, PlanSeed};
use crate::progress::ProgressLog;
use crate::scheduler::Scheduler;
use crate::workflow;

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
///
/// A nil `build` is a loud error here — single-shot `run` treats "nothing to
/// do" as a mistake, not a fixpoint (`plans/DYNAMIC_WORKFLOWS.md` §2.1). This
/// is a thin wrapper over [`run_workflow_or_done`], which the M7 campaign loop
/// (`src/campaign.rs`) calls directly to treat a nil build as its stop
/// condition instead — same wiring, different nil handling, one place the
/// scheduler/Lua setup lives.
// The reference-data + params + options inputs are each load-bearing and travel
// as data (the workflow is evaluated in this fn's own Lua state), so the arg
// count is the point — same rationale as `setup_lua`/`spawn_tui_run`.
#[allow(clippy::too_many_arguments)]
pub fn run_workflow(
    driver: Box<dyn Driver>,
    workflow_src: &str,
    shared_view: SharedView,
    map: Option<Arc<GameMap>>,
    monsters: Option<Arc<MonsterData>>,
    resources: Option<Arc<ResourceData>>,
    recipes: Option<Arc<RecipeData>>,
    npc_items: Option<Arc<NpcItemData>>,
    bank: Option<Arc<BankData>>,
    params: &[(String, String)],
    options: RunOptions,
) -> Result<CharacterView> {
    run_workflow_or_done(
        driver,
        workflow_src,
        shared_view,
        map,
        monsters,
        resources,
        recipes,
        npc_items,
        bank,
        params,
        options,
    )?
    .ok_or_else(|| {
        anyhow!("workflow built to nothing (single-shot run treats a nil build as a mistake)")
    })
}

/// The shared implementation behind [`run_workflow`] and the campaign loop:
/// evaluate the workflow once, plan-gate + run it if `build` produced an AST,
/// and return `Ok(None)` iff `build` returned nil (the M7 done-sentinel) —
/// letting a caller that loops (`campaign::run_until_done`) tell "nothing left
/// to do" apart from every other error without duplicating the scheduler/
/// `setup_lua`/`workflow::load` wiring.
// Same arg-count rationale as `run_workflow`, which this backs.
#[allow(clippy::too_many_arguments)]
pub fn run_workflow_or_done(
    driver: Box<dyn Driver>,
    workflow_src: &str,
    shared_view: SharedView,
    map: Option<Arc<GameMap>>,
    monsters: Option<Arc<MonsterData>>,
    resources: Option<Arc<ResourceData>>,
    recipes: Option<Arc<RecipeData>>,
    npc_items: Option<Arc<NpcItemData>>,
    bank: Option<Arc<BankData>>,
    params: &[(String, String)],
    options: RunOptions,
) -> Result<Option<CharacterView>> {
    let seed = PlanSeed::from_view(&shared_view.get());
    let origin = (seed.x, seed.y);
    let (character, scheduler_handle) = spawn_scheduler(driver, shared_view.clone(), options.abort);

    let result = (|| -> Result<bool> {
        let lua = setup_lua(LuaSetupOptions {
            character: Some(character),
            map,
            monsters,
            resources,
            recipes,
            npc_items,
            bank: bank.clone(),
            origin: Some(origin),
            progress: options.progress,
            workflows_root: None,
        })
        .map_err(|e| anyhow!("setup_lua: {e}"))?;
        // The read-only `ctx` for `build`, built from the live seed state through
        // the same helper the plan pass uses (the anti-drift argument, extended
        // to `build`).
        let ctx = build_state(&lua, &seed).map_err(|e| anyhow!("build ctx: {e}"))?;
        // Layer `ctx.bank` onto it, same helper and same "harmless extra key"
        // reasoning as `planner::plan` (`workflow::attach_bank`'s doc) — §5.6.
        workflow::attach_bank(&lua, &ctx, bank.as_deref())
            .map_err(|e| anyhow!("attach bank: {e}"))?;
        let Some(wf) = workflow::load(&lua, workflow_src, "workflow.fnl", params, ctx)? else {
            // The done-sentinel: build returned nil. Nothing to plan-gate or run.
            return Ok(false);
        };
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
        Ok(true)
    })();

    // Ensure the scheduler thread is joined even if the run failed.
    let _ = scheduler_handle.join();

    if result? {
        Ok(Some((*shared_view.get()).clone()))
    } else {
        Ok(None)
    }
}
