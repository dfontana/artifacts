//! Composition acceptance tests (`plans/DYNAMIC_WORKFLOWS.md` §3, M2).
//!
//! Covers the whole chain the design promises: a workflow file becomes
//! require-able (`workflows.<stem>`), `use_workflow` composes one into another
//! and wraps it in a `:group`, and the composed result plans/skeletons exactly
//! like a hand-written workflow.
//!
//! `daily.fnl` (the composition reference — `fennel/workflows/daily.fnl`) is
//! read from disk here (not `include_str!`) so the FULL chain is exercised for
//! real: file -> module -> require -> compose. The sub-workflows it requires
//! (`workflows.farm`/`workflows.hunt`) are always resolved from disk by the
//! `workflows.<stem>` package searcher (`src/lua.rs`), whose default root
//! (`fennel/workflows`) is cwd-relative — cargo sets the test process's cwd to
//! the package root, same assumption the TUI's workflow scan relies on.

use std::sync::Arc;

use artifacts::{
    lua::{eval_fennel, require_module, setup_lua, LuaSetupOptions},
    planner::{self, PlanSeed},
    tui::skeleton::{marshal, StepKind},
    workflow,
};
use artifacts_core::{
    combat::{CombatStats, MonsterView},
    drop::DropRate,
    map::GameMap,
    step::SkillLevels,
};
use mlua::prelude::*;

mod common;
use common::{make_map, make_resources, INV_MAX};

const COPPER: (i32, i32) = (2, 0);
const BANK: (i32, i32) = (4, 1);
const CHICKEN: (i32, i32) = (1, 1);

/// A resource tile, a monster tile, and a bank — the fixture every test here
/// shares, so `daily.fnl` (farm the resource, then hunt the monster) has
/// somewhere real to resolve both sub-workflows' `host.find_tile` calls.
fn fixture_map() -> Arc<GameMap> {
    make_map(
        6,
        2,
        &[
            (COPPER.0, COPPER.1, "resource", "copper_rocks"),
            (BANK.0, BANK.1, "bank", "bank"),
            (CHICKEN.0, CHICKEN.1, "monster", "chicken"),
        ],
    )
}

/// A chicken that the seeded character always one-shots from full HP (no
/// damage taken, no rest needed) and that drops exactly 1 `copper_ore` per win
/// (rate 1) — a deterministic yield, so the fight loop resolves to a fixed
/// iteration count in the plan pass exactly like farm's gather loop.
fn chicken_monster_data() -> Arc<artifacts::data::MonsterData> {
    let chicken = MonsterView {
        code: "chicken".into(),
        name: "Chicken".into(),
        level: 1,
        hp: 30,
        attack_fire: 0,
        attack_earth: 0,
        attack_water: 0,
        attack_air: 0,
        res_fire: 0,
        res_earth: 0,
        res_water: 0,
        res_air: 0,
        critical_strike: 0,
        initiative: 0,
        drops: vec![DropRate {
            code: "copper_ore".into(),
            rate: 1,
            min_quantity: 1,
            max_quantity: 1,
        }],
    };
    Arc::new(artifacts::data::MonsterData::from_vec(vec![chicken]))
}

/// A seed the character can win every fight from (attack 50 fire, no damage
/// taken back), shared by every plan call in this file.
fn winning_seed() -> PlanSeed {
    PlanSeed {
        hp: 100,
        max_hp: 100,
        inventory_max_items: INV_MAX,
        combat: CombatStats {
            attack: [50, 0, 0, 0],
            ..Default::default()
        },
        // mining >= 1 so farm's gather of copper_rocks (mining 1) isn't gated.
        skills: SkillLevels {
            mining: 1,
            ..Default::default()
        },
        ..PlanSeed::default()
    }
}

// ─── Test 1: composed plan is feasible and additive over its sub-plans ──────

#[test]
fn composed_daily_plans_feasible_with_summed_actions() {
    let map = fixture_map();
    let resources = make_resources(&[("copper_rocks", "mining", 1, "copper_ore")]);
    let monsters = chicken_monster_data();
    let seed = winning_seed();

    let farm_result = planner::plan(
        include_str!("../fennel/workflows/farm.fnl"),
        Some(map.clone()),
        Some(monsters.clone()),
        Some(resources.clone()),
        None,
        &seed,
        &[("target".to_string(), "copper_rocks".to_string())],
    )
    .expect("farm.fnl should plan");
    assert!(farm_result.feasible, "farm.fnl alone should be feasible");

    let hunt_result = planner::plan(
        include_str!("../fennel/workflows/hunt.fnl"),
        Some(map.clone()),
        Some(monsters.clone()),
        Some(resources.clone()),
        None,
        &seed,
        &[("target".to_string(), "chicken".to_string())],
    )
    .expect("hunt.fnl should plan");
    assert!(hunt_result.feasible, "hunt.fnl alone should be feasible");

    // daily.fnl itself read from disk — the full file -> module -> require ->
    // compose chain, not a compiled-in string.
    let daily_src = std::fs::read_to_string("fennel/workflows/daily.fnl")
        .expect("fennel/workflows/daily.fnl should be readable from the package root");
    let daily_result = planner::plan(
        &daily_src,
        Some(map),
        Some(monsters),
        Some(resources),
        None,
        &seed,
        &[
            ("resource".to_string(), "copper_rocks".to_string()),
            ("monster".to_string(), "chicken".to_string()),
        ],
    )
    .expect("daily.fnl should plan (require + compose end-to-end)");

    assert!(
        daily_result.feasible,
        "composed daily.fnl should be feasible"
    );
    assert_eq!(
        daily_result.actions,
        farm_result.actions + hunt_result.actions,
        "a :group node adds no cost/action-count of its own: the composed \
         action total must equal the sum of its two sub-plans"
    );
}

