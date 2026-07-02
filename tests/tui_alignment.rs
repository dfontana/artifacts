//! Tier 2 — hermetic alignment (`plans/TUI.md` §7). Runs real Fennel through the
//! TUI mechanism (number-nodes → skeleton → host.progress → run) against
//! MockDriver, offline. Guards the identity-alignment claim (§3.1): every id the
//! run reports is literally an id the skeleton recorded, and the reducer folds
//! the captured log into a coherent all-done terminal frame.
//!
//! Covers both shapes: `farm-copper` (linear + a `repeat-until` loop) and
//! `farm-chickens` (the `when_pred` branch — the skip path).

use std::sync::Arc;

use artifacts::{
    character::Character,
    driver::mock::{CannedResponse, MockDriver},
    lua::{eval_fennel, setup_lua},
    scheduler::Scheduler,
    tui::reducer::{reduce, Cell, RunPhase},
    tui::skeleton::{marshal, PlanStep, StepKind},
    tui::{new_progress_log, NodeId},
    view::SharedView,
};
use artifacts_core::{
    combat::MonsterView,
    map::{AccessSchema, GameMap, InteractionSchema, MapAccessType, MapTile},
    step::CharacterView,
};
use mlua::prelude::*;
use tokio::sync::mpsc;

// ─── shared fixtures ─────────────────────────────────────────────────────────

const COPPER: (i32, i32) = (2, 0);
const BANK: (i32, i32) = (4, 1);
const CHICKEN: (i32, i32) = (1, 1);
const INV_MAX: u32 = 10;

/// A clear 6×2 overworld with copper/bank/chicken content tiles, enough for A*
/// hops and `host.find_tile` lookups.
fn make_map() -> Arc<GameMap> {
    let mut m = GameMap::new();
    for y in 0i32..2 {
        for x in 0i32..6 {
            let interactions = if (x, y) == COPPER {
                content("copper_rocks", "resource")
            } else if (x, y) == BANK {
                content("bank", "bank")
            } else if (x, y) == CHICKEN {
                content("chicken", "monster")
            } else {
                InteractionSchema::default()
            };
            m.insert(MapTile {
                map_id: y * 10 + x,
                name: format!("{x},{y}"),
                skin: "grass".into(),
                x,
                y,
                layer: "overworld".into(),
                access: AccessSchema {
                    access_type: MapAccessType::Standard,
                },
                interactions,
            });
        }
    }
    Arc::new(m)
}

fn content(code: &str, kind: &str) -> InteractionSchema {
    use artifacts_core::map::MapContentSchema;
    InteractionSchema {
        content: Some(MapContentSchema {
            content_type: kind.into(),
            code: code.into(),
        }),
        ..Default::default()
    }
}

fn char_json(x: i32, y: i32, inv_count: u32, hp: u32) -> serde_json::Value {
    let mut inventory = vec![];
    if inv_count > 0 {
        inventory.push(serde_json::json!({"slot": 1, "code": "copper_ore", "quantity": inv_count}));
    }
    serde_json::json!({
        "name": "kael", "x": x, "y": y, "hp": hp, "max_hp": 100, "level": 1,
        // A fire attack so `is_winnable` (and thus ¬need-rest) is true against the
        // stat-less chicken below — this is what exercises the when-SKIP path.
        "attack_fire": 50,
        "inventory_max_items": INV_MAX, "inventory": inventory
    })
}

fn response(cooldown: f64, character: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "data": {
            "cooldown": {
                "total_seconds": cooldown, "remaining_seconds": cooldown,
                "started_at": "2024-01-01T00:00:00Z", "expiration": "2024-01-01T00:01:00Z",
                "reason": "action"
            },
            "character": character,
            "details": { "items": [] }
        }
    }))
    .unwrap()
}

