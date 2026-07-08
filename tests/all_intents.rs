//! Cross-cutting proof for every newly-wired character-action intent
//! (plans/ALL_INTENTS.md). Each case drives a one-action Fennel workflow through
//! the whole stack — `setup_lua` → `interp.run` → `Character` → `Scheduler` →
//! `MockDriver` → `SharedView` — and asserts two things a scripter depends on:
//!
//!   1. the exact request the intent put on the wire (method + path + JSON body),
//!      captured by `MockDriver`'s request log; and
//!   2. that the action's response flowed back into the live view.
//!
//! One table-driven test covers all of them (the "few broad, entrypoint-driven"
//! standard) rather than 21 near-identical files. A second test exercises the
//! offline `plan` pass so the new `:cost` formulas are proven too.
use std::sync::Arc;

use artifacts::{
    data::RecipeData,
    driver::mock::{CannedResponse, MockDriver},
    lua::{eval_fennel, predicate_state, require_module, setup_lua, LuaSetupOptions},
};
use artifacts_core::{
    combat::CombatStats,
    recipe::{RecipeCraft, RecipeInput, RecipeView},
    step::{CharacterView, SkillLevels},
};
use mlua::prelude::*;
use serde_json::json;

mod common;
use common::{char_json, response, INV_MAX};

/// A single intent's end-to-end expectation: the Fennel action expression a
/// workflow author would write, and the request it must produce on the wire.
struct Case {
    /// The `(action ...)` s-expression, dropped into a `(seq ...)` workflow.
    action: &'static str,
    /// Path substring the POST must target.
    path: &'static str,
    /// Expected JSON request body, or `None` for a bodyless action.
    body: Option<serde_json::Value>,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            action: "(action :craft [:copper_dagger 2])",
            path: "action/crafting",
            body: Some(json!({"code": "copper_dagger", "quantity": 2})),
        },
        Case {
            action: "(action :recycle [:copper_dagger 1])",
            path: "action/recycling",
            body: Some(json!({"code": "copper_dagger", "quantity": 1})),
        },
        Case {
            action: "(action :use-item [:cooked_chicken 1])",
            path: "action/use",
            body: Some(json!({"code": "cooked_chicken", "quantity": 1})),
        },
        Case {
            action: "(action :delete-item [:copper_ore 5])",
            path: "action/delete",
            body: Some(json!({"code": "copper_ore", "quantity": 5})),
        },
        Case {
            action: "(action :equip [:copper_dagger :weapon])",
            path: "action/equip",
            body: Some(json!([{"code": "copper_dagger", "slot": "weapon", "quantity": 1}])),
        },
        Case {
            action: "(action :unequip [:weapon])",
            path: "action/unequip",
            body: Some(json!([{"slot": "weapon", "quantity": 1}])),
        },
        Case {
            action: "(action :deposit-gold 100)",
            path: "action/bank/deposit/gold",
            body: Some(json!({"quantity": 100})),
        },
        Case {
            action: "(action :withdraw-gold 50)",
            path: "action/bank/withdraw/gold",
            body: Some(json!({"quantity": 50})),
        },
        Case {
            action: "(action :give-gold [25 :ally])",
            path: "action/give/gold",
            body: Some(json!({"quantity": 25, "character": "ally"})),
        },
        Case {
            action: "(action :give-item [:copper_ore 3 :ally])",
            path: "action/give/item",
            body: Some(
                json!({"items": [{"code": "copper_ore", "quantity": 3}], "character": "ally"}),
            ),
        },
        Case {
            action: "(action :npc-buy [:apple 4])",
            path: "action/npc/buy",
            body: Some(json!({"code": "apple", "quantity": 4})),
        },
        Case {
            action: "(action :npc-sell [:apple 4])",
            path: "action/npc/sell",
            body: Some(json!({"code": "apple", "quantity": 4})),
        },
        Case {
            action: "(action :ge-buy [:order-123 2])",
            path: "action/grandexchange/buy",
            body: Some(json!({"id": "order-123", "quantity": 2})),
        },
        Case {
            action: "(action :ge-cancel :order-123)",
            path: "action/grandexchange/cancel",
            body: Some(json!({"id": "order-123"})),
        },
        Case {
            action: "(action :ge-fill [:order-123 2])",
            path: "action/grandexchange/fill",
            body: Some(json!({"id": "order-123", "quantity": 2})),
        },
        Case {
            action: "(action :task-new)",
            path: "action/task/new",
            body: None,
        },
        Case {
            action: "(action :task-complete)",
            path: "action/task/complete",
            body: None,
        },
        Case {
            action: "(action :task-cancel)",
            path: "action/task/cancel",
            body: None,
        },
        Case {
            action: "(action :task-exchange)",
            path: "action/task/exchange",
            body: None,
        },
        Case {
            action: "(action :task-trade [:copper_ore 3])",
            path: "action/task/trade",
            body: Some(json!({"code": "copper_ore", "quantity": 3})),
        },
        Case {
            action: "(action :transition)",
            path: "action/transition",
            body: None,
        },
    ]
}

