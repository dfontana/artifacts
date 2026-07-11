//! Hermetic tests for the workflow-module protocol + parameter coercion (M1).
//!
//! Drives the public entry points — `workflow::load`, `workflow::schema`, and
//! `planner::plan` — rather than the Fennel `fennel.lib.params` fns directly:
//! `load` is the single seam every caller (CLI, TUI, composing workflow) goes
//! through, so exercising it is what proves coercion/validation reach `build`
//! and that the errors carry the declared-param help.

use artifacts::{
    lua::{setup_lua, LuaSetupOptions},
    planner::{self, PlanSeed},
    workflow::{self, ParamType},
};
use artifacts_core::step::SkillLevels;
use mlua::prelude::*;

mod common;
use common::{make_map, make_resources, INV_MAX};

/// A module that echoes its coerced params straight back out of `build`, so a
/// test can read exactly what reached it. Covers all three coercing types plus a
/// required game-code string, defaults, docs, and a `:doc`.
const ECHO: &str = "\
(local {: seq : action} (require :fennel.lib.interp))\n\
{:doc \"echo the coerced params\"\n\
 :params {:qty {:type :number :default 2 :doc \"how many\"}\n\
          :flag {:type :bool :default false}\n\
          :mode {:type :enum :options [:fast :slow] :default :fast}\n\
          :target {:type :resource :required true :doc \"what to gather\"}}\n\
 :build (fn [p _] p)}";

fn state() -> Lua {
    setup_lua(LuaSetupOptions::default()).expect("setup_lua")
}

/// Load `src` through the module protocol with string params, returning either
/// the (echoed) coerced table or the error.
fn load(lua: &Lua, src: &str, params: &[(&str, &str)]) -> anyhow::Result<Option<LuaValue>> {
    let owned: Vec<(String, String)> = params
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let ctx = lua.create_table().expect("ctx table");
    workflow::load(lua, src, "test.fnl", &owned, ctx)
}

fn as_table(lua: &Lua, v: Option<LuaValue>) -> LuaTable {
    LuaTable::from_lua(v.expect("build returned nil"), lua).expect("build returned a table")
}

// ─── coercion + defaults ─────────────────────────────────────────────────────

#[test]
fn coerces_supplied_values_and_applies_defaults() {
    let lua = state();

    // All supplied: string→number, string→bool, enum membership, game-code string.
    let p = as_table(
        &lua,
        load(
            &lua,
            ECHO,
            &[
                ("target", "copper_rocks"),
                ("qty", "5"),
                ("flag", "true"),
                ("mode", "slow"),
            ],
        )
        .expect("valid params load"),
    );
    assert_eq!(p.get::<i64>("qty").unwrap(), 5, "\"5\" coerces to number 5");
    assert!(p.get::<bool>("flag").unwrap(), "\"true\" coerces to bool");
    assert_eq!(p.get::<String>("mode").unwrap(), "slow");
    assert_eq!(
        p.get::<String>("target").unwrap(),
        "copper_rocks",
        "game-code types stay strings"
    );

    // Only the required param: the three defaults fill in.
    let d = as_table(&lua, load(&lua, ECHO, &[("target", "iron_rocks")]).unwrap());
    assert_eq!(d.get::<i64>("qty").unwrap(), 2, "default number applied");
    assert!(!d.get::<bool>("flag").unwrap(), "default bool applied");
    assert_eq!(
        d.get::<String>("mode").unwrap(),
        "fast",
        "default enum applied"
    );
}

#[test]
fn defaults_are_coerced_like_supplied_values() {
    let lua = state();

    // A default authored as a STRING for a :number param coerces on application,
    // exactly as a supplied "10" would.
    let string_default = "{:params {:qty {:type :number :default \"10\"}} :build (fn [p _] p)}";
    let p = as_table(&lua, load(&lua, string_default, &[]).unwrap());
    assert_eq!(
        p.get::<i64>("qty").unwrap(),
        10,
        "string-authored :number default coerces"
    );

    // A default OUTSIDE an enum's :options is an author typo: it must fail
    // loudly at load (with the declared-param help), never reach `build`.
    let bad_enum_default = "{:params {:mode {:type :enum :options [:fast :slow] \
                            :default :sideways}} :build (fn [p _] p)}";
    let err = load(&lua, bad_enum_default, &[]).expect_err("bad default rejected");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("is not one of") && msg.contains("declared params:"),
        "bad enum default should fail with membership error + help, got: {msg}"
    );
}

// ─── validation errors (each lists the declared params) ──────────────────────

