//! Thin CLI: load a `.fnl` workflow and run it through one of the two passes.
//!
//!   artifacts plan <workflow.fnl> <character> [key=value ...]   (needs ARTIFACTS_SECRET)
//!   artifacts run  <workflow.fnl> <character> [key=value ...]   (needs ARTIFACTS_SECRET)
//!   artifacts run  <workflow.fnl> <character> [key=value ...] --until-done [--max-iterations N]
//!
//! `plan` predicts cost and feasibility with no execution: seeded from the named
//! character's live state (position, hp, inventory) for a per-character
//! prediction, after fetching the overworld map + monster data (a workflow's
//! `build` calls `host.find_tile`, so the map must be loaded for the plan to be
//! truthful). Trailing `key=value` args set the workflow's declared params. `run`
//! hits the live API: it fetches the character + overworld map, then executes
//! the workflow's `run` pass. In both cases the `plan`/`run` *passes* are pure;
//! only the CLI bootstrap does I/O to populate the cached reference data.
//!
//! `--until-done` switches `run` into the M7 campaign loop
//! (`artifacts::campaign::run_until_done`): reference data (map/monsters/
//! resources/recipes/npc items) is fetched once, then each iteration refetches
//! the character + bank fresh, re-invokes `build`, plan-gates the result, and
//! runs it — stopping the moment `build` returns nil. Without `--until-done`,
//! `run`'s behavior is unchanged: single-shot, a nil build is a loud error.

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use artifacts::campaign;
use artifacts::context::ExecutionContext;
use artifacts::data::{BankData, MonsterData, NpcItemData, RecipeData, ResourceData};
use artifacts::driver::http::HttpDriver;
use artifacts::driver::Driver;
use artifacts::planner::{self, PlanResult, PlanSeed};
use artifacts::{live, tui};
use artifacts_core::step::CharacterView;

/// `--max-iterations` default when `--until-done` is given without one — the
/// harness's analogue of the interpreters' MAX-ITERS guard against a `build`
/// that never converges to nil (`DYNAMIC_WORKFLOWS` §8 M7).
const DEFAULT_MAX_ITERATIONS: u32 = 100;

const RUN_USAGE: &str = "usage: artifacts run <workflow.fnl> <character> [key=value ...] \
                          [--until-done] [--max-iterations N]";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        print_usage();
        bail!("no subcommand given");
    };

    match cmd.as_str() {
        "plan" => {
            let path = args
                .get(1)
                .context("usage: artifacts plan <workflow.fnl> <character> [key=value ...]")?;
            let character = args
                .get(2)
                .context("usage: artifacts plan <workflow.fnl> <character> [key=value ...]")?;
            let params = parse_params(args.get(3..).unwrap_or(&[]))?;
            let src = read_workflow(path)?;
            let ctx = load_live_context(character)?;
            // Label plan errors with the workflow's file stem (e.g. `farm`), not
            // the whole path or a placeholder.
            let name = std::path::Path::new(path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(path);
            let result = planner::plan_named(
                name,
                &src,
                &ctx.execution,
                &PlanSeed::from_view(&ctx.view),
                &params,
            )?;
            print_plan(path, &result);
        }
        "run" => {
            let path = args.get(1).context(RUN_USAGE)?;
            let character = args.get(2).context(RUN_USAGE)?;
            let (flags, kv_args) = parse_run_flags(args.get(3..).unwrap_or(&[]))?;
            let params = parse_params(&kv_args)?;
            let src = read_workflow(path)?;

            if flags.until_done {
                let reference = load_reference_data(character)?;
                let character = character.clone();
                campaign::run_until_done(
                    &src,
                    &reference,
                    &params,
                    flags.max_iterations.unwrap_or(DEFAULT_MAX_ITERATIONS),
                    move || {
                        let driver =
                            HttpDriver::from_env(character.as_str()).map_err(|e| anyhow!("{e}"))?;
                        let view = driver.fetch_character().map_err(|e| anyhow!("{e}"))?;
                        let bank = BankData::load(&driver)?;
                        Ok((Box::new(driver) as Box<dyn Driver>, view, bank))
                    },
                )?;
            } else {
                let ctx = load_live_context(character)?;
                let result = live::run_workflow(
                    Box::new(ctx.driver),
                    &src,
                    artifacts::character::SharedView::new(ctx.view),
                    &ctx.execution,
                    &params,
                    live::RunOptions::default(),
                )?;
                print_run(&result);
            }
        }
        "tui" => {
            let character = args.get(1).context("usage: artifacts tui <character>")?;
            let ctx = load_live_context(character)?;
            tui::run(character.to_string(), ctx.view, ctx.execution, ctx.driver)?;
        }
        "-h" | "--help" | "help" => print_usage(),
        other => {
            print_usage();
            bail!("unknown subcommand: {other}");
        }
    }
    Ok(())
}

