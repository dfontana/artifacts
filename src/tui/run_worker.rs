//! The TUI's combined `plan`-for-skeleton + `run` worker (`plans/TUI.md` §3.1).
//! Kept in the TUI layer (not `live.rs`) so the live bridge stays agnostic of the
//! run-panel types (`RunSession`/`RunStatus`/`PlanStep`) — the dependency runs
//! `tui → live`, never back.

use std::sync::atomic::Ordering;
use std::thread::JoinHandle;

use anyhow::Result;
use mlua::prelude::*;

use crate::context::ExecutionContext;
use crate::driver::http::HttpDriver;
use crate::live::{run_workflow, ExecutionMode, RunOptions};
use crate::tui::app::{RunSession, RunStatus};
use crate::tui::skeleton::{join_loop_counts, marshal, read_loop_counts};

/// Spawn the TUI's combined run on a fresh worker thread and return its handle.
///
/// Builds a **fresh** `HttpDriver::from_env` (§3.5) so the run never contends
/// with the TUI's idle poll driver, then runs the combined path (§3.1) on the
/// worker: one Lua state, evaluated once, then `number-nodes` → `skeleton` →
/// `plan(seed)` → publish the skeleton → `run`. Errors here are construction
/// failures (e.g. no token); run failures ride in `session.status`.
pub fn spawn_tui_run(
    character: &str,
    workflow_src: String,
    context: ExecutionContext,
    params: Vec<(String, String)>,
    session: RunSession,
    force: bool,
) -> Result<JoinHandle<Result<()>>> {
    let run_driver = HttpDriver::from_env(character)
        .map_err(|e| anyhow::anyhow!("constructing run HttpDriver: {e}"))?;
    Ok(std::thread::spawn(move || {
        tui_run_worker(
            Box::new(run_driver),
            workflow_src,
            context,
            params,
            session,
            force,
        )
    }))
}

/// The combined `plan`-for-skeleton + `run` path (§3.1), built on the shared
/// `live::run_workflow` (driver/scheduler/setup_lua/eval_fennel wiring lives
/// there now). The TUI's own contribution is the `pre_run` hook below: it
/// marshals the skeleton to an owned `Vec<PlanStep>` and publishes it **before**
/// the blocking `run`, on the SAME Lua state/AST `run_workflow` evaluated once
/// — so ids align by identity across number-nodes/skeleton/plan/run.
fn tui_run_worker(
    driver: Box<dyn crate::driver::Driver>,
    workflow_src: String,
    context: ExecutionContext,
    params: Vec<(String, String)>,
    session: RunSession,
    force: bool,
) -> Result<()> {
    let abort = session.abort.clone();
    let progress = session.progress.clone();
    let skeleton_slot = session.skeleton.clone();

    // The interp entry points are exports of the `fennel.lib.interp` module
    // (seeded into package.loaded by setup_lua), not globals; pull each fn off
    // the table `run_workflow` hands us. One helper keeps the call sites to a
    // line.
    let pre_run = Box::new(
        move |_lua: &Lua,
              wf: &LuaValue,
              interp: &LuaTable,
              plan_result: &LuaTable,
              _plan: &crate::planner::PlanResult|
              -> Result<()> {
            let interp_fn = |name: &str| -> Result<LuaFunction> {
                interp.get(name).map_err(|e| anyhow::anyhow!("{e}"))
            };

            // 1. skeleton (flat structural walk) → owned Vec<PlanStep>. Node ids
            // were stamped by the shared live entrypoint before its plan walk.
            let sk_tbl: LuaTable = interp_fn("skeleton")?
                .call(wf)
                .map_err(|e| anyhow::anyhow!("skeleton: {e}"))?;
            let mut skeleton = marshal(&sk_tbl).map_err(|e| anyhow::anyhow!("marshal: {e}"))?;

            // 2. Join loop counts from the mandatory plan already run by the
            //    shared live entrypoint; never perform a second partial plan.
            let counts = read_loop_counts(plan_result).map_err(|e| anyhow::anyhow!("{e}"))?;
            join_loop_counts(&mut skeleton, &counts);

            // 3. Publish the skeleton BEFORE the blocking run — the run panel renders
            //    `preparing run…` until this lands, then switches to the live rows.
            let _ = skeleton_slot.set(skeleton);
            Ok(())
        },
    );

    let result = run_workflow(
        driver,
        &workflow_src,
        session.view.clone(),
        &context,
        &params,
        RunOptions {
            abort: abort.clone(),
            progress: Some(progress),
            mode: if force {
                ExecutionMode::Force
            } else {
                ExecutionMode::Gated
            },
            pre_run: Some(pre_run),
        },
    );

    // Publish the terminal status. A cancel (abort set) unwinds the run with an
    // error too, but that is an intentional stop, not a failure — settle to Done
    // so no pop-over fires.
    let aborted = abort.load(Ordering::SeqCst);
    let mut status = session.status.lock().unwrap();
    *status = match &result {
        Ok(_) => RunStatus::Done,
        Err(_) if aborted => RunStatus::Done,
        Err(e) => RunStatus::Failed(format!("{e:#}")),
    };
    drop(status);

    if aborted {
        return Ok(());
    }
    result.map(|_| ())
}