#[test]
fn test_every_new_intent_runs_end_to_end() {
    for case in cases() {
        // A driver that answers any action with the same character snapshot,
        // parked at a distinctive (7, 8) so we can prove the outcome refreshed
        // the live view rather than leaving the initial (0, 0) in place.
        let mut driver = MockDriver::new();
        driver.push_response(CannedResponse::new(
            "action/",
            200,
            response(3.0, char_json(7, 8, 0, 100)),
        ));
        let log = driver.request_log();

        let initial = CharacterView {
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
        // A workflow MODULE wrapping the single action under test: `build`
        // ignores params/ctx and returns the AST run_workflow executes.
        let src = format!(
            "(local {{: seq : action}} (require :fennel.lib.interp))\n\
             {{:build (fn [_ _] (seq {}))}}",
            case.action
        );
        let final_view = artifacts::live::run_workflow(
            Box::new(driver),
            &src,
            artifacts::character::SharedView::new(initial),
            None,
            None,
            None,
            None,
            &[],
            artifacts::live::RunOptions::default(),
        )
        .unwrap_or_else(|e| panic!("run failed for `{}`: {e}", case.action));

        // 1. The outcome flowed back into the live view.
        assert_eq!(
            (final_view.x, final_view.y),
            (7, 8),
            "`{}` should refresh the live view from the action response",
            case.action
        );

        // 2. Exactly one request went out, with the expected wire format.
        let reqs = log.lock().expect("request log poisoned");
        assert_eq!(
            reqs.len(),
            1,
            "`{}` should send exactly one request, sent {}",
            case.action,
            reqs.len()
        );
        let req = &reqs[0];
        assert!(
            req.path.contains(case.path),
            "`{}` should POST to `{}`, went to `{}`",
            case.action,
            case.path,
            req.path
        );
        match (&case.body, &req.body) {
            (Some(expected), Some(raw)) => {
                let got: serde_json::Value =
                    serde_json::from_slice(raw).expect("request body not valid JSON");
                assert_eq!(&got, expected, "`{}` sent an unexpected body", case.action);
            }
            (None, None) => {}
            (expected, got) => panic!(
                "`{}` body mismatch: expected {expected:?}, sent {}",
                case.action,
                got.as_ref()
                    .map(|b| String::from_utf8_lossy(b).into_owned())
                    .unwrap_or_else(|| "<none>".into())
            ),
        }
        drop(reqs);
    }
}

/// The offline `plan` pass over a workflow of new actions: proves the `:cost`
/// formulas (`craft` = 5s/item, `recycle` = 3s/item, the flat-3s `simple`
/// family) are wired through `host.cooldown_cost` and summed by the interpreter.
#[test]
fn test_new_action_plan_costs() {
    let lua = setup_lua(LuaSetupOptions {
        recipes: Some(Arc::new(craft_recipes())),
        ..Default::default()
    })
    .expect("setup_lua failed");
    let src = "(local {: seq : action} (require :fennel.lib.interp))\n\
        (seq (action :craft [:copper_dagger 2])\n\
             (action :recycle [:iron_ore 3])\n\
             (action :use-item [:cooked_chicken 1])\n\
             (action :transition))";
    let wf = eval_fennel(&lua, src, "plan.fnl").expect("failed to load workflow");

    // weaponcrafting >= 1 so the craft skill gate passes (copper_dagger needs
    // weaponcrafting 1); predicate_state builds :skills and :inventory itself.
    let skills = SkillLevels {
        weaponcrafting: 1,
        ..Default::default()
    };
    let st = predicate_state(
        &lua,
        0,
        0,
        100,
        100,
        0,
        INV_MAX,
        0,
        &CombatStats::default(),
        &skills,
        &[],
    )
    .expect("predicate_state failed");

    let interp = require_module(&lua, "fennel.lib.interp").expect("require interp");
    let plan_fn: LuaFunction = interp.get("plan").expect("plan not found");
    let result: LuaTable = plan_fn.call((wf, st)).expect("plan call failed");

    let seconds: f64 = result.get("seconds").expect("missing seconds");
    let actions: u32 = result.get("actions").expect("missing actions");
    let feasible: bool = result.get("feasible").expect("missing feasible");

    // craft(2)=10 + recycle(3)=9 + use(simple)=3 + transition(simple)=3 = 25s.
    assert!((seconds - 25.0).abs() < 0.01, "expected 25s, got {seconds}");
    assert_eq!(actions, 4);
    assert!(feasible);
}

/// A one-recipe fixture: `copper_dagger` costs 6 `copper` per craft and yields 1.
/// Enough to drive `host.recipe` / the `craft` `:sim` without a live `/items`.
fn craft_recipes() -> RecipeData {
    RecipeData::from_items(vec![RecipeView {
        code: "copper_dagger".into(),
        craft: Some(RecipeCraft {
            skill: Some("weaponcrafting".into()),
            level: Some(1),
            items: vec![RecipeInput {
                code: "copper".into(),
                quantity: 6,
            }],
            quantity: 1,
        }),
    }])
}

/// The `craft` `:sim` consumes the real recipe inputs and adds the crafted
/// output (the reference-data payoff), driven through the `fennel.lib.actions`
/// table the interpreter uses. Crafting 2 `copper_dagger` (6 copper each) out of
/// a pack of 20 copper leaves 8 copper + 2 daggers, and moves the item count by
/// the net (−12 consumed, +2 crafted).
#[test]
fn test_craft_sim_consumes_recipe_inputs() {
    let lua = setup_lua(LuaSetupOptions {
        recipes: Some(Arc::new(craft_recipes())),
        ..Default::default()
    })
    .expect("setup_lua failed");

    let src = "(local {: actions} (require :fennel.lib.actions))\n\
        (local craft (. actions :craft))\n\
        (local st {:inventory {:copper 20} :inventory-count 20\n\
                   :skills {:weaponcrafting 1}})\n\
        (local out (craft.sim st [:copper_dagger 2]))\n\
        {:copper (or (. out.inventory :copper) 0)\n\
         :dagger (or (. out.inventory :copper_dagger) 0)\n\
         :count out.inventory-count}";
    let out: LuaTable = eval_fennel(&lua, src, "craft_sim.fnl")
        .and_then(|v| LuaTable::from_lua(v, &lua))
        .expect("craft sim eval");

    assert_eq!(out.get::<u32>("copper").unwrap(), 8, "12 copper consumed");
    assert_eq!(out.get::<u32>("dagger").unwrap(), 2, "2 daggers crafted");
    assert_eq!(
        out.get::<u32>("count").unwrap(),
        10,
        "net item count 20-12+2"
    );
}

/// The Grand Exchange `:sim`s move gold + inventory from the author-supplied
/// `code`/`price` hints (the npc-buy pattern; no order-book cache). ge-buy 2
/// copper @5 spends 10 gold and adds the copper; ge-fill (sell) 1 copper @8
/// earns 8 gold and removes it. No recipe/reference data needed.
#[test]
fn test_ge_sim_uses_price_hint() {
    let lua = setup_lua(LuaSetupOptions::default()).expect("setup_lua failed");
    let src = "(local {: actions} (require :fennel.lib.actions))\n\
        (local buy (. actions :ge-buy))\n\
        (local fill (. actions :ge-fill))\n\
        (local st {:inventory {} :inventory-count 0 :gold 100})\n\
        (local after-buy (buy.sim st [:order-1 2 :copper 5]))\n\
        (local after-fill (fill.sim after-buy [:order-2 1 :copper 8]))\n\
        {:buy-gold after-buy.gold\n\
         :buy-copper (or (. after-buy.inventory :copper) 0)\n\
         :fill-gold after-fill.gold\n\
         :fill-copper (or (. after-fill.inventory :copper) 0)}";
    let out: LuaTable = eval_fennel(&lua, src, "ge_sim.fnl")
        .and_then(|v| LuaTable::from_lua(v, &lua))
        .expect("ge sim eval");

    assert_eq!(out.get::<u32>("buy-gold").unwrap(), 90, "100 - 2*5 spent");
    assert_eq!(out.get::<u32>("buy-copper").unwrap(), 2, "2 copper bought");
    assert_eq!(out.get::<u32>("fill-gold").unwrap(), 98, "90 + 1*8 earned");
    assert_eq!(out.get::<u32>("fill-copper").unwrap(), 1, "1 copper sold");
}
