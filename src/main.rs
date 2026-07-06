//! Thin CLI: load a `.fnl` workflow and run it through one of the two passes.
//!
//!   artifacts plan <workflow.fnl> [character]   (needs ARTIFACTS_TOKEN; fetches the overworld map + monsters, no character)
//!   artifacts run  <workflow.fnl> <character>    (needs ARTIFACTS_TOKEN)
//!
//! `plan` predicts cost and feasibility with no execution: against a default
//! seed after fetching the overworld map + monster data (a workflow calls
//! `host.find_tile` at load time, so the map must be loaded for the plan to be
//! truthful), or — when a character is named — seeded from that character's
//! live state (position, hp, inventory) for a per-character prediction. `run`
//! hits the live API: it fetches the character + overworld map, then executes
//! the workflow's `run` pass. In both cases the `plan`/`run` *passes* are pure;
//! only the CLI bootstrap does I/O to populate the cached reference data.

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use artifacts::data::MonsterData;
use artifacts::driver::http::HttpDriver;
use artifacts::live;
use artifacts::planner::{self, PlanResult, PlanSeed};
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
                .context("usage: artifacts plan <workflow.fnl> [character]")?;
            let src = read_workflow(path)?;
            // With a character, seed the model from its live state for an
            // accurate per-character prediction; without one, plan against the
            // default seed — but still fetch the overworld map + monster data,
            // because workflows call `host.find_tile` (and may read
            // `host.monster_stats`) at load time. A token is required either way.
            let (seed, map, monsters) = match args.get(2) {
                Some(character) => seed_from_live(character)?,
                None => {
                    let (map, monsters) = load_static_context()?;
                    (
                        PlanSeed::default(),
                        Some(Arc::new(map)),
                        Some(Arc::new(monsters)),
                    )
                }
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
        "tui" => {
            let character = args.get(1).context("usage: artifacts tui <character>")?;
            run_tui(character)?;
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

/// Construct the live driver and fetch everything both `plan <character>` and
/// `run` need: the character, the overworld map (so travel costs use real A*
/// hops rather than Manhattan), and the TTL-cached monster data.
fn load_live_context(character: &str) -> Result<(HttpDriver, CharacterView, GameMap, MonsterData)> {
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

    eprintln!("loading overworld map (cached)...");
    let map = artifacts::data::load_overworld_map(&driver).context("loading map")?;
    eprintln!("  {} tiles loaded", map.tile_count());

    let monsters = load_monsters(&driver)?;

    Ok((driver, view, map, monsters))
}

/// Fetch a live character + overworld map and turn them into a planning seed,
/// so `plan` predicts from where the character actually is right now.
fn seed_from_live(character: &str) -> Result<LiveContext> {
    let (_driver, view, map, monsters) = load_live_context(character)?;
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

/// Fetch the overworld map + monster data with a character-less driver, for
/// the no-arg `plan` path. Workflows resolve tiles via `host.find_tile` at
/// load time (and may read `host.monster_stats`), so even a character-less
/// plan needs both. `/maps` and `/monsters` are top-level and public, so no
/// character is fetched — only the token is required.
fn load_static_context() -> Result<(GameMap, MonsterData)> {
    let driver = HttpDriver::from_env_token()
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("constructing HttpDriver (is ARTIFACTS_TOKEN set?)")?;

    eprintln!("loading overworld map (cached)...");
    let map = artifacts::data::load_overworld_map(&driver).context("loading map")?;
    eprintln!("  {} tiles loaded", map.tile_count());

    let monsters = load_monsters(&driver)?;
    Ok((map, monsters))
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
    let (driver, view, map, monsters) = load_live_context(character)?;

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
        final_view.inventory_count(),
        final_view.inventory_max_items
    );
    Ok(())
}

/// Launch the TUI: a live single-character dashboard that plans and runs
/// workflows. Reuses `load_live_context` and **keeps** the driver for idle polls
/// (§3.5) rather than discarding it as the plan/run paths do.
fn run_tui(character: &str) -> Result<()> {
    let (driver, view, map, monsters) = load_live_context(character)?;
    artifacts::tui::run(
        character.to_string(),
        view,
        Some(Arc::new(map)),
        Some(Arc::new(monsters)),
        driver,
    )
}

fn read_workflow(path: &str) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("reading workflow {path}"))
}

fn print_usage() {
    eprintln!(
        "artifacts — Artifacts MMO workflow runner\n\
         \n\
         USAGE:\n\
         \x20 artifacts plan <workflow.fnl> [character]   (needs ARTIFACTS_TOKEN; no character fetches only the map + monsters)\n\
         \x20 artifacts run  <workflow.fnl> <character>    (needs ARTIFACTS_TOKEN)\n\
         \x20 artifacts tui  <character>                   (needs ARTIFACTS_TOKEN)\n"
    );
}
