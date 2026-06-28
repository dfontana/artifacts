//! Offline planning helpers: run the `estimate`/`simulate` Fennel passes and
//! return plain Rust structs, keeping `mlua` types out of callers (e.g. the CLI).

use std::sync::Arc;

use anyhow::Result;
use mlua::prelude::*;

use artifacts_core::map::GameMap;
use artifacts_core::step::CharacterView;

use crate::lua::{eval_fennel, setup_lua};

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
        }
    }
}

impl PlanSeed {
    /// Seed from a live character snapshot (position, hp, inventory).
    pub fn from_view(v: &CharacterView) -> Self {
        Self {
            x: v.x,
            y: v.y,
            hp: v.hp,
            max_hp: v.max_hp,
            inventory_count: v.inventory_count(),
            inventory_max_items: v.inventory_max_items,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone)]
pub struct EstimateResult {
    pub seconds: f64,
    pub actions: u32,
    pub bucket_action: u32,
    /// Loop labels resolved by the sim pass, e.g. ("gathers", 10).
    pub assumptions: Vec<(String, u32)>,
}

#[derive(Debug, Clone)]
pub struct SimulateResult {
    pub feasible: bool,
    pub gathers: u32,
    pub estimate: EstimateResult,
}

fn build_state(lua: &Lua, seed: &PlanSeed) -> LuaResult<LuaTable> {
    let st = lua.create_table()?;
    st.set("x", seed.x)?;
    st.set("y", seed.y)?;
    st.set("hp", seed.hp)?;
    st.set("max-hp", seed.max_hp)?;
    st.set("inventory-count", seed.inventory_count)?;
    st.set("inventory-max-items", seed.inventory_max_items)?;
    st.set("inventory", lua.create_table()?)?;

    let tile = lua.create_table()?;
    tile.set("level", seed.tile_level)?;
    tile.set("resource", seed.tile_resource.clone())?;
    st.set("tile", tile)?;
    Ok(st)
}

fn extract_estimate(result: &LuaTable) -> LuaResult<EstimateResult> {
    let seconds: f64 = result.get("seconds")?;
    let actions: u32 = result.get("actions")?;
    let bucket_cost: LuaTable = result.get("bucket-cost")?;
    let bucket_action: u32 = bucket_cost.get("action").unwrap_or(0);

    let mut assumptions = Vec::new();
    if let Ok(t) = result.get::<LuaTable>("assumptions") {
        for pair in t.pairs::<String, u32>() {
            if let Ok((k, v)) = pair {
                assumptions.push((k, v));
            }
        }
    }
    assumptions.sort();

    Ok(EstimateResult {
        seconds,
        actions,
        bucket_action,
        assumptions,
    })
}

/// Run the `estimate` pass on a workflow source.
pub fn estimate(
    workflow_src: &str,
    map: Option<Arc<GameMap>>,
    seed: &PlanSeed,
) -> Result<EstimateResult> {
    let lua = setup_lua(None, map).map_err(|e| anyhow::anyhow!("setup_lua: {e}"))?;
    let wf = eval_fennel(&lua, workflow_src, "workflow.fnl")
        .map_err(|e| anyhow::anyhow!("load workflow: {e}"))?;
    let st = build_state(&lua, seed).map_err(|e| anyhow::anyhow!("build state: {e}"))?;

    let estimate_fn: LuaFunction = lua
        .globals()
        .get("estimate")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let result: LuaTable = estimate_fn
        .call((wf, st))
        .map_err(|e| anyhow::anyhow!("estimate pass: {e}"))?;

    extract_estimate(&result).map_err(|e| anyhow::anyhow!("{e}"))
}

/// Run the `simulate` pass on a workflow source.
pub fn simulate(
    workflow_src: &str,
    map: Option<Arc<GameMap>>,
    seed: &PlanSeed,
    trials: u32,
) -> Result<SimulateResult> {
    let lua = setup_lua(None, map).map_err(|e| anyhow::anyhow!("setup_lua: {e}"))?;
    let wf = eval_fennel(&lua, workflow_src, "workflow.fnl")
        .map_err(|e| anyhow::anyhow!("load workflow: {e}"))?;
    let st = build_state(&lua, seed).map_err(|e| anyhow::anyhow!("build state: {e}"))?;

    let simulate_fn: LuaFunction = lua
        .globals()
        .get("simulate")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let result: LuaTable = simulate_fn
        .call((wf, st, trials))
        .map_err(|e| anyhow::anyhow!("simulate pass: {e}"))?;

    let feasible: bool = result.get("feasible").map_err(|e| anyhow::anyhow!("{e}"))?;
    let gathers: u32 = result.get("gathers").unwrap_or(0);
    let est_tbl: LuaTable = result.get("estimate").map_err(|e| anyhow::anyhow!("{e}"))?;
    let estimate = extract_estimate(&est_tbl).map_err(|e| anyhow::anyhow!("{e}"))?;

    Ok(SimulateResult {
        feasible,
        gathers,
        estimate,
    })
}
