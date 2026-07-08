//! M3 model-surface tests (`plans/DYNAMIC_WORKFLOWS.md` §5.1–5.5, §5.7).
//!
//! Covers the model-surface fixes/extensions this milestone lands, driven
//! through the public seams the standard prefers (`planner::plan`,
//! `live::run_workflow`, the `host`/predicate surface via `setup_lua`) rather
//! than per-function micro-tests:
//!   - skill-gated gather / craft produce a plan blocker;
//!   - a `has_item` loop terminates with the right count in plan AND evaluates
//!     correctly against a live-shaped view (MockDriver run);
//!   - `skill_at_least` works in plan and errors clearly on an unknown skill;
//!   - `find_tile`'s explicit anchor overrides the default anchor;
//!   - `NpcItemData` groups merchant listings by item code.

use std::sync::Arc;

use artifacts::{
    data::{NpcItemData, RecipeData},
    driver::mock::{CannedResponse, MockDriver},
    lua::{eval_fennel, predicate_state, require_module, setup_lua, LuaSetupOptions},
    planner::{self, PlanSeed},
};
use artifacts_core::{
    combat::CombatStats,
    npc::NpcItemView,
    recipe::{RecipeCraft, RecipeInput, RecipeView},
    step::{CharacterView, SkillLevels},
};
use mlua::prelude::*;

mod common;
use common::{char_json, make_map, make_resources, response};

// ─── helpers ─────────────────────────────────────────────────────────────────

/// Run the `plan` pass over a bare-AST `src` against `st`, returning the result
/// table (the same low-level path `farm_copper.rs` uses).
fn plan_bare(lua: &Lua, src: &str, st: LuaTable) -> LuaTable {
    let wf = eval_fennel(lua, src, "wf.fnl").expect("eval workflow");
    let interp = require_module(lua, "fennel.lib.interp").expect("require interp");
    let plan_fn: LuaFunction = interp.get("plan").expect("plan fn");
    plan_fn.call((wf, st)).expect("plan call")
}

/// A model state at `(x, y)`, big inventory cap, with the given skills and no
/// inventory contents — through the single `predicate_state` surface.
fn state_with_skills(lua: &Lua, x: i32, y: i32, skills: &SkillLevels) -> LuaTable {
    predicate_state(
        lua,
        x,
        y,
        100,
        100,
        0,
        100,
        0,
        &CombatStats::default(),
        skills,
        &[],
    )
    .expect("predicate_state")
}

fn blockers(result: &LuaTable) -> Vec<String> {
    result
        .get::<LuaTable>("blockers")
        .map(|t| t.sequence_values::<String>().flatten().collect())
        .unwrap_or_default()
}

// ─── skill-gated gather ──────────────────────────────────────────────────────

#[test]
fn gather_above_skill_is_a_plan_blocker() {
    // A level-10 mining resource, but the character has mining 1: gathering it is
    // genuinely infeasible, so the plan must flag it (not silently predict cost).
    let lua = setup_lua(LuaSetupOptions {
        map: Some(make_map(2, 2, &[(0, 0, "resource", "iron_rocks")])),
        resources: Some(make_resources(&[("iron_rocks", "mining", 10, "iron_ore")])),
        ..Default::default()
    })
    .expect("setup_lua");

    let st = state_with_skills(
        &lua,
        0,
        0,
        &SkillLevels {
            mining: 1,
            ..Default::default()
        },
    );
    let result = plan_bare(
        &lua,
        "(local {: seq : action} (require :fennel.lib.interp))\n\
                                  (seq (action :gather))",
        st,
    );

    let feasible: bool = result.get("feasible").expect("feasible");
    assert!(!feasible, "gathering above skill must be infeasible");
    let bs = blockers(&result);
    assert!(
        bs.iter()
            .any(|b| b.contains("gathering iron_rocks needs mining 10") && b.contains("have 1")),
        "expected a mining skill-gate blocker, got: {bs:?}"
    );
}

// ─── skill-gated craft ───────────────────────────────────────────────────────

