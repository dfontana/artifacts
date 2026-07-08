//! Planning helper: run the `plan` Fennel pass and return a plain Rust
//! struct, keeping `mlua` types out of callers (e.g. the CLI). Seeding the pass
//! from a live character's state (`PlanSeed::from_view`) makes the prediction
//! specific to that character rather than a generic best case.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use mlua::prelude::*;

use artifacts_core::combat::CombatStats;
use artifacts_core::ident::Code;
use artifacts_core::map::GameMap;
use artifacts_core::step::{CharacterView, SkillLevels};

use crate::data::{MonsterData, RecipeData, ResourceData};
use crate::lua::{predicate_state, require_module, setup_lua, LuaSetupOptions};
use crate::workflow;

/// Seed state for a planning pass. The Fennel model state is built from this.
/// `PartialEq` so callers that re-plan frequently (the TUI) can skip a re-plan
/// when the seed hasn't changed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlanSeed {
    pub x: i32,
    pub y: i32,
    pub hp: u32,
    pub max_hp: u32,
    pub inventory_count: u32,
    pub inventory_max_items: u32,
    /// The character's current gold, so the plan can decrement it on buys/
    /// deposits and check gold predicates (`gold_at_least`).
    pub gold: u32,
    /// The character's combat stats, for the fight `:cost`/`:sim` and `winnable?`.
    pub combat: CombatStats,
    /// The character's skill levels, for the gather/craft skill gates and
    /// `skill_at_least`. On the state surface as `st.skills.*`.
    pub skills: SkillLevels,
    /// The character's inventory contents as `(code, qty)` pairs (duplicate slots
    /// summed, empties skipped), so `has_item`-style predicates and the plan's
    /// inventory prediction start from what the character really holds — the
    /// inventory-on-the-surface fix (`DYNAMIC_WORKFLOWS` §5.3).
    pub inventory: Vec<(Code, u32)>,
}

impl PlanSeed {
    /// Seed from a live character snapshot
    pub fn from_view(v: &CharacterView) -> Self {
        Self {
            x: v.x,
            y: v.y,
            hp: v.hp,
            max_hp: v.max_hp,
            inventory_count: v.inventory_count(),
            inventory_max_items: v.inventory_max_items,
            gold: v.gold,
            combat: CombatStats::from(v),
            skills: SkillLevels::from(v),
            inventory: v.inventory_pairs(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PlanResult {
    pub seconds: f64,
    pub actions: u32,
    pub bucket_action: u32,
    /// Whether the character can carry the workflow out from its seed state.
    pub feasible: bool,
    /// Human-readable reasons the plan is infeasible; empty when `feasible`.
    pub blockers: Vec<String>,
    /// Advisory risks that do NOT make the plan infeasible (e.g. probabilistic
    /// fight drops that might overflow inventory).
    pub warnings: Vec<String>,
    /// Loop labels resolved by the plan pass, e.g. ("gathers", 10).
    pub assumptions: Vec<(String, u32)>,
}

/// Build the Fennel model-state table `plan` seeds from. `pub(crate)` so the
/// combined TUI path (`live.rs`) can seed `plan` on its shared character-equipped
/// state rather than spinning up `planner::plan`'s own `None`-character state.
pub(crate) fn build_state(lua: &Lua, seed: &PlanSeed) -> LuaResult<LuaTable> {
    // `predicate_state` now builds `st.inventory` itself (from `seed.inventory`),
    // so the former manual `st.set("inventory", {})` here is gone — both the plan
    // seed and the live `host.view` get the full surface from the one helper.
    predicate_state(
        lua,
        seed.x,
        seed.y,
        seed.hp,
        seed.max_hp,
        seed.inventory_count,
        seed.inventory_max_items,
        seed.gold,
        &seed.combat,
        &seed.skills,
        &seed.inventory,
    )
}

/// Collect a Lua sequence of strings stored under `key` (missing → empty).
fn string_list(result: &LuaTable, key: &str) -> Vec<String> {
    result
        .get::<LuaTable>(key)
        .map(|t| t.sequence_values::<String>().flatten().collect())
        .unwrap_or_default()
}

fn extract_plan(result: &LuaTable) -> LuaResult<PlanResult> {
    let seconds: f64 = result.get("seconds")?;
    let actions: u32 = result.get("actions")?;
    let bucket_cost: LuaTable = result.get("bucket-cost")?;
    let bucket_action: u32 = bucket_cost.get("action").unwrap_or(0);
    let feasible: bool = result.get("feasible").unwrap_or(true);

    let blockers = string_list(result, "blockers");
    let warnings = string_list(result, "warnings");

    let mut assumptions = Vec::new();
    if let Ok(t) = result.get::<LuaTable>("assumptions") {
        for (k, v) in t.pairs::<String, u32>().flatten() {
            assumptions.push((k, v));
        }
    }
    assumptions.sort();

    Ok(PlanResult {
        seconds,
        actions,
        bucket_action,
        feasible,
        blockers,
        warnings,
        assumptions,
    })
}

/// Run the `plan` pass on a workflow source: predict both cost and feasibility
/// from `seed` (use [`PlanSeed::from_view`] to seed from a live character).
/// `params` are the raw `key=value` inputs the workflow's `build` is coerced
/// against (empty for a param-less workflow).
pub fn plan(
    workflow_src: &str,
    map: Option<Arc<GameMap>>,
    monsters: Option<Arc<MonsterData>>,
    resources: Option<Arc<ResourceData>>,
    recipes: Option<Arc<RecipeData>>,
    seed: &PlanSeed,
    params: &[(String, String)],
) -> Result<PlanResult> {
    let lua = setup_lua(LuaSetupOptions {
        map,
        monsters,
        resources,
        recipes,
        origin: Some((seed.x, seed.y)),
        ..Default::default()
    })
    .map_err(|e| anyhow!("setup_lua: {e}"))?;

    // The seed state doubles as the read-only `ctx` handed to `build`: a Lua
    // table is a reference, and the plan pass never mutates its input (sims copy),
    // so the same table is safe to use for both.
    let st = build_state(&lua, seed).map_err(|e| anyhow!("build state: {e}"))?;
    let wf = workflow::load(&lua, workflow_src, "workflow.fnl", params, st.clone())?.ok_or_else(
        || anyhow!("workflow built to nothing (single-shot run treats a nil build as a mistake)"),
    )?;

    // The interp entry points live in the `fennel.lib.interp` module (seeded into
    // package.loaded by setup_lua), not as globals, so fetch `plan` off the
    // required module rather than the global table.
    let interp =
        require_module(&lua, "fennel.lib.interp").map_err(|e| anyhow!("require interp: {e}"))?;
    let plan_fn: LuaFunction = interp.get("plan").map_err(|e| anyhow!("{e}"))?;
    let result: LuaTable = plan_fn
        .call((wf, st))
        .map_err(|e| anyhow!("plan pass: {e}"))?;

    extract_plan(&result).map_err(|e| anyhow!("{e}"))
}