/// Stamp ids, capture the marshaled skeleton, run the workflow against the mock,
/// and return `(skeleton, fired id-log)`.
fn run_and_capture(
    workflow_src: &str,
    responses: Vec<CannedResponse>,
    monsters: Option<Arc<artifacts::data::MonsterData>>,
    initial_hp: u32,
) -> (Vec<PlanStep>, Vec<NodeId>) {
    let log = new_progress_log();

    let mut driver = MockDriver::new();
    driver.push_responses(responses);

    let initial = CharacterView {
        name: "kael".into(),
        x: 0,
        y: 0,
        hp: initial_hp,
        max_hp: 100,
        level: 1,
        attack_fire: 50,
        inventory_max_items: INV_MAX,
        inventory: vec![],
        ..Default::default()
    };
    let view = SharedView::new(initial);
    let (tx, rx) = mpsc::channel(32);
    let abort = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let scheduler = Scheduler::new(Box::new(driver), rx, view.clone(), abort);
    let handle = std::thread::spawn(move || scheduler.run());

    let character = Character::new(tx, view.clone());
    let lua = setup_lua(
        Some(character),
        Some(make_map()),
        monsters,
        Some(log.clone()),
    )
    .expect("setup_lua");
    let wf = eval_fennel(&lua, workflow_src, "wf.fnl").expect("eval workflow");

    // number-nodes → skeleton, on the SAME numbered table the run will read.
    let number_nodes: LuaFunction = lua.globals().get("number_nodes").unwrap();
    number_nodes.call::<LuaValue>(&wf).expect("number_nodes");
    let skeleton_fn: LuaFunction = lua.globals().get("skeleton").unwrap();
    let sk_tbl: LuaTable = skeleton_fn.call(&wf).expect("skeleton");
    let skeleton = marshal(&sk_tbl).expect("marshal skeleton");

    let run_fn: LuaFunction = lua.globals().get("run").unwrap();
    run_fn.call::<()>(&wf).expect("run");

    drop(lua);
    let _ = handle.join();

    let captured = log.lock().unwrap().clone();
    (skeleton, captured)
}

// ─── farm-copper: linear + loop ──────────────────────────────────────────────

#[test]
fn farm_copper_ids_align() {
    let mut responses = vec![CannedResponse::new(
        "action/move",
        200,
        response(10.0, char_json(COPPER.0, COPPER.1, 0, 100)),
    )];
    for i in 1..=INV_MAX {
        responses.push(CannedResponse::new(
            "action/gathering",
            200,
            response(30.0, char_json(COPPER.0, COPPER.1, i, 100)),
        ));
    }
    responses.push(CannedResponse::new(
        "action/move",
        200,
        response(15.0, char_json(BANK.0, BANK.1, INV_MAX, 100)),
    ));
    responses.push(CannedResponse::new(
        "action/bank/deposit/item",
        200,
        response(3.0, char_json(BANK.0, BANK.1, 0, 100)),
    ));

    let (skeleton, log) = run_and_capture(
        include_str!("../fennel/workflows/farm-copper.fnl"),
        responses,
        None,
        100,
    );

    assert_alignment(&skeleton, &log);

    // Structural sanity: travel, loop, gather, travel, deposit-all.
    let kinds: Vec<_> = skeleton.iter().map(|s| (s.kind, s.op.as_str())).collect();
    assert_eq!(
        kinds,
        vec![
            (StepKind::Action, "travel-to"),
            (StepKind::Loop, "repeat-until"),
            (StepKind::Action, "gather"),
            (StepKind::Action, "travel-to"),
            (StepKind::Action, "deposit-all"),
        ]
    );

    // The gather body node fired exactly INV_MAX times (the loop iteration count).
    let gather_id = skeleton[2].id;
    let gathers = log.iter().filter(|&&x| x == gather_id).count();
    assert_eq!(gathers, INV_MAX as usize, "gather fired once per iteration");

    // Terminal frame: everything done.
    let rows = reduce(&skeleton, &log, RunPhase::Done);
    assert!(
        rows.iter().all(|r| r.cell == Cell::Done),
        "a completed run leaves every row done: {rows:?}"
    );
}

// ─── farm-chickens: the when_pred branch ─────────────────────────────────────