/// Everything both `plan <character>` and `run` need, fetched in one place:
/// the driver, the character, the overworld map (so travel costs use real A*
/// hops rather than Manhattan), the TTL-cached monster/resource/recipe/
/// NPC-catalog data, and the account's bank holdings.
struct LiveContext {
    driver: HttpDriver,
    view: CharacterView,
    execution: ExecutionContext,
}

/// Construct the live driver and fetch a [`LiveContext`]. Unlike the TTL-cached
/// reference data, bank holdings are live account state and are deliberately
/// fetched fresh on every invocation (`BankData::load`'s doc;
/// `DYNAMIC_WORKFLOWS` §5.6).
fn load_live_context(character: &str) -> Result<LiveContext> {
    let driver = HttpDriver::from_env(character).map_err(|e| anyhow!("{e}"))?;
    let view = driver.fetch_character().map_err(|e| anyhow!("{e}"))?;
    let map = artifacts::data::load_overworld_map(&driver)?;
    let monsters = MonsterData::load(&driver)?;
    let resources = ResourceData::load(&driver)?;
    let recipes = RecipeData::load(&driver)?;
    let npc_items = NpcItemData::load(&driver)?;
    let bank = BankData::load(&driver)?;
    Ok(LiveContext {
        driver,
        view,
        execution: ExecutionContext {
            map: Some(Arc::new(map)),
            monsters: Some(Arc::new(monsters)),
            resources: Some(Arc::new(resources)),
            recipes: Some(Arc::new(recipes)),
            npc_items: Some(Arc::new(npc_items)),
            bank: Some(Arc::new(bank)),
        },
    })
}

/// The reference data the M7 campaign loop loads ONCE before iterating
/// (`DYNAMIC_WORKFLOWS` §8 M7): map/monsters/resources/recipes/npc items, via
/// a throwaway driver — unlike [`load_live_context`], no character view or
/// bank is fetched here, since those are live state each iteration refetches
/// itself (`campaign::run_until_done`'s `fetch` closure).
fn load_reference_data(character: &str) -> Result<ExecutionContext> {
    let driver = HttpDriver::from_env(character).map_err(|e| anyhow!("{e}"))?;
    let map = artifacts::data::load_overworld_map(&driver)?;
    let monsters = MonsterData::load(&driver)?;
    let resources = ResourceData::load(&driver)?;
    let recipes = RecipeData::load(&driver)?;
    let npc_items = NpcItemData::load(&driver)?;
    Ok(ExecutionContext {
        map: Some(Arc::new(map)),
        monsters: Some(Arc::new(monsters)),
        resources: Some(Arc::new(resources)),
        recipes: Some(Arc::new(recipes)),
        npc_items: Some(Arc::new(npc_items)),
        bank: None,
    })
}

fn print_plan(path: &str, result: &PlanResult) {
    println!("plan ({path}):");
    println!("  feasible:      {}", result.feasible);
    println!("  actions:       {}", result.actions);
    println!("  seconds:       {:.0}", result.seconds);
    println!("  action bucket: {}", result.bucket_action);
    if !result.assumptions.is_empty() {
        println!("  assumptions:");
        for (k, v) in &result.assumptions {
            println!("    {k}: {v}");
        }
    }
    if !result.blockers.is_empty() {
        println!("  blockers:");
        for b in &result.blockers {
            println!("    - {b}");
        }
    }
    if !result.warnings.is_empty() {
        println!("  warnings:");
        for w in &result.warnings {
            println!("    - {w}");
        }
    }
}

fn print_run(view: &CharacterView) {
    println!(
        "done: '{}' at ({}, {}), hp {}/{}, inventory {}/{}",
        view.name,
        view.x,
        view.y,
        view.hp,
        view.max_hp,
        view.inventory_count(),
        view.inventory_max_items
    );
}

fn read_workflow(path: &str) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("reading workflow {path}"))
}

/// Parse trailing `key=value` CLI arguments into raw param pairs. Coercion and
/// per-workflow help happen in Fennel (`fennel.lib.params`), so this only splits;
/// a token with no `=` is a loud usage error.
fn parse_params(args: &[String]) -> Result<Vec<(String, String)>> {
    args.iter()
        .map(|a| {
            a.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .ok_or_else(|| anyhow!("bad parameter '{a}': expected key=value"))
        })
        .collect()
}

