/// Hermetic acceptance test for the farm-copper workflow.
///
/// Proves: one Fennel source, planned offline and executed, with
/// cooldowns/rate-limits invisible to the workflow author.
///
/// Mock game data (deterministic):
///   - Character kael at (0,0), inventory 0/10 slots
///   - COPPER (copper_rocks) tile at (2,0), BANK tile at (4,1) — resolved by
///     the workflow via `host.find_tile`, not hardcoded coordinates
///   - Copper resource level 1 → gather cooldown 30 + 1/2 = 30s (floor)
///   - Movement 5s per tile; deposit 3s per distinct item type
///   - Distances via Manhattan
use std::sync::Arc;

use artifacts::{
    driver::mock::{CannedResponse, MockDriver},
    lua::{eval_fennel, predicate_state, require_module, setup_lua, LuaSetupOptions},
    workflow,
};
use artifacts_core::{
    combat::CombatStats,
    map::GameMap,
    step::{CharacterView, SkillLevels},
};
use mlua::prelude::*;

mod common;
use common::{char_json, make_map, make_resources, response, INV_MAX};

// ─── Mock game data constants ────────────────────────────────────────────────

/// COPPER tile at (2,0), resource level 1.
/// Gather cooldown = 30 + floor(1/2) = 30s.
const COPPER_X: i32 = 2;
const COPPER_Y: i32 = 0;
const COPPER_LEVEL: u32 = 1;

/// BANK tile at (4,1).
const BANK_X: i32 = 4;
const BANK_Y: i32 = 1;

/// Expected plan values (derived from formulas, not hand-tuned).
///
/// travel (0,0)→(2,0): 2 tiles × 5s = 10s
/// gather ×10: 10 × 30s = 300s
/// travel (2,0)→(4,1): 3 tiles × 5s = 15s
/// deposit-all (1 distinct type): 1 × 3s = 3s
/// total: 328s, 13 actions, 13 action bucket cost, 10 gathers
const EXPECTED_SECONDS: f64 = 328.0;
const EXPECTED_ACTIONS: u32 = 13;
const EXPECTED_BUCKET_ACTION: u32 = 13;
const EXPECTED_GATHERS: u32 = 10;

/// The 5×2 grid with the content tiles `host.find_tile` resolves.
fn make_test_map() -> Arc<GameMap> {
    make_map(
        5,
        2,
        &[
            (COPPER_X, COPPER_Y, "resource", "copper_rocks"),
            (BANK_X, BANK_Y, "bank", "bank"),
        ],
    )
}

// ─── Helper: build a Lua state for the plan pass (no Character handle) ──────

fn make_plan_lua() -> Lua {
    setup_lua(LuaSetupOptions {
        map: Some(make_test_map()),
        resources: Some(make_resources(&[(
            "copper_rocks",
            "mining",
            COPPER_LEVEL,
            "copper_ore",
        )])),
        ..Default::default()
    })
    .expect("setup_lua failed")
}

/// Load the farm workflow AST into the Lua state and return it. Goes through the
/// workflow-module protocol (`workflow::load`): coerce `target=copper_rocks`,
/// call `build` (its `ctx` unused here), take the AST it returns. `ctx` can be
/// any table since farm's `build` ignores it.
fn load_workflow(lua: &Lua) -> LuaValue {
    let ctx = lua.create_table().expect("ctx table");
    workflow::load(
        lua,
        include_str!("../fennel/workflows/farm.fnl"),
        "farm.fnl",
        &[("target".to_string(), "copper_rocks".to_string())],
        ctx,
    )
    .expect("failed to load farm.fnl")
    .expect("farm.fnl built to an AST, not nil")
}

/// Build the initial model state table for the plan pass, standing at `(x, y)`.
/// `:gather`'s :cost/:sim now resolve the resource under the model's *current*
/// position via `host.active_resource`, so the caller picks a position that's
/// actually a resource tile in `make_test_map` when the workflow gathers
/// without traveling first.
fn make_model_state(lua: &Lua, x: i32, y: i32) -> LuaTable {
    // Build the seed state through the SAME single source production uses
    // (planner::build_state = predicate_state + :inventory), rather than
    // hand-rolling each key. The old hand-rolled table omitted :combat and
    // survived only because farm-copper has no :fight step; a :fight workflow
    // reusing it would have crashed at st.combat.haste with a cryptic nil-index
    // error. Going through predicate_state makes it a complete, valid state by
    // construction (and assert-state in interp.fnl now enforces that at plan
    // entry).
    // Skills carry mining >= COPPER_LEVEL so the gather skill gate passes (a
    // resource above the character's skill is now a plan blocker); inventory
    // starts empty. predicate_state builds :inventory itself now, so no manual set.
    let skills = SkillLevels {
        mining: COPPER_LEVEL,
        ..Default::default()
    };
    predicate_state(
        lua,
        x,
        y,
        100,
        100,
        0,
        INV_MAX,
        0,
        &CombatStats::default(),
        &skills,
        &[],
    )
    .expect("predicate_state failed")
}

