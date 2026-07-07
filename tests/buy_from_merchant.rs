/// Hermetic acceptance test for the buy-from-merchant workflow.
///
/// Proves: gold is on the model-state surface (predicate_state), so a
/// gold-gated repeat-until loop resolves the SAME number of iterations whether
/// predicted offline (plan pass, pure model state) or executed for real (run
/// pass, live host.view) — and a workflow that starts too poor to buy even
/// once takes the when-guard's skip path instead of erroring.
///
/// Mock game data (deterministic):
///   - Character kael at (0,0)
///   - Timber merchant (npc) tile at (2,0) — resolved by the workflow via
///     `host.find_tile`, not hardcoded coordinates
///   - ash_wood costs 10 gold/unit (PRICE in buy-from-merchant.fnl)
use std::sync::Arc;

use artifacts::{
    driver::mock::{CannedResponse, MockDriver},
    lua::{eval_fennel, predicate_state, require_module, setup_lua, LuaSetupOptions},
};
use artifacts_core::{combat::CombatStats, map::GameMap, step::CharacterView};
use mlua::prelude::*;

mod common;
use common::{char_json_gold, response};

// ─── Mock game data constants ────────────────────────────────────────────────

/// Timber merchant tile at (2,0).
const MERCHANT_X: i32 = 2;
const MERCHANT_Y: i32 = 0;

/// ash_wood unit price, must match buy-from-merchant.fnl's PRICE.
const PRICE: u32 = 10;

/// Large enough that no test ever hits the inventory cap.
const INV_CAP: u32 = 100;

/// The 5×2 grid with the npc content tile `host.find_tile` resolves.
fn make_test_map() -> Arc<GameMap> {
    common::make_map(5, 2, &[(MERCHANT_X, MERCHANT_Y, "npc", "timber_merchant")])
}

fn load_workflow(lua: &Lua) -> LuaValue {
    eval_fennel(
        lua,
        include_str!("../fennel/workflows/buy-from-merchant.fnl"),
        "buy-from-merchant.fnl",
    )
    .expect("failed to load buy-from-merchant.fnl")
}

/// Build the initial model state table for the plan pass, mirroring
/// farm_copper's make_model_state: predicate_state plus the :inventory and
/// :tile keys build_state layers on (this workflow never gathers, so the tile
/// is a dummy — but assert-state requires the key regardless).
fn make_model_state(lua: &Lua, gold: u32) -> LuaTable {
    let st = predicate_state(
        lua,
        0,
        0,
        100,
        100,
        0,
        INV_CAP,
        gold,
        &CombatStats::default(),
    )
    .expect("predicate_state failed");
    st.set("inventory", lua.create_table().unwrap()).unwrap();
    st.set("tile", lua.create_table().unwrap()).unwrap();
    st
}

fn run_plan(lua: &Lua, wf: LuaValue, st: LuaTable) -> LuaTable {
    let interp = require_module(lua, "fennel.lib.interp").expect("require interp");
    let plan_fn: LuaFunction = interp.get("plan").expect("plan not found");
    plan_fn.call((wf, st)).expect("plan call failed")
}

// ─── Test 1: plan buys until broke ───────────────────────────────────────────

#[test]
fn test_plan_buys_until_broke() {
    let lua = setup_lua(LuaSetupOptions {
        map: Some(make_test_map()),
        ..Default::default()
    })
    .expect("setup_lua failed");
    let wf = load_workflow(&lua);
    let st = make_model_state(&lua, 97);

    let result = run_plan(&lua, wf, st);

    let actions: u32 = result.get("actions").expect("missing actions");
    let feasible: bool = result.get("feasible").expect("missing feasible");
    let blockers: LuaTable = result.get("blockers").expect("missing blockers");
    let assumptions: LuaTable = result.get("assumptions").expect("missing assumptions");
    let buys: u32 = assumptions.get("buys").unwrap_or(0);

    // 97 -> 87 -> ... -> 7: nine buys of 10 gold each (the tenth would need 10
    // but only 7 remain). This is the proof gold decrements per iteration —
    // otherwise the loop could never terminate.
    assert_eq!(
        buys, 9,
        "buys: expected 9 (97 -> 7 in steps of 10), got {buys}"
    );
    // 1 travel-to + 9 npc-buy.
    assert_eq!(actions, 10, "actions: expected 10, got {actions}");
    assert!(feasible, "plan: expected feasible=true");
    assert_eq!(blockers.raw_len(), 0, "plan: expected no blockers");
}