/// A `copper_dagger` recipe requiring weaponcrafting 5 — enough to drive the
/// craft `:sim`'s skill gate via `host.recipe`.
fn dagger_recipes() -> RecipeData {
    RecipeData::from_items(vec![RecipeView {
        code: "copper_dagger".into(),
        craft: Some(RecipeCraft {
            skill: Some("weaponcrafting".into()),
            level: Some(5),
            items: vec![RecipeInput {
                code: "copper".into(),
                quantity: 6,
            }],
            quantity: 1,
        }),
    }])
}

#[test]
fn craft_above_skill_is_a_plan_blocker() {
    let lua = setup_lua(LuaSetupOptions {
        recipes: Some(Arc::new(dagger_recipes())),
        ..Default::default()
    })
    .expect("setup_lua");

    // weaponcrafting 1, recipe needs 5.
    let st = state_with_skills(
        &lua,
        0,
        0,
        &SkillLevels {
            weaponcrafting: 1,
            ..Default::default()
        },
    );
    let result = plan_bare(
        &lua,
        "(local {: seq : action} (require :fennel.lib.interp))\n\
                                  (seq (action :craft [:copper_dagger 1]))",
        st,
    );

    let feasible: bool = result.get("feasible").expect("feasible");
    assert!(!feasible, "crafting above skill must be infeasible");
    let bs = blockers(&result);
    assert!(
        bs.iter().any(
            |b| b.contains("crafting copper_dagger needs weaponcrafting 5") && b.contains("have 1")
        ),
        "expected a weaponcrafting skill-gate blocker, got: {bs:?}"
    );
}

// ─── has_item loop: terminates in plan AND on a live-shaped view ──────────────

/// "Gather until inventory holds 3 copper_ore." Depends on the gather-drop fix
/// (§5.1: gather adds copper_ore) and inventory being on the live surface (§5.3).
const GATHER_UNTIL_3: &str = "\
(local {: seq : action : repeat_until} (require :fennel.lib.interp))\n\
(local {: has_item} (require :fennel.lib.predicates))\n\
{:build (fn [_ _]\n\
          (seq (repeat_until (fn [st] (has_item :copper_ore 3 st)) :gathers\n\
                 (action :gather))))}";

#[test]
fn has_item_loop_terminates_in_plan_at_the_right_count() {
    // Standing on a copper tile at (0,0); each gather yields 1 copper_ore, so the
    // loop resolves to exactly 3 iterations.
    let seed = PlanSeed {
        inventory_max_items: 100,
        skills: SkillLevels {
            mining: 1,
            ..Default::default()
        },
        ..PlanSeed::default()
    };
    let result = planner::plan(
        GATHER_UNTIL_3,
        Some(make_map(2, 2, &[(0, 0, "resource", "copper_rocks")])),
        None,
        Some(make_resources(&[(
            "copper_rocks",
            "mining",
            1,
            "copper_ore",
        )])),
        None,
        None,
        &seed,
        &[],
    )
    .expect("plan should succeed");

    assert!(
        result.feasible,
        "has_item loop should be feasible: {result:?}"
    );
    assert_eq!(
        result.assumptions,
        vec![("gathers".to_string(), 3)],
        "gather-until-3-copper_ore resolves to 3 iterations"
    );
}

#[test]
fn has_item_loop_terminates_on_a_live_view() {
    // The run pass evaluates the same predicate against host.view each iteration.
    // Three canned gathers land 1/2/3 copper_ore; the loop exits once the live
    // view shows 3 — proving inventory-contents predicates work identically in
    // plan and run (the §5.3 fix: host.view now carries :inventory).
    let mut driver = MockDriver::new();
    for i in 1..=3u32 {
        driver.push_response(CannedResponse::new(
            "action/gathering",
            200,
            response(30.0, char_json(0, 0, i, 100)),
        ));
    }

    let initial = CharacterView {
        name: "kael".into(),
        x: 0,
        y: 0,
        hp: 100,
        max_hp: 100,
        level: 1,
        inventory_max_items: 100,
        inventory: vec![],
        ..Default::default()
    };
    let final_view = artifacts::live::run_workflow(
        Box::new(driver),
        GATHER_UNTIL_3,
        artifacts::character::SharedView::new(initial),
        None,
        None,
        None,
        None,
        None,
        &[],
        artifacts::live::RunOptions::default(),
    )
    .expect("run_workflow");

    assert_eq!(
        final_view.inventory_count(),
        3,
        "the loop stops exactly when the live view holds 3 copper_ore"
    );
}