#[test]
fn rejects_bad_params_with_help() {
    let lua = state();

    let cases: &[(&[(&str, &str)], &str)] = &[
        (&[], "missing required param 'target'"),
        (
            &[("target", "copper_rocks"), ("nope", "1")],
            "unknown param 'nope'",
        ),
        (
            &[("target", "copper_rocks"), ("qty", "abc")],
            "expected a number",
        ),
        (
            &[("target", "copper_rocks"), ("mode", "sideways")],
            "is not one of",
        ),
        (
            &[("target", "copper_rocks"), ("flag", "yes")],
            "expected 'true' or 'false'",
        ),
    ];

    for (params, needle) in cases {
        let err = load(&lua, ECHO, params).expect_err("should reject");
        let msg = format!("{err:#}");
        assert!(
            msg.contains(needle),
            "error for {params:?} should mention {needle:?}, got: {msg}"
        );
        // Every coercion error doubles as the per-workflow help.
        assert!(
            msg.contains("declared params:") && msg.contains("target"),
            "error for {params:?} should list the declared params, got: {msg}"
        );
    }
}

// ─── bare-AST files are rejected with prescriptive guidance ──────────────────

#[test]
fn bare_ast_is_rejected() {
    let lua = state();
    let bare = "(local {: seq : action} (require :fennel.lib.interp))\n\
                (seq (action :rest))";
    let err = load(&lua, bare, &[]).expect_err("a bare AST must be rejected");
    let msg = format!("{err:#}");
    assert!(
        msg.contains(":build") && msg.contains("wrap"),
        "bare-AST error should tell the author to wrap in {{:build ...}}, got: {msg}"
    );
}

// ─── schema() marshals :doc + :params without building ───────────────────────

#[test]
fn schema_marshals_declared_params() {
    let lua = state();
    let info = workflow::schema(&lua, ECHO, "echo.fnl").expect("schema marshals");

    assert_eq!(info.doc.as_deref(), Some("echo the coerced params"));

    // Sorted by name: flag, mode, qty, target.
    let names: Vec<&str> = info.params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["flag", "mode", "qty", "target"]);

    let by = |n: &str| info.params.iter().find(|p| p.name == n).unwrap();

    let target = by("target");
    assert_eq!(target.ptype, ParamType::Resource);
    assert!(target.required);
    assert_eq!(target.default, None);
    assert_eq!(target.doc.as_deref(), Some("what to gather"));

    let qty = by("qty");
    assert_eq!(qty.ptype, ParamType::Number);
    assert!(!qty.required);
    assert_eq!(qty.default.as_deref(), Some("2"));

    let mode = by("mode");
    assert_eq!(mode.ptype, ParamType::Enum);
    assert_eq!(mode.options, vec!["fast".to_string(), "slow".to_string()]);
}

// ─── a parameterized workflow plans end-to-end via planner::plan ─────────────

#[test]
fn parameterized_farm_plans_through_planner() {
    // copper_rocks tile at (2,0), bank at (4,1) — the farm.fnl `build` resolves
    // both via host.find_tile against this fixture map.
    let map = make_map(
        5,
        2,
        &[(2, 0, "resource", "copper_rocks"), (4, 1, "bank", "bank")],
    );
    let resources = make_resources(&[("copper_rocks", "mining", 1, "copper_ore")]);
    let seed = PlanSeed {
        inventory_max_items: INV_MAX,
        // mining >= 1 so farm's gather of copper_rocks (mining 1) isn't gated.
        skills: SkillLevels {
            mining: 1,
            ..Default::default()
        },
        ..PlanSeed::default()
    };

    let context = artifacts::context::ExecutionContext {
        map: Some(map),
        resources: Some(resources),
        ..Default::default()
    };
    let result = planner::plan(
        include_str!("../fennel/workflows/farm.fnl"),
        &context,
        &seed,
        &[("target".to_string(), "copper_rocks".to_string())],
    )
    .expect("farm.fnl should plan with target=copper_rocks");

    assert!(result.feasible, "farm plan should be feasible: {result:?}");
    // travel + gather×INV_MAX + travel + deposit-all.
    assert_eq!(result.actions, INV_MAX + 3);
    assert_eq!(
        result.assumptions,
        vec![("gathers".to_string(), INV_MAX)],
        "the gather loop resolves to a full inventory"
    );
}

// ─── the missing-required-param error is what a required game-code surfaces ───

#[test]
fn required_resource_param_is_reported_by_planner() {
    // No `target` at all: planner::plan should bubble the coercion error, which
    // names the missing param and lists the declared ones.
    let map = make_map(3, 2, &[(1, 0, "resource", "copper_rocks")]);
    let context = artifacts::context::ExecutionContext {
        map: Some(map),
        resources: Some(make_resources(&[(
            "copper_rocks",
            "mining",
            1,
            "copper_ore",
        )])),
        ..Default::default()
    };
    let err = planner::plan(
        include_str!("../fennel/workflows/farm.fnl"),
        &context,
        &PlanSeed::default(),
        &[],
    )
    .expect_err("farm.fnl with no target must error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("missing required param 'target'"),
        "planner should surface the missing-param error, got: {msg}"
    );
}
