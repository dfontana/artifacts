//! Offline planning helper: run the `plan` Fennel pass and return a plain Rust
//! struct, keeping `mlua` types out of callers (e.g. the CLI). Seeding the pass
//! from a live character's state (`PlanSeed::from_view`) makes the prediction
//! specific to that character rather than a generic best case.

use std::sync::Arc;

use anyhow::Result;
use mlua::prelude::*;

use artifacts_core::combat::CombatStats;
use artifacts_core::map::GameMap;
use artifacts_core::step::CharacterView;

use crate::data::MonsterData;
use crate::lua::{eval_fennel, predicate_state, setup_lua};

/// Seed state for a planning pass. The Fennel model state is built from this.
#[derive(Debug, Clone)]
pub struct PlanSeed {
    pub x: i32,
    pub y: i32,
    pub hp: u32,
    pub max_hp: u32,
    pub inventory_count: u32,
    pub inventory_max_items: u32,
    /// Resource level of the gather tile (drives gather cooldown prediction).
    pub tile_level: u32,
    /// Resource code yielded by the gather tile.
    pub tile_resource: String,
    /// The character's combat stats, for the fight `:cost`/`:sim` and `winnable?`.
    pub combat: CombatStats,
}

impl Default for PlanSeed {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            hp: 100,
            max_hp: 100,
            inventory_count: 0,
            inventory_max_items: 100,
            tile_level: 1,
            tile_resource: "copper_ore".to_string(),
            combat: CombatStats {
                hp: 100,
                initiative: 0,
                attack: [0; 4],
                res: [0; 4],
                dmg: [0; 4],
                global_dmg: 0,
                critical_strike: 0,
                haste: 0,
            },
        }
    }
}

impl PlanSeed {
    /// Seed from a live character snapshot (position, hp, inventory, combat stats).
    pub fn from_view(v: &CharacterView) -> Self {
        Self {
            x: v.x,
            y: v.y,
            hp: v.hp,
            max_hp: v.max_hp,
            inventory_count: v.inventory_count(),
            inventory_max_items: v.inventory_max_items,
            combat: CombatStats::from(v),
            ..Self::default()
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
    let st = predicate_state(
        lua,
        seed.x,
        seed.y,
        seed.hp,
        seed.max_hp,
        seed.inventory_count,
        seed.inventory_max_items,
        &seed.combat,
    )?;
    st.set("inventory", lua.create_table()?)?;

    let tile = lua.create_table()?;
    tile.set("level", seed.tile_level)?;
    tile.set("resource", seed.tile_resource.clone())?;
    st.set("tile", tile)?;
    Ok(st)
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
pub fn plan(
    workflow_src: &str,
    map: Option<Arc<GameMap>>,
    monsters: Option<Arc<MonsterData>>,
    seed: &PlanSeed,
) -> Result<PlanResult> {
    // mlua::Error isn't Send (no `send` feature), so it can't ride anyhow's `?`;
    // stringify it at each boundary instead.
    let lua =
        setup_lua(None, map, monsters, None).map_err(|e| anyhow::anyhow!("setup_lua: {e}"))?;
    let wf = eval_fennel(&lua, workflow_src, "workflow.fnl")
        .map_err(|e| anyhow::anyhow!("load workflow: {e}"))?;
    let st = build_state(&lua, seed).map_err(|e| anyhow::anyhow!("build state: {e}"))?;

    let plan_fn: LuaFunction = lua
        .globals()
        .get("plan")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let result: LuaTable = plan_fn
        .call((wf, st))
        .map_err(|e| anyhow::anyhow!("plan pass: {e}"))?;

    extract_plan(&result).map_err(|e| anyhow::anyhow!("{e}"))
}
