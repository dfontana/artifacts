//! Thin CLI: load a `.fnl` workflow and run it through one of the three passes.
//!
//!   artifacts estimate <workflow.fnl>
//!   artifacts simulate <workflow.fnl> [trials]
//!   artifacts run      <workflow.fnl> <character>   (needs ARTIFACTS_TOKEN)
//!
//! `run` hits the live API: it fetches the character + overworld map, then
//! executes the workflow's `run` pass. `estimate`/`simulate` are fully offline.

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use artifacts::driver::http::HttpDriver;
use artifacts::live;
use artifacts::planner::{self, PlanSeed};

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
        "estimate" => {
            let path = args
                .get(1)
                .context("usage: artifacts estimate <workflow.fnl>")?;
            let src = read_workflow(path)?;
            let result = planner::estimate(&src, None, &PlanSeed::default())?;
            println!("estimate ({path}):");
            println!("  actions:      {}", result.actions);
            println!("  seconds:      {:.0}", result.seconds);
            println!("  action bucket: {}", result.bucket_action);
            if !result.assumptions.is_empty() {
                println!("  assumptions:");
                for (k, v) in &result.assumptions {
                    println!("    {k}: {v}");
                }
            }
        }
        "simulate" => {
            let path = args
                .get(1)
                .context("usage: artifacts simulate <workflow.fnl> [trials]")?;
            let trials: u32 = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(1);
            let src = read_workflow(path)?;
            let result = planner::simulate(&src, None, &PlanSeed::default(), trials)?;
            println!("simulate ({path}, {trials} trials):");
            println!("  feasible: {}", result.feasible);
            println!("  gathers:  {}", result.gathers);
            println!("  seconds:  {:.0}", result.estimate.seconds);
            println!("  actions:  {}", result.estimate.actions);
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

    eprintln!("running workflow...");
    let final_view = live::run_workflow(Box::new(driver), src, view, Some(Arc::new(map)))?;

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
         \x20 artifacts estimate <workflow.fnl>\n\
         \x20 artifacts simulate <workflow.fnl> [trials]\n\
         \x20 artifacts run      <workflow.fnl> <character>   (needs ARTIFACTS_TOKEN)\n"
    );
}
