//! The TUI's combined `plan`-for-skeleton + `run` worker (`plans/TUI.md` §3.1).
//! Kept in the TUI layer (not `live.rs`) so the live bridge stays agnostic of the
//! run-panel types (`RunSession`/`RunStatus`/`PlanStep`) — the dependency runs
//! `tui → live`, never back.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::Result;
use artifacts_core::map::GameMap;
use artifacts_core::step::CharacterView;
use mlua::prelude::*;

use crate::data::MonsterData;
use crate::driver::http::HttpDriver;
use crate::live::spawn_scheduler;
use crate::lua::{eval_fennel, setup_lua};
use crate::planner::{self, PlanSeed};
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
    initial_view: CharacterView,
    map: Option<Arc<GameMap>>,
    monsters: Option<Arc<MonsterData>>,
    session: RunSession,
) -> Result<JoinHandle<Result<()>>> {
    let run_driver = HttpDriver::from_env(character)
        .map_err(|e| anyhow::anyhow!("constructing run HttpDriver: {e}"))?;
    Ok(std::thread::spawn(move || {
        tui_run_worker(
            Box::new(run_driver),
            workflow_src,
            initial_view,
            map,
            monsters,
            session,
        )
    }))
}

/// The combined `plan`-for-skeleton + `run` path (§3.1). Runs entirely on one
/// worker thread because `mlua`'s `Lua` is `!Send`; it marshals the skeleton to
/// an owned `Vec<PlanStep>` and publishes it **before** the blocking `run`.
fn tui_run_worker(
    driver: Box<dyn crate::driver::Driver>,
    workflow_src: String,
    initial_view: CharacterView,
    map: Option<Arc<GameMap>>,
    monsters: Option<Arc<MonsterData>>,
    session: RunSession,
) -> Result<()> {
    let (character, scheduler_handle) =
        spawn_scheduler(driver, session.view.clone(), session.abort.clone());

    let result = (|| -> Result<()> {
        // One character-equipped state, with the progress log wired in.
        let lua = setup_lua(
            Some(character),
            map.clone(),
            monsters.clone(),
            Some(session.progress.clone()),
        )
        .map_err(|e| anyhow::anyhow!("setup_lua: {e}"))?;

        // Evaluate the workflow ONCE → the single AST the next four walks read,
        // so the ids align by identity (§3.1).
        let wf = eval_fennel(&lua, &workflow_src, "workflow.fnl")
            .map_err(|e| anyhow::anyhow!("load workflow: {e}"))?;

        // The interp entry points are all Lua globals fetched by name the same
        // way; one helper keeps the four call sites to a single line each.
        let global_fn = |name: &str| -> Result<LuaFunction> {
            lua.globals().get(name).map_err(|e| anyhow::anyhow!("{e}"))
        };

        // 1. number-nodes (pre-order id stamp) on the shared table.
        global_fn("number_nodes")?
            .call::<LuaValue>(&wf)
            .map_err(|e| anyhow::anyhow!("number-nodes: {e}"))?;

        // 2. skeleton (flat structural walk) → owned Vec<PlanStep>.
        let sk_tbl: LuaTable = global_fn("skeleton")?
            .call(&wf)
            .map_err(|e| anyhow::anyhow!("skeleton: {e}"))?;
        let mut skeleton = marshal(&sk_tbl).map_err(|e| anyhow::anyhow!("marshal: {e}"))?;

        // 3. plan(seed) on the SAME shared state (only pure host fns), resolving
        //    loop counts and feasibility; join the id-keyed counts (§3.2).
        let seed = PlanSeed::from_view(&initial_view);
        let st = planner::build_state(&lua, &seed).map_err(|e| anyhow::anyhow!("seed: {e}"))?;
        let plan_result: LuaTable = global_fn("plan")?
            .call((&wf, st))
            .map_err(|e| anyhow::anyhow!("plan pass: {e}"))?;
        let counts = read_loop_counts(&plan_result).map_err(|e| anyhow::anyhow!("{e}"))?;
        join_loop_counts(&mut skeleton, &counts);

        // 4. Publish the skeleton BEFORE the blocking run — the run panel renders
        //    `preparing run…` until this lands, then switches to the live rows.
        let _ = session.skeleton.set(skeleton);

        // 5. run — fires host.progress(node.id) per node into session.progress.
        global_fn("run")?
            .call::<()>(&wf)
            .map_err(|e| anyhow::anyhow!("run pass: {e}"))?;
        Ok(())
    })();

    let _ = scheduler_handle.join();

    // Publish the terminal status. A cancel (abort set) unwinds the run with an
    // error too, but that is an intentional stop, not a failure — settle to Done
    // so no pop-over fires.
    let aborted = session.abort.load(Ordering::SeqCst);
    let mut status = session.status.lock().unwrap();
    *status = match &result {
        Ok(()) => RunStatus::Done,
        Err(_) if aborted => RunStatus::Done,
        Err(e) => RunStatus::Failed(format!("{e:#}")),
    };
    drop(status);

    if aborted {
        return Ok(());
    }
    result
}