// ─── Test 2: plan skips the buy entirely when already broke ─────────────────

#[test]
fn test_plan_skips_when_broke() {
    let lua = setup_lua(LuaSetupOptions {
        map: Some(make_test_map()),
        ..Default::default()
    })
    .expect("setup_lua failed");
    let wf = load_workflow(&lua);
    let st = make_model_state(&lua, 5); // below PRICE=10

    let result = run_plan(&lua, wf, st);

    let actions: u32 = result.get("actions").expect("missing actions");
    let feasible: bool = result.get("feasible").expect("missing feasible");
    let blockers: LuaTable = result.get("blockers").expect("missing blockers");
    let assumptions: LuaTable = result.get("assumptions").expect("missing assumptions");
    let buys: u32 = assumptions.get("buys").unwrap_or(0);

    assert_eq!(buys, 0, "buys: expected 0 (gold=5 < PRICE=10), got {buys}");
    // Only the travel-to; the conditional gate keeps the character from
    // buying with no gold, rather than erroring or going negative.
    assert_eq!(
        actions, 1,
        "actions: expected 1 (travel only), got {actions}"
    );
    assert!(feasible, "plan: expected feasible=true");
    assert_eq!(blockers.raw_len(), 0, "plan: expected no blockers");
}

// ─── Test 3: the CLI live helper (live::run_workflow) end-to-end ────────────
//
// Drives the public `live::run_workflow` helper directly rather than hand-wiring
// setup_lua + require_module + run. That helper once regressed to
// `lua.globals().get("run")` (nil — `run` is a module export, not a global),
// breaking `artifacts run` on the CLI while every hand-wired test kept passing.
// Going through the public entrypoint is what closes that gap.

#[test]
fn test_run_workflow_helper_end_to_end() {
    let mut driver = MockDriver::new();
    driver.push_responses(build_canned_responses());

    let initial_view = CharacterView {
        name: "kael".into(),
        x: 0,
        y: 0,
        hp: 100,
        max_hp: 100,
        level: 1,
        inventory_max_items: INV_CAP,
        inventory: vec![],
        gold: 97,
        ..Default::default()
    };

    let final_view = artifacts::live::run_workflow(
        Box::new(driver),
        include_str!("../fennel/workflows/buy-from-merchant.fnl"),
        artifacts::character::SharedView::new(initial_view),
        Some(make_test_map()),
        None,
        None,
        artifacts::live::RunOptions::default(),
    )
    .expect("run_workflow failed");

    assert_eq!(
        final_view.gold, 7,
        "CLI helper should decrement gold 97 -> 7 over 9 buys"
    );
}

fn build_canned_responses() -> Vec<CannedResponse> {
    let mut responses = Vec::new();

    // 1. travel-to merchant: character arrives, gold unchanged.
    responses.push(CannedResponse::new(
        "action/move",
        200,
        response(10.0, char_json_gold(MERCHANT_X, MERCHANT_Y, 0, 100, 97)),
    ));

    // 2-10. npc-buy x9: gold steps down by PRICE=10 each time, 97 -> 7.
    for i in 1..=9u32 {
        let gold = 97 - i * PRICE;
        responses.push(CannedResponse::new(
            "action/npc/buy",
            200,
            response(3.0, char_json_gold(MERCHANT_X, MERCHANT_Y, i, 100, gold)),
        ));
    }

    responses
}