// ─── Test 1: plan pass (cost + feasibility) ──────────────────────────────────

#[test]
fn test_plan_pass() {
    let lua = make_plan_lua();
    let wf = load_workflow(&lua);
    // Starts at (0,0): the workflow travels to COPPER itself via find_tile.
    let st = make_model_state(&lua, 0, 0);

    let interp = require_module(&lua, "fennel.lib.interp").expect("require interp");
    let plan_fn: LuaFunction = interp.get("plan").expect("plan not found");
    let result: LuaTable = plan_fn.call((wf, st)).expect("plan call failed");

    let seconds: f64 = result.get("seconds").expect("missing seconds");
    let actions: u32 = result.get("actions").expect("missing actions");
    let bucket_cost: LuaTable = result.get("bucket-cost").expect("missing bucket-cost");
    let bucket_action: u32 = bucket_cost.get("action").unwrap_or(0);
    let assumptions: LuaTable = result.get("assumptions").expect("missing assumptions");
    let gathers: u32 = assumptions.get("gathers").unwrap_or(0);
    let feasible: bool = result.get("feasible").expect("missing feasible");
    let blockers: LuaTable = result.get("blockers").expect("missing blockers");

    assert_eq!(
        actions, EXPECTED_ACTIONS,
        "actions: expected {EXPECTED_ACTIONS}, got {actions}"
    );
    assert!(
        (seconds - EXPECTED_SECONDS).abs() < 0.5,
        "seconds: expected {EXPECTED_SECONDS}, got {seconds}"
    );
    assert_eq!(
        gathers, EXPECTED_GATHERS,
        "gathers: expected {EXPECTED_GATHERS} (resolved by plan loop), got {gathers}"
    );
    assert_eq!(
        bucket_action, EXPECTED_BUCKET_ACTION,
        "bucket-cost.action: expected {EXPECTED_BUCKET_ACTION}, got {bucket_action}"
    );
    // The farm-copper loop banks exactly when full, so nothing overflows.
    assert!(feasible, "plan: expected feasible=true");
    assert_eq!(blockers.raw_len(), 0, "plan: expected no blockers");
}

// ─── Test 2: plan flags an infeasible workflow ───────────────────────────────

#[test]
fn test_plan_detects_inventory_overflow() {
    let lua = make_plan_lua();
    // Gather a fixed 20 times into a 10-slot inventory: the model overflows and
    // the plan must report it as infeasible rather than silently predicting cost.
    let wf = eval_fennel(
        &lua,
        "(local {: seq : action : repeat_n} (require :fennel.lib.interp))\
\n(seq (repeat_n 20 (action :gather)))",
        "overflow.fnl",
    )
    .expect("failed to load overflow workflow");
    // No travel step in this workflow — stand directly on the resource tile.
    let st = make_model_state(&lua, COPPER_X, COPPER_Y);

    let interp = require_module(&lua, "fennel.lib.interp").expect("require interp");
    let plan_fn: LuaFunction = interp.get("plan").expect("plan not found");
    let result: LuaTable = plan_fn.call((wf, st)).expect("plan call failed");

    let feasible: bool = result.get("feasible").expect("missing feasible");
    let blockers: LuaTable = result.get("blockers").expect("missing blockers");

    assert!(
        !feasible,
        "plan: gathering past capacity should be infeasible"
    );
    assert!(
        blockers.raw_len() >= 1,
        "plan: expected at least one overflow blocker, got {}",
        blockers.raw_len()
    );
}

// ─── Test 4: planner::plan (the public entrypoint the CLI + TUI call) ────────

