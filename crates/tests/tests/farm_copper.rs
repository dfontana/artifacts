/// Hermetic acceptance test for the farm-copper workflow.
///
/// Proves: one Fennel source, planned offline and executed, with
/// cooldowns/rate-limits invisible to the workflow author.
///
/// Mock game data (deterministic):
///   - Character kael at (0,0), inventory 0/10 slots
///   - COPPER tile at (2,0), BANK tile at (4,1)
///   - Copper resource level 1 → gather cooldown 30 + 1/2 = 30s (floor)
///   - Movement 5s per tile; deposit 3s per distinct item type
///   - Distances via Manhattan

use std::sync::Arc;
use artifacts_driver::mock::{CannedResponse, MockDriver};
use artifacts_runtime::{
    character::Character,
    lua::{eval_fennel, setup_lua},
    scheduler::Scheduler,
    view::SharedView,
};
use artifacts_core::{map::GameMap, step::CharacterView};
use mlua::prelude::*;
use tokio::sync::mpsc;

// ─── Mock game data constants ────────────────────────────────────────────────

/// COPPER tile at (2,0), resource level 1.
/// Gather cooldown = 30 + floor(1/2) = 30s.
const COPPER_X: i32 = 2;
const COPPER_Y: i32 = 0;
const COPPER_LEVEL: u32 = 1;

/// BANK tile at (4,1).
const BANK_X: i32 = 4;
const BANK_Y: i32 = 1;

/// Inventory cap.
const INV_MAX: u32 = 10;

/// Expected estimate values (derived from formulas, not hand-tuned).
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

// ─── Helper: build a 5×2 clear map matching the farm-copper scenario ─────────

fn make_test_map() -> Arc<GameMap> {
    use artifacts_core::map::{AccessSchema, GameMap, InteractionSchema, MapAccessType, MapTile};
    let mut m = GameMap::new();
    for y in 0i32..2 {
        for x in 0i32..5 {
            m.insert(MapTile {
                map_id: y * 10 + x,
                name: format!("{x},{y}"),
                skin: "grass".into(),
                x,
                y,
                layer: "overworld".into(),
                access: AccessSchema { access_type: MapAccessType::Standard },
                interactions: InteractionSchema::default(),
            });
        }
    }
    Arc::new(m)
}

// ─── Helper: build a Lua state for estimate/simulate (no Character handle) ───

fn make_estimate_lua() -> Lua {
    setup_lua(None, Some(make_test_map())).expect("setup_lua failed")
}

/// Load the farm-copper workflow AST into the Lua state and return it.
fn load_workflow(lua: &Lua) -> LuaValue {
    eval_fennel(
        lua,
        include_str!("../../../fennel/workflows/farm-copper.fnl"),
        "farm-copper.fnl",
    )
    .expect("failed to load farm-copper.fnl")
}

/// Build the initial model state table for estimate/simulate.
fn make_model_state(lua: &Lua) -> LuaTable {
    let st = lua.create_table().unwrap();
    st.set("x", 0i32).unwrap();
    st.set("y", 0i32).unwrap();
    st.set("hp", 100u32).unwrap();
    st.set("max-hp", 100u32).unwrap();
    st.set("inventory-count", 0u32).unwrap();
    st.set("inventory-max-items", INV_MAX).unwrap();
    st.set("inventory", lua.create_table().unwrap()).unwrap();

    // tile info for gather cost calculation.
    let tile = lua.create_table().unwrap();
    tile.set("x", COPPER_X).unwrap();
    tile.set("y", COPPER_Y).unwrap();
    tile.set("resource", "copper_ore").unwrap();
    tile.set("level", COPPER_LEVEL).unwrap();
    st.set("tile", tile).unwrap();

    st
}

// ─── Test 1: estimate pass ───────────────────────────────────────────────────

#[test]
fn test_estimate_pass() {
    let lua = make_estimate_lua();
    let wf = load_workflow(&lua);
    let st = make_model_state(&lua);

    let estimate_fn: LuaFunction = lua.globals().get("estimate").expect("estimate not found");
    let result: LuaTable = estimate_fn
        .call((wf, st))
        .expect("estimate call failed");

    let seconds: f64 = result.get("seconds").expect("missing seconds");
    let actions: u32 = result.get("actions").expect("missing actions");
    let bucket_cost: LuaTable = result.get("bucket-cost").expect("missing bucket-cost");
    let bucket_action: u32 = bucket_cost.get("action").unwrap_or(0);
    let assumptions: LuaTable = result.get("assumptions").expect("missing assumptions");
    let gathers: u32 = assumptions.get("gathers").unwrap_or(0);

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
        "gathers: expected {EXPECTED_GATHERS} (resolved by sim loop), got {gathers}"
    );
    assert_eq!(
        bucket_action, EXPECTED_BUCKET_ACTION,
        "bucket-cost.action: expected {EXPECTED_BUCKET_ACTION}, got {bucket_action}"
    );
}

