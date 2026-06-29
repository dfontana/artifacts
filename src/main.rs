//! Thin CLI: load a `.fnl` workflow and run it through one of the two passes.
//!
//!   artifacts plan <workflow.fnl> [character]   ([character] needs ARTIFACTS_TOKEN)
//!   artifacts run  <workflow.fnl> <character>    (needs ARTIFACTS_TOKEN)
//!
//! `plan` predicts cost and feasibility with no execution: offline against a
//! default seed, or — when a character is named — seeded from that character's
//! live state (position, hp, inventory) for a per-character prediction. `run`
//! hits the live API: it fetches the character + overworld map, then executes
//! the workflow's `run` pass.

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use artifacts::data::MonsterData;
use artifacts::driver::http::HttpDriver;
use artifacts::live;
use artifacts::planner::{self, PlanResult, PlanSeed};
use artifacts_core::map::GameMap;

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
                .context("usage: artifacts plan <workflow.fnl> [character]")?;
            let src = read_workflow(path)?;
            // With a character, seed the model from its live state for an
            // accurate per-character prediction; without one, plan offline
            // against the default seed.
            let (seed, map, monsters) = match args.get(2) {
                Some(character) => seed_from_live(character)?,
                None => (PlanSeed::default(), None, None),
            };
            let result = planner::plan(&src, map, monsters, &seed)?;
            print_plan(path, &result);
        }
        "run" => {
            let path = args
                .get(1)
                .context("usage: artifacts run <workflow.fnl> <character>")?;
            let character = args
                .get(2)
                .context("usage: artifacts run <workflow.fnl> <character>")?;
            let src = read_workflow(path)?;
            run_live(&src, character)?;
        }
        "-h" | "--help" | "help" => print_usage(),
        other => {
            print_usage();
            bail!("unknown subcommand: {other}");
        }
    }
    Ok(())
}

/// A planning seed plus the map and monster data the plan/run passes need.
type LiveContext = (PlanSeed, Option<Arc<GameMap>>, Option<Arc<MonsterData>>);

/// Fetch a live character + overworld map and turn them into a planning seed,
/// so `plan` predicts from where the character actually is right now.
fn seed_from_live(character: &str) -> Result<LiveContext> {
    let driver = HttpDriver::from_env(character)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("constructing HttpDriver (is ARTIFACTS_TOKEN set?)")?;

    eprintln!("fetching character '{character}'...");
    let view = driver
        .fetch_character()
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("fetching character")?;
    eprintln!(
        "  at ({}, {}), hp {}/{}, inventory {}/{}",
        view.x,
        view.y,
        view.hp,
        view.max_hp,
        view.inventory_count(),
        view.inventory_max_items
    );

    // The map lets travel costs use real A* hops rather than Manhattan.
    eprintln!("fetching overworld map...");
    let map = driver
        .fetch_overworld_map()
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("fetching map")?;
    eprintln!("  {} tiles loaded", map.tile_count());

    let monsters = load_monsters(&driver)?;

    Ok((
        PlanSeed::from_view(&view),
        Some(Arc::new(map)),
        Some(Arc::new(monsters)),
    ))
}

/// Load monster reference data (TTL disk cache, fetched on miss).
fn load_monsters(driver: &HttpDriver) -> Result<MonsterData> {
    eprintln!("loading monster data (cached)...");
    let monsters = MonsterData::load(driver).context("loading monster data")?;
    eprintln!("  {} monsters available", monsters.len());
    Ok(monsters)
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

fn run_live(src: &str, character: &str) -> Result<()> {
    let driver = HttpDriver::from_env(character)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("constructing HttpDriver (is ARTIFACTS_TOKEN set?)")?;

    eprintln!("fetching character '{character}'...");
    let view = driver
        .fetch_character()
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("fetching character")?;
    eprintln!(
        "  at ({}, {}), hp {}/{}, inventory {}/{}",
        view.x,
        view.y,
        view.hp,
        view.max_hp,
        view.inventory_slots_used(),
        view.inventory_max_items
    );

    eprintln!("fetching overworld map...");
    let map = driver
        .fetch_overworld_map()
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("fetching map")?;
    eprintln!("  {} tiles loaded", map.tile_count());

    let monsters = load_monsters(&driver)?;

    eprintln!("running workflow...");
    let final_view = live::run_workflow(
        Box::new(driver),
        src,
        view,
        Some(Arc::new(map)),
        Some(Arc::new(monsters)),
    )?;

    println!(
        "done: '{}' at ({}, {}), hp {}/{}, inventory {}/{}",
        final_view.name,
        final_view.x,
        final_view.y,
        final_view.hp,
        final_view.max_hp,
        final_view.inventory_slots_used(),
        final_view.inventory_max_items
    );
    Ok(())
}

fn read_workflow(path: &str) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("reading workflow {path}"))
}

fn print_usage() {
    eprintln!(
        "artifacts — Artifacts MMO workflow runner\n\
         \n\
         USAGE:\n\
         \x20 artifacts plan <workflow.fnl> [character]   ([character] needs ARTIFACTS_TOKEN)\n\
         \x20 artifacts run  <workflow.fnl> <character>    (needs ARTIFACTS_TOKEN)\n"
    );
}