// ─── Test 2: skeleton shows two group rows, correct depths, intact ids/guards ─

#[test]
fn composed_daily_skeleton_shows_two_groups() {
    let map = fixture_map();
    let lua = setup_lua(LuaSetupOptions {
        map: Some(map),
        ..Default::default()
    })
    .expect("setup_lua");

    let daily_src = std::fs::read_to_string("fennel/workflows/daily.fnl")
        .expect("fennel/workflows/daily.fnl should be readable from the package root");
    let ctx = lua.create_table().expect("ctx table");
    let wf = workflow::load(
        &lua,
        &daily_src,
        "daily.fnl",
        &[
            ("resource".to_string(), "copper_rocks".to_string()),
            ("monster".to_string(), "chicken".to_string()),
        ],
        ctx,
    )
    .expect("load daily.fnl")
    .expect("daily.fnl built to an AST, not nil");

    let interp = require_module(&lua, "fennel.lib.interp").expect("require interp");
    let number_nodes: LuaFunction = interp.get("number_nodes").unwrap();
    number_nodes.call::<LuaValue>(&wf).expect("number_nodes");
    let skeleton_fn: LuaFunction = interp.get("skeleton").unwrap();
    let sk_tbl: LuaTable = skeleton_fn.call(&wf).expect("skeleton");
    let skeleton = marshal(&sk_tbl).expect("marshal skeleton");

    // ids strictly increasing across the whole flattened row list.
    for w in skeleton.windows(2) {
        assert!(
            w[0].id < w[1].id,
            "skeleton row ids must strictly increase: {} then {}",
            w[0].id,
            w[1].id
        );
    }

    // Exactly two top-level (:depth 0) :group rows: farm's then hunt's.
    let top_groups: Vec<usize> = skeleton
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind == StepKind::Group && s.depth == 0)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        top_groups.len(),
        2,
        "expected two top-level group rows (farm, hunt): {skeleton:?}"
    );

    let farm_group = &skeleton[top_groups[0]];
    let farm_label = farm_group.label.as_deref().unwrap_or_default();
    assert!(
        farm_label.contains("farm") && farm_label.contains("target=copper_rocks"),
        "farm group label should name the workflow + its coerced params: {farm_label}"
    );
    assert!(
        farm_group.guard_id.is_none(),
        "top-level group is unguarded"
    );

    let hunt_group = &skeleton[top_groups[1]];
    let hunt_label = hunt_group.label.as_deref().unwrap_or_default();
    assert!(
        hunt_label.contains("hunt") && hunt_label.contains("target=chicken"),
        "hunt group label should name the workflow + its coerced params: {hunt_label}"
    );
    assert!(
        hunt_group.guard_id.is_none(),
        "top-level group is unguarded"
    );

    // Every row strictly between the two group headers is farm's — all at
    // depth >= 1 (children of the farm group, one level or more below it).
    for s in &skeleton[top_groups[0] + 1..top_groups[1]] {
        assert!(
            s.depth >= 1,
            "farm's rows must sit at depth >= 1 under its group: {s:?}"
        );
    }
    // Same for every row after the hunt group header.
    for s in &skeleton[top_groups[1] + 1..] {
        assert!(
            s.depth >= 1,
            "hunt's rows must sit at depth >= 1 under its group: {s:?}"
        );
    }

    // hunt's `rest` row is guarded by its `when`; guard-id survived the extra
    // :group nesting level use_workflow introduced.
    let rest_row = skeleton
        .iter()
        .find(|s| s.op == "rest")
        .expect("a rest row in the composed skeleton");
    assert!(
        rest_row.guard_id.is_some(),
        "rest's guard-id must pass through the :group unchanged"
    );
}

// ─── Test 3: use_workflow surfaces a param error with the declared-param help ─

#[test]
fn use_workflow_missing_required_param_errors_with_help() {
    let lua = setup_lua(LuaSetupOptions::default()).expect("setup_lua");
    let err = eval_fennel(
        &lua,
        "(local {: use_workflow} (require :fennel.lib.interp))\n\
         (use_workflow :farm {} {})",
        "bad_params.fnl",
    )
    .expect_err("use_workflow with a missing required param must error");
    let msg = err.to_string();
    assert!(
        msg.contains("target"),
        "error should name the missing param 'target': {msg}"
    );
    assert!(
        msg.contains("declared params"),
        "error should carry the declared-param help: {msg}"
    );
}

// ─── Test 4: requiring a nonexistent workflow surfaces the searcher's message ─

#[test]
fn require_nonexistent_workflow_errors_via_searcher() {
    let lua = setup_lua(LuaSetupOptions::default()).expect("setup_lua");
    let err = eval_fennel(&lua, "(require :workflows.nope)", "req_nope.fnl")
        .expect_err("requiring a nonexistent workflow module must error");
    let msg = err.to_string();
    assert!(
        msg.contains("nope"),
        "error should name the missing module 'nope': {msg}"
    );
    assert!(
        msg.contains("fennel/workflows/nope.fnl") || msg.contains("no file"),
        "error should carry the searcher's file-not-found message: {msg}"
    );
}