// ─── Test 2: simulate pass ───────────────────────────────────────────────────

#[test]
fn test_simulate_pass() {
    let lua = make_estimate_lua();
    let wf = load_workflow(&lua);
    let st = make_model_state(&lua);

    let simulate_fn: LuaFunction = lua.globals().get("simulate").expect("simulate not found");
    let result: LuaTable = simulate_fn
        .call((wf, st, 1u32))
        .expect("simulate call failed");

    let feasible: bool = result.get("feasible").expect("missing feasible");
    let gathers: u32 = result.get("gathers").unwrap_or(0);

    assert!(feasible, "simulate: expected feasible=true");
    assert_eq!(
        gathers, EXPECTED_GATHERS,
        "simulate gathers: expected {EXPECTED_GATHERS}, got {gathers}"
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
        skin: None,
    };

    let shared_view = SharedView::new(initial_view);
    let (tx, rx) = mpsc::channel(32);
    let scheduler = Scheduler::new(Box::new(driver), rx, shared_view.clone());

    // Run scheduler on a background thread (it's blocking internally).
    let scheduler_handle = std::thread::spawn(move || scheduler.run());

    let char = Character::new(tx, shared_view.clone());

    // Run the workflow on the current thread (which acts as the "script thread").
    let lua = setup_lua(Some(char), Some(make_test_map())).expect("setup_lua with character failed");
    let wf = load_workflow(&lua);

    let run_fn: LuaFunction = lua.globals().get("run").expect("run fn not found");
    run_fn.call::<()>(wf).expect("workflow run failed");

    // After the workflow: character should be at BANK with empty inventory.
    let final_view = shared_view.get();
    assert_eq!(final_view.x, BANK_X, "final x should be BANK_X={BANK_X}");
    assert_eq!(final_view.y, BANK_Y, "final y should be BANK_Y={BANK_Y}");
    assert_eq!(
        final_view.inventory_count(),
        0,
        "inventory should be empty after deposit-all"
    );

    // Signal scheduler to shut down.
    drop(lua); // drops Character → drops tx → scheduler rx closes
    let _ = scheduler_handle.join();
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn make_char_json(x: i32, y: i32, inv_count: u32, inv_max: u32) -> serde_json::Value {
    // Build inventory as slots based on count.
    let mut inventory = vec![];
    if inv_count > 0 {
        inventory.push(serde_json::json!({
            "slot": 1,
            "code": "copper_ore",
            "quantity": inv_count
        }));
    }
    serde_json::json!({
        "name": "kael",
        "x": x,
        "y": y,
        "hp": 100,
        "max_hp": 100,
        "level": 1,
        "inventory_max_items": inv_max,
        "inventory": inventory
    })
}

fn make_response(cooldown_secs: f64, char_json: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "data": {
            "cooldown": {
                "total_seconds": cooldown_secs,
                "remaining_seconds": cooldown_secs,
                "started_at": "2024-01-01T00:00:00Z",
                "expiration": "2024-01-01T00:01:00Z",
                "reason": "action"
            },
            "character": char_json,
            "details": { "items": [] }
        }
    }))
    .unwrap()
}

fn build_canned_responses() -> Vec<CannedResponse> {
    let mut responses = Vec::new();

    // 1. travel-to COPPER: 2 tiles × 5s = 10s
    responses.push(CannedResponse::new(
        "action/move",
        200,
        make_response(10.0, make_char_json(COPPER_X, COPPER_Y, 0, INV_MAX)),
    ));

    // 2–11. gather ×10 (filling inventory one item per gather)
    for i in 1..=10u32 {
        responses.push(CannedResponse::new(
            "action/gathering",
            200,
            make_response(30.0, make_char_json(COPPER_X, COPPER_Y, i, INV_MAX)),
        ));
    }

    // 12. travel-to BANK: 3 tiles × 5s = 15s
    responses.push(CannedResponse::new(
        "action/move",
        200,
        make_response(15.0, make_char_json(BANK_X, BANK_Y, 10, INV_MAX)),
    ));

    // 13. deposit-all (expanded to deposit-item for copper_ore): 3s
    responses.push(CannedResponse::new(
        "action/bank/deposit/item",
        200,
        make_response(3.0, make_char_json(BANK_X, BANK_Y, 0, INV_MAX)),
    ));

    responses
}