#[test]
fn farm_chickens_ids_align() {
    // One monster: a chicken nillinbot always beats from full HP, so `need-rest`
    // (¬winnable) is false → the `when` body (rest) is always skipped.
    let chicken = MonsterView {
        code: "chicken".into(),
        name: "Chicken".into(),
        level: 1,
        hp: 30, // dies to one 50-fire hit; deals no damage, so we win from full HP
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
        drops: vec![],
    };
    let monsters = Arc::new(artifacts::data::MonsterData::from_vec(vec![chicken]));

    // travel to chicken, then two winning fights fill the inventory, then bank.
    let mut responses = vec![CannedResponse::new(
        "action/move",
        200,
        response(5.0, char_json(CHICKEN.0, CHICKEN.1, 0, 100)),
    )];
    for i in 1..=2u32 {
        responses.push(CannedResponse::new(
            "action/fight",
            200,
            fight_win(char_json(CHICKEN.0, CHICKEN.1, i * (INV_MAX / 2), 100)),
        ));
    }
    responses.push(CannedResponse::new(
        "action/move",
        200,
        response(15.0, char_json(BANK.0, BANK.1, INV_MAX, 100)),
    ));
    responses.push(CannedResponse::new(
        "action/bank/deposit/item",
        200,
        response(3.0, char_json(BANK.0, BANK.1, 0, 100)),
    ));

    let (skeleton, log) = run_and_capture(
        include_str!("../fennel/workflows/farm-chickens.fnl"),
        responses,
        Some(monsters),
        100,
    );

    assert_alignment(&skeleton, &log);

    // The when guard fired every iteration but rest never did → rest is guarded
    // and skipped in the terminal frame.
    let rest_row = skeleton
        .iter()
        .position(|s| s.op == "rest")
        .expect("a rest row");
    assert!(
        skeleton[rest_row].guard_id.is_some(),
        "rest carries its when's id as guard"
    );
    let rows = reduce(&skeleton, &log, RunPhase::Done);
    assert_eq!(
        rows[rest_row].cell,
        Cell::Skipped,
        "rest was guarded and never ran → skipped"
    );

    // The fight row (the when's sibling) did run every iteration.
    let fight_row = skeleton.iter().position(|s| s.op == "fight").unwrap();
    assert_eq!(rows[fight_row].cell, Cell::Done);
}

fn fight_win(character: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "data": {
            "cooldown": {
                "total_seconds": 10.0, "remaining_seconds": 10.0,
                "started_at": "2024-01-01T00:00:00Z", "expiration": "2024-01-01T00:01:00Z",
                "reason": "fight"
            },
            "character": character,
            "fight": {
                "xp": 10, "gold": 1, "drops": [], "turns": 1, "result": "win",
                "logs": [], "monster_blocked_hits": {}, "player_blocked_hits": {}
            }
        }
    }))
    .unwrap()
}

// ─── shared assertion ────────────────────────────────────────────────────────

/// The identity-alignment guarantee (§3.1). Forward: no fired id escapes the
/// numbered id space — every fired id is either a skeleton row or a structural
/// `:seq` node (which fires but renders as no row), both of which sit at or below
/// the last node's id. Reverse: every *unguarded* row on the executed path was
/// reached (a row under a `when` may legitimately be skipped).
fn assert_alignment(skeleton: &[PlanStep], log: &[NodeId]) {
    assert!(!log.is_empty(), "the run fired at least one progress id");
    // Visit-order ids are contiguous 0..=max and the last node (deposit-all here)
    // is always a row, so the skeleton's max id is the whole AST's max id.
    let max_id = skeleton.iter().map(|s| s.id).max().unwrap();
    for &fired in log {
        assert!(
            fired <= max_id,
            "fired id {fired} escapes the skeleton id space (max {max_id})"
        );
    }
    for s in skeleton {
        if s.guard_id.is_none() {
            assert!(
                log.contains(&s.id),
                "unguarded skeleton row {} (op {}) was never reached",
                s.id,
                s.op
            );
        }
    }
}
