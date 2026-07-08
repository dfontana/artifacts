//! Thin CLI: load a `.fnl` workflow and run it through one of the two passes.
//!
//!   artifacts plan <workflow.fnl> <character> [key=value ...]   (needs ARTIFACTS_SECRET)
//!   artifacts run  <workflow.fnl> <character> [key=value ...]   (needs ARTIFACTS_SECRET)
//!
//! `plan` predicts cost and feasibility with no execution: seeded from the named
//! character's live state (position, hp, inventory) for a per-character
//! prediction, after fetching the overworld map + monster data (a workflow's
//! `build` calls `host.find_tile`, so the map must be loaded for the plan to be
//! truthful). Trailing `key=value` args set the workflow's declared params. `run`
//! hits the live API: it fetches the character + overworld map, then executes
//! the workflow's `run` pass. In both cases the `plan`/`run` *passes* are pure;
//! only the CLI bootstrap does I/O to populate the cached reference data.

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use artifacts::data::{MonsterData, RecipeData, ResourceData};
use artifacts::driver::http::HttpDriver;
use artifacts::planner::{self, PlanResult, PlanSeed};
use artifacts::{live, tui};
use artifacts_core::map::GameMap;
use artifacts_core::step::CharacterView;

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
            let (_driver, view, map, monsters, resources, recipes) = load_live_context(character)?;
            let result = planner::plan(
                &src,
                Some(Arc::new(map)),
                Some(Arc::new(monsters)),
                Some(Arc::new(resources)),
                Some(Arc::new(recipes)),
                &PlanSeed::from_view(&view),
                &params,
            )?;
            print_plan(path, &result);
        }
        "run" => {
            let path = args
                .get(1)
                .context("usage: artifacts run <workflow.fnl> <character> [key=value ...]")?;
            let character = args
                .get(2)
                .context("usage: artifacts run <workflow.fnl> <character> [key=value ...]")?;
            let params = parse_params(args.get(3..).unwrap_or(&[]))?;
            let src = read_workflow(path)?;
            let (driver, view, map, monsters, resources, recipes) = load_live_context(character)?;
            let result = live::run_workflow(
                Box::new(driver),
                &src,
                artifacts::character::SharedView::new(view),
                Some(Arc::new(map)),
                Some(Arc::new(monsters)),
                Some(Arc::new(resources)),
                Some(Arc::new(recipes)),
                &params,
                live::RunOptions::default(),
            )?;
            print_run(&result);
        }
        "tui" => {
            let character = args.get(1).context("usage: artifacts tui <character>")?;
            let (driver, view, map, monsters, resources, recipes) = load_live_context(character)?;
            tui::run(
                character.to_string(),
                view,
                Some(Arc::new(map)),
                Some(Arc::new(monsters)),
                Some(Arc::new(resources)),
                Some(Arc::new(recipes)),
                driver,
            )?;
        }
        "-h" | "--help" | "help" => print_usage(),
        other => {
            print_usage();
            bail!("unknown subcommand: {other}");
        }
    }
    Ok(())
}

/// Construct the live driver and fetch everything both `plan <character>` and
/// `run` need: the character, the overworld map (so travel costs use real A*
/// hops rather than Manhattan), and the TTL-cached monster, resource, and
/// recipe data.
fn load_live_context(
    character: &str,
) -> Result<(
    HttpDriver,
    CharacterView,
    GameMap,
    MonsterData,
    ResourceData,
    RecipeData,
)> {
    let driver = HttpDriver::from_env(character).map_err(|e| anyhow!("{e}"))?;
    let view = driver.fetch_character().map_err(|e| anyhow!("{e}"))?;
    let map = artifacts::data::load_overworld_map(&driver)?;
    let monsters = MonsterData::load(&driver)?;
    let resources = ResourceData::load(&driver)?;
    let recipes = RecipeData::load(&driver)?;
    Ok((driver, view, map, monsters, resources, recipes))
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

fn print_usage() {
    eprintln!(
        "artifacts — Artifacts MMO workflow runner\n\
         \n\
         USAGE:\n\
         \x20 artifacts plan <workflow.fnl> <character> [key=value ...]  (needs ARTIFACTS_SECRET)\n\
         \x20 artifacts run  <workflow.fnl> <character> [key=value ...]  (needs ARTIFACTS_SECRET)\n\
         \x20 artifacts tui  <character>                                 (needs ARTIFACTS_SECRET)\n\
         \n\
         Trailing key=value args set the workflow's declared params, e.g.\n\
         \x20 artifacts plan fennel/workflows/farm.fnl nillinbot target=copper_rocks\n"
    );
}
