/// Hermetic test for the :withdraw-item action — the prove-out addition for
/// the one-place intent chain (plans/INTENTS.md §6): plan pass predicts the
/// deposit-formula cost and the inventory gain; run pass hits the withdraw
/// endpoint via MockDriver and lands the items in the live view.
use artifacts::{
    driver::mock::{CannedResponse, MockDriver},
    lua::{predicate_state, require_module, setup_lua, LuaSetupOptions},
    workflow,
};
use artifacts_core::{
    combat::CombatStats,
    step::{CharacterView, SkillLevels},
};
use mlua::prelude::*;

mod common;
use common::{char_json, response, INV_MAX};

// A workflow MODULE (not a bare AST): its `build` ignores params/ctx and returns
// the one-action AST. Both the plan test (via `workflow::load`) and the run test
// (via `live::run_workflow`) go through the module protocol.
const WORKFLOW: &str = "(local {: seq : action} (require :fennel.lib.interp))\
\n{:build (fn [_ _] (seq (action :withdraw-item [:copper_ore 3])))}";

fn load_workflow(lua: &Lua) -> LuaValue {
    let ctx = lua.create_table().expect("ctx table");
    workflow::load(lua, WORKFLOW, "withdraw.fnl", &[], ctx)
        .expect("failed to load workflow")
        .expect("built to an AST, not nil")
}

fn make_model_state(lua: &Lua) -> LuaTable {
    // predicate_state now builds the full state surface (including :skills and
    // :inventory), so this is the complete, valid state assert-state requires —
    // no manual key patching.
    predicate_state(
        lua,
        0,
        0,
        100,
        100,
        0,
        INV_MAX,
        0,
        &CombatStats::default(),
        &SkillLevels::default(),
        &[],
    )
    .expect("predicate_state failed")
}

#[test]
fn test_plan_pass_withdraw() {
    let lua = setup_lua(LuaSetupOptions::default()).expect("setup_lua failed");
    let wf = load_workflow(&lua);
    let st = make_model_state(&lua);

    let interp = require_module(&lua, "fennel.lib.interp").expect("require interp");
    let plan_fn: LuaFunction = interp.get("plan").expect("plan not found");
    let result: LuaTable = plan_fn.call((wf, st)).expect("plan call failed");

    let seconds: f64 = result.get("seconds").expect("missing seconds");
    let actions: u32 = result.get("actions").expect("missing actions");
    let feasible: bool = result.get("feasible").expect("missing feasible");

    // deposit formula, 1 distinct type: 3s.
    assert!((seconds - 3.0).abs() < 0.01, "expected 3s, got {seconds}");
    assert_eq!(actions, 1);
    assert!(feasible);
}

#[test]
fn test_run_pass_withdraw() {
    let mut driver = MockDriver::new();
    driver.push_responses(vec![CannedResponse::new(
        "action/bank/withdraw/item",
        200,
        response(3.0, char_json(0, 0, 3, 100)),
    )]);

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
        WORKFLOW,
        artifacts::character::SharedView::new(initial_view),
        None,
        None,
        None,
        None,
        None,
        None,
        &[],
        artifacts::live::RunOptions::default(),
    )
    .expect("run_workflow failed");

    assert_eq!(
        final_view.inventory_count(),
        3,
        "withdraw should land 3 items in the live view"
    );
}
