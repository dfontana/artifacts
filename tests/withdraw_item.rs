/// Hermetic test for the :withdraw-item action — the prove-out addition for
/// the one-place intent chain (plans/INTENTS.md §6): plan pass predicts the
/// deposit-formula cost and the inventory gain; run pass hits the withdraw
/// endpoint via MockDriver and lands the items in the live view.
use artifacts::{
    driver::mock::{CannedResponse, MockDriver},
    lua::{eval_fennel, predicate_state, require_module, setup_lua, LuaSetupOptions},
};
use artifacts_core::{combat::CombatStats, step::CharacterView};
use mlua::prelude::*;

mod common;
use common::{char_json, response, spawn_mock, INV_MAX};

const WORKFLOW: &str = "(local {: seq : action} (require :fennel.lib.interp))\
\n(seq (action :withdraw-item [:copper_ore 3]))";

fn load_workflow(lua: &Lua) -> LuaValue {
    eval_fennel(lua, WORKFLOW, "withdraw.fnl").expect("failed to load workflow")
}

fn make_model_state(lua: &Lua) -> LuaTable {
    let st = predicate_state(lua, 0, 0, 100, 100, 0, INV_MAX, 0, &CombatStats::default())
        .expect("predicate_state failed");
    st.set("inventory", lua.create_table().unwrap()).unwrap();
    // `interp.fnl`'s assert-state requires `:tile` unconditionally (part of
    // the complete model-state key surface), even though :withdraw-item's
    // :cost/:sim never read it — an empty table satisfies the "key present"
    // check without affecting behavior.
    st.set("tile", lua.create_table().unwrap()).unwrap();
    st
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
    let (char, shared_view, scheduler_handle) = spawn_mock(driver, initial_view);

    let lua = setup_lua(LuaSetupOptions {
        character: Some(char),
        ..Default::default()
    })
    .expect("setup_lua with character failed");
    let wf = load_workflow(&lua);

    let interp = require_module(&lua, "fennel.lib.interp").expect("require interp");
    let run_fn: LuaFunction = interp.get("run").expect("run fn not found");
    run_fn.call::<()>(wf).expect("workflow run failed");

    assert_eq!(
        shared_view.get().inventory_count(),
        3,
        "withdraw should land 3 items in the live view"
    );

    drop(lua); // drops Character → closes the scheduler channel
    let _ = scheduler_handle.join();
}
