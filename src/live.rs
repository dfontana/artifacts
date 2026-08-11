//! Live execution helper: wire a `Driver` to the scheduler + `Character` and run
//! a workflow's `run` pass against the real game. Keeps `mlua` and the threading
//! bridge encapsulated so callers (the CLI and the TUI) stay thin — the TUI's
//! The build → plan → feasibility gate → run sequence is enforced here for
//! every caller. `RunOptions::pre_run` is only an observer seam for TUI skeleton
//! publication and campaign reporting (`plans/TUI.md` §3.1).

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::{anyhow, Result};
use artifacts_core::step::CharacterView;
use mlua::prelude::*;
use tokio::sync::mpsc;

use crate::character::{Character, SharedView};
use crate::context::ExecutionContext;
use crate::driver::Driver;
use crate::lua::{require_module, setup_lua, LuaSetupOptions};
use crate::planner::{build_state, extract_plan, PlanResult, PlanSeed};
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

/// An observer spliced after the mandatory plan gate and before `run` — the
/// TUI uses it to publish a numbered skeleton using the already-computed plan,
/// without `run_workflow` knowing anything about run-panel types.
pub type PreRunHook<'a> =
    Box<dyn FnOnce(&Lua, &LuaValue, &LuaTable, &LuaTable, &PlanResult) -> Result<()> + 'a>;

/// Whether the mandatory feasibility gate may be overridden. Safe execution is
/// the default; bypassing blockers requires choosing `Force` explicitly.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ExecutionMode {
    #[default]
    Gated,
    Force,
}

/// Knobs `run_workflow` needs beyond "run this workflow": a cancel flag, a
/// progress sink, an explicit execution mode, and an optional observer. Defaults
/// match the safe plain CLI run: uncancelable, gated, and no observer.
#[derive(Default)]
pub struct RunOptions<'a> {
    /// Checked by the scheduler; set this to stop a run early. The CLI's
    /// default (`AtomicBool::new(false)`) is never set, so its run cannot be
    /// cancelled.
    pub abort: Arc<AtomicBool>,
    /// When `Some`, `host.progress` appends each node id `run-node` enters.
    pub progress: Option<ProgressLog>,
    /// Explicit unsafe override for a known-infeasible plan.
    pub mode: ExecutionMode,
    /// Observer invoked after planning and gating, before any action is sent.
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
pub fn run_workflow(
    driver: Box<dyn Driver>,
    workflow_src: &str,
    shared_view: SharedView,
    context: &ExecutionContext,
    params: &[(String, String)],
    options: RunOptions,
) -> Result<CharacterView> {
    run_workflow_or_done(driver, workflow_src, shared_view, context, params, options)?.ok_or_else(
        || anyhow!("workflow built to nothing (single-shot run treats a nil build as a mistake)"),
    )
}

/// The shared implementation behind [`run_workflow`] and the campaign loop:
/// evaluate the workflow once, plan-gate + run it if `build` produced an AST,
/// and return `Ok(None)` iff `build` returned nil (the M7 done-sentinel) —
/// letting a caller that loops (`campaign::run_until_done`) tell "nothing left
/// to do" apart from every other error without duplicating the scheduler/
/// `setup_lua`/`workflow::load` wiring.
pub fn run_workflow_or_done(
    driver: Box<dyn Driver>,
    workflow_src: &str,
    shared_view: SharedView,
    context: &ExecutionContext,
    params: &[(String, String)],
    options: RunOptions,
) -> Result<Option<CharacterView>> {
    let seed = PlanSeed::from_view(&shared_view.get());
    let origin = (seed.x, seed.y);
    let (character, scheduler_handle) = spawn_scheduler(driver, shared_view.clone(), options.abort);

    let result = (|| -> Result<bool> {
        let lua = setup_lua(LuaSetupOptions {
            character: Some(character),
            map: context.map.clone(),
            monsters: context.monsters.clone(),
            resources: context.resources.clone(),
            recipes: context.recipes.clone(),
            npc_items: context.npc_items.clone(),
            bank: context.bank.clone(),
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
        workflow::attach_bank(&lua, &ctx, context.bank.as_deref())
            .map_err(|e| anyhow!("attach bank: {e}"))?;
        let Some(wf) = workflow::load(&lua, workflow_src, "workflow.fnl", params, ctx)? else {
            // The done-sentinel: build returned nil. Nothing to plan-gate or run.
            return Ok(false);
        };
        let interp = require_module(&lua, "fennel.lib.interp")
            .map_err(|e| anyhow!("require interp: {e}"))?;
        // Stamp stable node ids before planning. This is harmless for non-TUI
        // callers and lets observers join the plan's loop counts to a skeleton
        // without needing a second plan walk.
        let number_nodes: LuaFunction = interp.get("number_nodes").map_err(|e| anyhow!("{e}"))?;
        number_nodes
            .call::<LuaValue>(wf.clone())
            .map_err(|e| anyhow!("number nodes: {e}"))?;

        // Planning is an invariant of every execution path, not an optional
        // callback. It uses the same seed and bank-enriched state as `build`.
        let plan_state = build_state(&lua, &seed).map_err(|e| anyhow!("plan state: {e}"))?;
        workflow::attach_bank(&lua, &plan_state, context.bank.as_deref())
            .map_err(|e| anyhow!("attach plan bank: {e}"))?;
        let plan_fn: LuaFunction = interp.get("plan").map_err(|e| anyhow!("{e}"))?;
        let plan_table: LuaTable = plan_fn
            .call((wf.clone(), plan_state))
            .map_err(|e| anyhow!("plan pass: {e}"))?;
        let plan = extract_plan(&plan_table).map_err(|e| anyhow!("plan result: {e}"))?;
        if !plan.feasible && options.mode == ExecutionMode::Gated {
            return Err(anyhow!("plan blocked: {}", plan.blockers.join("; ")));
        }

        if let Some(pre_run) = options.pre_run {
            pre_run(&lua, &wf, &interp, &plan_table, &plan).map_err(|e| anyhow!("pre-run: {e}"))?;
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
