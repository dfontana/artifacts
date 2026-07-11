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
//! This is deliberately thin: [`run_until_done`] delegates each chunk to the
//! same `live::run_workflow_or_done` build/plan/gate/run entrypoint as a
//! single-shot CLI run. Its hook only reports the already-computed plan, so an
//! iteration costs exactly one `build` and one `plan` call.

use std::sync::Arc;

use anyhow::{anyhow, Result};

use artifacts_core::step::CharacterView;

use crate::character::SharedView;
use crate::context::ExecutionContext;
use crate::data::BankData;
use crate::driver::Driver;
use crate::live::{self, PreRunHook, RunOptions};

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
    reference: &ExecutionContext,
    params: &[(String, String)],
    max_iterations: u32,
    mut fetch: impl FnMut() -> Result<(Box<dyn Driver>, CharacterView, BankData)>,
) -> Result<()> {
    for iteration in 1..=max_iterations {
        println!("=== campaign iteration {iteration}/{max_iterations} ===");

        let (driver, view, bank) =
            fetch().map_err(|e| anyhow!("iteration {iteration}: refetching live state: {e}"))?;
        let context = reference.with_bank(Arc::new(bank));

        // Reporting is only an observer. `live::run_workflow_or_done` owns the
        // mandatory plan and feasibility gate before invoking this hook.
        let pre_run: PreRunHook = Box::new(move |_, _, _, _, plan| {
            println!(
                "  build: produced an AST ({} action(s) predicted, {:.0}s)",
                plan.actions, plan.seconds
            );
            println!("  plan: feasible ({} action(s))", plan.actions);
            Ok(())
        });

        let outcome = live::run_workflow_or_done(
            driver,
            workflow_src,
            SharedView::new(view),
            &context,
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