/// Regression: drive the real `planner::plan` API, not the plan pass reassembled
/// by hand (`test_plan_pass`). `planner::plan` is the surface both the CLI `plan`
/// command and the TUI plan preview call; it fetches the interp `plan` fn — a
/// `require`-able module export, NOT a Lua global. A version that fetched it off
/// the global table got `nil` and failed every invocation with "converting Lua
/// nil to function", yet the hand-assembled `test_plan_pass` (which requires the
/// module itself) stayed green and hid it. Going through the public fn is what
/// closes that gap — assert the same feasible result `test_plan_pass` pins, but
/// via the entrypoint production actually uses.
#[test]
fn planner_plan_entrypoint_returns_feasible() {
    use artifacts::planner::{self, PlanSeed};

    // Seed matches make_model_state: (0,0), hp 100, INV_MAX cap.
    let seed = PlanSeed {
        inventory_max_items: INV_MAX,
        // mining >= COPPER_LEVEL so the gather skill gate passes.
        skills: SkillLevels {
            mining: COPPER_LEVEL,
            ..Default::default()
        },
        ..PlanSeed::default()
    };
    let result = planner::plan(
        include_str!("../fennel/workflows/farm.fnl"),
        Some(make_test_map()),
        None,
        Some(make_resources(&[(
            "copper_rocks",
            "mining",
            COPPER_LEVEL,
            "copper_ore",
        )])),
        None,
        None,
        None,
        &seed,
        &[("target".to_string(), "copper_rocks".to_string())],
    )
    .expect("planner::plan should succeed for farm (not error on a nil fn)");

    assert!(result.feasible, "farm plan should be feasible");
    assert_eq!(
        result.actions, EXPECTED_ACTIONS,
        "actions via planner::plan should match the hand path"
    );
    assert!(
        (result.seconds - EXPECTED_SECONDS).abs() < 0.5,
        "seconds via planner::plan: expected {EXPECTED_SECONDS}, got {}",
        result.seconds
    );
}

// ─── Test 3: run pass against MockDriver ────────────────────────────────────

#[test]
fn test_run_pass() {
    // Build canned responses for the 13 actions:
    // 1 travel-to COPPER, 10 gather, 1 travel-to BANK, 1 deposit-all
    let responses = build_canned_responses();

    let mut driver = MockDriver::new();
    driver.push_responses(responses);

    let initial_view = CharacterView {
        name: "kael".into(),
        x: 0,
        y: 0,
        hp: 100,
        max_hp: 100,
        level: 1,
        inventory_max_items: INV_MAX,
        inventory: vec![],
        ..Default::default()
    };

    let final_view = artifacts::live::run_workflow(
        Box::new(driver),
        include_str!("../fennel/workflows/farm.fnl"),
        artifacts::character::SharedView::new(initial_view),
        Some(make_test_map()),
        None,
        None,
        None,
        None,
        None,
        &[("target".to_string(), "copper_rocks".to_string())],
        artifacts::live::RunOptions::default(),
    )
    .expect("run_workflow failed");

    // After the workflow: character should be at BANK with empty inventory.
    assert_eq!(final_view.x, BANK_X, "final x should be BANK_X={BANK_X}");
    assert_eq!(final_view.y, BANK_Y, "final y should be BANK_Y={BANK_Y}");
    assert_eq!(
        final_view.inventory_count(),
        0,
        "inventory should be empty after deposit-all"
    );
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn build_canned_responses() -> Vec<CannedResponse> {
    let mut responses = Vec::new();

    // 1. travel-to COPPER: 2 tiles × 5s = 10s
    responses.push(CannedResponse::new(
        "action/move",
        200,
        response(10.0, char_json(COPPER_X, COPPER_Y, 0, 100)),
    ));

    // 2–11. gather ×10 (filling inventory one item per gather)
    for i in 1..=10u32 {
        responses.push(CannedResponse::new(
            "action/gathering",
            200,
            response(30.0, char_json(COPPER_X, COPPER_Y, i, 100)),
        ));
    }

    // 12. travel-to BANK: 3 tiles × 5s = 15s
    responses.push(CannedResponse::new(
        "action/move",
        200,
        response(15.0, char_json(BANK_X, BANK_Y, 10, 100)),
    ));

    // 13. deposit-all (expanded to deposit-item for copper_ore): 3s
    responses.push(CannedResponse::new(
        "action/bank/deposit/item",
        200,
        response(3.0, char_json(BANK_X, BANK_Y, 0, 100)),
    ));

    responses
}