// ─── skill_at_least ──────────────────────────────────────────────────────────

#[test]
fn skill_at_least_reads_the_surface_and_rejects_typos() {
    let lua = setup_lua(LuaSetupOptions::default()).expect("setup_lua");
    let preds = require_module(&lua, "fennel.lib.predicates").expect("require predicates");
    let skill_at_least: LuaFunction = preds.get("skill_at_least").expect("skill_at_least");

    let st = state_with_skills(
        &lua,
        0,
        0,
        &SkillLevels {
            mining: 5,
            ..Default::default()
        },
    );

    let ok: bool = skill_at_least
        .call(("mining", 5u32, &st))
        .expect("mining 5 >= 5");
    assert!(ok, "mining 5 satisfies >= 5");
    let no: bool = skill_at_least
        .call(("mining", 6u32, &st))
        .expect("mining 5 < 6");
    assert!(!no, "mining 5 does not satisfy >= 6");

    // A typo'd skill name must error loudly rather than reading as 0 (which would
    // silently pass a `>= 0` check or mis-fail a real gate).
    let err = skill_at_least
        .call::<bool>(("mning", 1u32, &st))
        .expect_err("a typo'd skill must error");
    assert!(
        err.to_string().contains("unknown skill 'mning'"),
        "unknown skill should error clearly, got: {err}"
    );
}

// ─── find_tile explicit anchor (§5.7) ────────────────────────────────────────

#[test]
fn find_tile_explicit_anchor_overrides_default() {
    // Two banks at opposite ends. With no anchor the default (0,0) resolves the
    // near one; an explicit anchor near the FAR one flips the result.
    const NEAR: (i32, i32) = (1, 0);
    const FAR: (i32, i32) = (5, 1);
    let lua = setup_lua(LuaSetupOptions {
        map: Some(make_map(
            7,
            2,
            &[
                (NEAR.0, NEAR.1, "bank", "bank"),
                (FAR.0, FAR.1, "bank", "bank"),
            ],
        )),
        ..Default::default()
    })
    .expect("setup_lua");

    let read = |src: &str| -> (i32, i32) {
        let v = eval_fennel(&lua, src, "find.fnl").expect("eval find_tile");
        let t = LuaTable::from_lua(v, &lua).expect("table");
        (t.get("x").unwrap(), t.get("y").unwrap())
    };

    // No anchor: default (0,0) origin → the near bank.
    assert_eq!(read("(host.find_tile :bank :bank)"), NEAR);
    // Explicit anchor near the far bank → the far bank (overrides the default).
    assert_eq!(
        read("(host.find_tile :bank :bank {:x 6 :y 0})"),
        FAR,
        "explicit anchor must override the default (0,0) origin"
    );
}

// ─── NpcItemData (data-only groundwork, §5.5) ────────────────────────────────

#[test]
fn npc_item_data_groups_merchants_by_item() {
    let data = NpcItemData::from_vec(vec![
        NpcItemView {
            code: "ash_plank".into(),
            npc: "carpenter".into(),
            currency: "gold".into(),
            buy_price: Some(10),
            sell_price: Some(3),
        },
        NpcItemView {
            code: "ash_plank".into(),
            npc: "trader".into(),
            currency: "gold".into(),
            buy_price: Some(12),
            sell_price: None,
        },
        NpcItemView {
            code: "iron_bar".into(),
            npc: "smith".into(),
            currency: "gold".into(),
            buy_price: Some(20),
            sell_price: None,
        },
    ]);

    let ash = data.get(&"ash_plank".into()).expect("ash_plank is sold");
    assert_eq!(
        ash.len(),
        2,
        "both merchants listing ash_plank are returned"
    );
    let npcs: Vec<&str> = ash.iter().map(|m| m.npc.as_str()).collect();
    assert!(npcs.contains(&"carpenter") && npcs.contains(&"trader"));

    assert_eq!(data.get(&"iron_bar".into()).map(<[_]>::len), Some(1));
    assert!(
        data.get(&"nonexistent".into()).is_none(),
        "an unsourced item returns None"
    );
}
