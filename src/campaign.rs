//! The M7 campaign loop (`plans/DYNAMIC_WORKFLOWS.md` §8 M7): `artifacts run
//! <wf> <character> k=v... --until-done`. Long-horizon work (level a skill,
//! work through an achievement list, drain a bank backlog) is not one
//! workflow run — it's re-invoking `build` with a fresh live `ctx` after each
//! completed chunk until `build` returns nil (the done-sentinel,
//! `workflow.rs`'s doc). The loop lives here, at the harness layer, **above**
//! a single plan/run — it never reaches inside an AST (§6's "no mid-run
//! regeneration" invariant): each iteration builds one fixed AST, plan-gates
//! it, runs it to completion, and only then asks `build` again.
//!
//! This is deliberately thin: [`run_until_done`] composes exactly the same
//! pieces the single-shot CLI path does (`live::run_workflow_or_done`,
//! `planner::build_state`/`extract_plan`) — no new scheduler or Lua-state
//! wiring. The one build call per iteration is enforced by reusing
//! `run_workflow_or_done`'s `pre_run` hook (`live.rs`) to run the plan pass
//! **on the same already-built AST**, in the same Lua state, before the run
//! pass fires — so an iteration costs exactly one `build` call, not two.

use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use mlua::prelude::*;

use artifacts_core::map::GameMap;
use artifacts_core::step::CharacterView;

use crate::character::SharedView;
use crate::data::{BankData, MonsterData, NpcItemData, RecipeData, ResourceData};
use crate::driver::Driver;
use crate::live::{self, PreRunHook, RunOptions};
use crate::planner::{self, PlanSeed};

/// Reference data loaded ONCE before the loop starts (`DYNAMIC_WORKFLOWS` §8
/// M7: "reference data ... is loaded once; only live state ... refetches per
/// iteration"). Cheap to clone (every field is an `Arc`), so each iteration
/// clones its own copy to hand to `run_workflow_or_done`.
#[derive(Debug, Default, Clone)]
pub struct CampaignReferenceData {
    pub map: Option<Arc<GameMap>>,
    pub monsters: Option<Arc<MonsterData>>,
    pub resources: Option<Arc<ResourceData>>,
    pub recipes: Option<Arc<RecipeData>>,
    pub npc_items: Option<Arc<NpcItemData>>,
}

/// Run `workflow_src` to completion, re-invoking `build` with a freshly
/// fetched live snapshot after each successful run until `build` returns nil.
///
/// `fetch` is called once per iteration and must return a driver ready to run
/// the next chunk plus that driver's own fresh character/bank snapshot — the
/// CLI wires this to a fresh `HttpDriver` (`main.rs`); tests wire it to a
/// `MockDriver` and hand-crafted `CharacterView`/`BankData` per call, which is
/// how a fixture "acquires" state between iterations without diffing anything
/// itself (the loop never diffs live state — `DYNAMIC_WORKFLOWS` §8 M7's
/// contract note: a `build` that re-emits an identical chunk against
/// unchanged live state burns iterations doing nothing).
///
/// Stops (returns `Ok(())`) the moment an iteration's `build` returns nil.
/// Aborts loudly (`Err`, naming the blocker) the moment an iteration's plan
/// gate finds the fresh build infeasible — that iteration is never run.
/// Exhausts (`Err`, naming `--max-iterations`) if `build` never returns nil
/// within `max_iterations` iterations.
pub fn run_until_done(
    workflow_src: &str,
    reference: &CampaignReferenceData,
    params: &[(String, String)],
    max_iterations: u32,
    mut fetch: impl FnMut() -> Result<(Box<dyn Driver>, CharacterView, BankData)>,
) -> Result<()> {
    for iteration in 1..=max_iterations {
        println!("=== campaign iteration {iteration}/{max_iterations} ===");

        let (driver, view, bank) =
            fetch().map_err(|e| anyhow!("iteration {iteration}: refetching live state: {e}"))?;
        let seed = PlanSeed::from_view(&view);
        let bank = Arc::new(bank);

        // The plan-gate hook: runs `interp.plan` on the SAME Lua state/AST
        // `run_workflow_or_done` just built (one `build` call total this
        // iteration), from a state table seeded the same way the CLI's `plan`
        // subcommand seeds it. A blocker aborts by returning `Err` here,
        // which `run_workflow_or_done` propagates BEFORE calling `interp.run`
        // — so an infeasible iteration is never run (`DYNAMIC_WORKFLOWS` §8
        // M7: "a blocker aborts the loop loudly rather than running a
        // known-infeasible chunk").
        let seed_for_plan = seed;
        let pre_run: PreRunHook = Box::new(move |lua: &Lua, wf: &LuaValue, interp: &LuaTable| {
            let st = planner::build_state(lua, &seed_for_plan)
                .map_err(|e| anyhow!("seeding plan state: {e}"))?;
            let plan_fn: LuaFunction = interp.get("plan").map_err(|e| anyhow!("{e}"))?;
            let result: LuaTable = plan_fn
                .call((wf.clone(), st))
                .map_err(|e| anyhow!("plan pass: {e}"))?;
            let plan = planner::extract_plan(&result).map_err(|e| anyhow!("{e}"))?;

            println!(
                "  build: produced an AST ({} action(s) predicted, {:.0}s)",
                plan.actions, plan.seconds
            );
            if !plan.feasible {
                println!("  plan: BLOCKED");
                for b in &plan.blockers {
                    println!("    - {b}");
                }
                bail!("plan blocked: {}", plan.blockers.join("; "));
            }
            println!("  plan: feasible ({} action(s))", plan.actions);
            Ok(())
        });

        let outcome = live::run_workflow_or_done(
            driver,
            workflow_src,
            SharedView::new(view),
            reference.map.clone(),
            reference.monsters.clone(),
            reference.resources.clone(),
            reference.recipes.clone(),
            reference.npc_items.clone(),
            Some(bank),
            params,
            RunOptions {
                pre_run: Some(pre_run),
                ..Default::default()
            },
        );

        match outcome {
            Ok(None) => {
                println!("  build: returned nil — campaign done after {iteration} iteration(s)");
                return Ok(());
            }
            Ok(Some(final_view)) => {
                println!(
                    "  run: ok — '{}' now at ({}, {}), hp {}/{}, inventory {}/{}",
                    final_view.name,
                    final_view.x,
                    final_view.y,
                    final_view.hp,
                    final_view.max_hp,
                    final_view.inventory_count(),
                    final_view.inventory_max_items
                );
            }
            Err(e) => {
                return Err(anyhow!("campaign aborted at iteration {iteration}: {e:#}"));
            }
        }
    }

    Err(anyhow!(
        "campaign exhausted --max-iterations {max_iterations} without build returning nil \
         (a build that never converges to nil will loop forever without this guard)"
    ))
}