/// `--until-done` / `--max-iterations N`, parsed out of `run`'s trailing args.
/// Everything not matched here is a `key=value` param candidate, handed to
/// [`parse_params`] unchanged (`DYNAMIC_WORKFLOWS` §8 M7's CLI shape).
#[derive(Debug, Default, PartialEq, Eq)]
struct RunFlags {
    until_done: bool,
    max_iterations: Option<u32>,
}

/// Split `run`'s trailing args into `(flags, remaining-key=value-candidates)`.
/// Loud errors: `--max-iterations` with a missing/non-numeric/zero value, and
/// `--max-iterations` given without `--until-done` (the guard is meaningless
/// for a single-shot run, so requiring `--until-done` catches the likely typo
/// of dropping it).
fn parse_run_flags(args: &[String]) -> Result<(RunFlags, Vec<String>)> {
    let mut flags = RunFlags::default();
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--until-done" => {
                flags.until_done = true;
                i += 1;
            }
            "--max-iterations" => {
                let raw = args.get(i + 1).ok_or_else(|| {
                    anyhow!("--max-iterations requires a value, e.g. --max-iterations 50")
                })?;
                let n: u32 = raw.parse().map_err(|_| {
                    anyhow!("--max-iterations expects a positive integer, got '{raw}'")
                })?;
                if n == 0 {
                    bail!("--max-iterations must be at least 1, got 0");
                }
                flags.max_iterations = Some(n);
                i += 2;
            }
            other => {
                rest.push(other.to_string());
                i += 1;
            }
        }
    }
    if flags.max_iterations.is_some() && !flags.until_done {
        bail!("--max-iterations only makes sense with --until-done");
    }
    Ok((flags, rest))
}

fn print_usage() {
    eprintln!(
        "artifacts — Artifacts MMO workflow runner\n\
         \n\
         USAGE:\n\
         \x20 artifacts plan <workflow.fnl> <character> [key=value ...]  (needs ARTIFACTS_SECRET)\n\
         \x20 artifacts run  <workflow.fnl> <character> [key=value ...]  (needs ARTIFACTS_SECRET)\n\
         \x20 artifacts run  <workflow.fnl> <character> [key=value ...] --until-done \
         [--max-iterations N]\n\
         \x20                                                            (needs ARTIFACTS_SECRET)\n\
         \x20 artifacts tui  <character>                                 (needs ARTIFACTS_SECRET)\n\
         \n\
         Trailing key=value args set the workflow's declared params, e.g.\n\
         \x20 artifacts plan fennel/workflows/farm.fnl nillinbot target=copper_rocks\n\
         \n\
         --until-done loops `run`: refetch live state, rebuild the workflow, plan-gate\n\
         it, run it, repeat — until `build` returns nil (default cap 100 iterations,\n\
         override with --max-iterations N).\n"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_flags_passes_everything_through() {
        let (flags, rest) = parse_run_flags(&args(&["target=copper_rocks", "qty=3"])).unwrap();
        assert_eq!(flags, RunFlags::default());
        assert_eq!(rest, vec!["target=copper_rocks", "qty=3"]);
    }

    #[test]
    fn until_done_alone_defaults_max_iterations_to_none() {
        let (flags, rest) =
            parse_run_flags(&args(&["target=copper_rocks", "--until-done"])).unwrap();
        assert!(flags.until_done);
        assert_eq!(flags.max_iterations, None);
        assert_eq!(rest, vec!["target=copper_rocks"]);
    }

    #[test]
    fn until_done_with_max_iterations() {
        let (flags, rest) = parse_run_flags(&args(&[
            "--until-done",
            "--max-iterations",
            "5",
            "target=x",
        ]))
        .unwrap();
        assert!(flags.until_done);
        assert_eq!(flags.max_iterations, Some(5));
        assert_eq!(rest, vec!["target=x"]);
    }

    #[test]
    fn max_iterations_without_until_done_is_an_error() {
        let err = parse_run_flags(&args(&["--max-iterations", "5"])).unwrap_err();
        assert!(
            format!("{err}").contains("--until-done"),
            "error should name --until-done: {err}"
        );
    }

    #[test]
    fn max_iterations_missing_value_is_an_error() {
        let err = parse_run_flags(&args(&["--until-done", "--max-iterations"])).unwrap_err();
        assert!(format!("{err}").contains("--max-iterations"));
    }

    #[test]
    fn max_iterations_malformed_value_is_an_error() {
        let err =
            parse_run_flags(&args(&["--until-done", "--max-iterations", "banana"])).unwrap_err();
        assert!(format!("{err}").contains("positive integer"));
    }

    #[test]
    fn max_iterations_zero_is_an_error() {
        let err = parse_run_flags(&args(&["--until-done", "--max-iterations", "0"])).unwrap_err();
        assert!(format!("{err}").contains("at least 1"));
    }
}
