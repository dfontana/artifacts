// fennel.lua vendor: v1.6.1
// sha256: c3d45602041e7d8ef8a212563573df040c48a85c648a29fb4597ebed4bc38ec2
// source: https://fennel-lang.org/downloads/fennel-1.6.1.lua
// Loaded once per Lua state at startup.

use mlua::prelude::*;
use std::sync::Arc;

use crate::character::Character;
use crate::data::MonsterData;
use artifacts_core::combat::{self, CombatStats};
use artifacts_core::cooldown::formulas;
use artifacts_core::map::GameMap;
use artifacts_core::step::{FightOutcome, OutcomeKind};

/// Bootstrap a Lua state with:
///  1. The Fennel compiler loaded into globals["fennel"]
///  2. A `host` table with all registered host functions
///  3. The Fennel lib files (actions, predicates, interp) evaluated
///
/// `map` is optional: when present, `host.path_hops` uses A* against it;
/// when absent it falls back to Manhattan distance.
///
/// `monsters` backs `host.monster_stats`; when absent (e.g. an offline plan with
/// no character/token) any combat-stat lookup fails loudly rather than guessing.
pub fn setup_lua(
    character: Option<Character>,
    map: Option<Arc<GameMap>>,
    monsters: Option<Arc<MonsterData>>,
) -> LuaResult<Lua> {
    let lua = Lua::new();

    // 1. Load Fennel compiler.
    let fennel_src = include_str!("../vendor/fennel.lua");
    let fennel: LuaTable = lua.load(fennel_src).set_name("fennel.lua").eval()?;
    lua.globals().set("fennel", fennel.clone())?;

    // 2. Register host functions.
    register_host_functions(&lua, character, map, monsters)?;

    // 3. Load Fennel library files and install each one's exports as globals.
    //    actions → constructors; predicates → predicate fns; interp → the three
    //    passes + set_actions.
    let eval: LuaFunction = fennel.get("eval")?;
    let actions_ret = load_lib(
        &lua,
        &eval,
        include_str!("../fennel/lib/actions.fnl"),
        "actions.fnl",
    )?;
    load_lib(
        &lua,
        &eval,
        include_str!("../fennel/lib/predicates.fnl"),
        "predicates.fnl",
    )?;
    load_lib(
        &lua,
        &eval,
        include_str!("../fennel/lib/interp.fnl"),
        "interp.fnl",
    )?;

    // Register the actions table via the set_actions global installed above.
    let set_actions: LuaFunction = lua.globals().get("set_actions")?;
    let actions_tbl: LuaTable = actions_ret.get("actions")?;
    set_actions.call::<()>(actions_tbl)?;

    Ok(lua)
}

/// Eval one Fennel lib source, install its exported table as globals, and return
/// that table (callers occasionally need a specific export, e.g. `actions`).
fn load_lib(lua: &Lua, eval: &LuaFunction, src: &str, name: &str) -> LuaResult<LuaTable> {
    let opts = lua.create_table_from([("filename", name)])?;
    let exports: LuaTable = eval.call((src, opts))?;
    let globals = lua.globals();
    for pair in exports.clone().pairs::<LuaValue, LuaValue>() {
        let (k, v) = pair?;
        globals.set(k, v)?;
    }
    Ok(exports)
}

/// Build the predicate-facing model-state table read by all three passes.
/// This is the single source of the hyphen-cased key surface predicates depend
/// on (`st.x`, `st.inventory-count`, …); add new predicate inputs here, not at
/// each call site. Switching these to underscores would break predicates.fnl.
// The single state surface deliberately takes each predicate input explicitly,
// so both the planner seed and the live view are built identically (see the
// architecture doc); that's more than clippy's arg limit but is the point.
#[allow(clippy::too_many_arguments)]
pub fn predicate_state(
    lua: &Lua,
    x: i32,
    y: i32,
    hp: u32,
    max_hp: u32,
    inventory_count: u32,
    inventory_max_items: u32,
    combat: &CombatStats,
) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;
    t.set("x", x)?;
    t.set("y", y)?;
    t.set("hp", hp)?;
    t.set("max-hp", max_hp)?;
    t.set("inventory-count", inventory_count)?;
    t.set("inventory-max-items", inventory_max_items)?;
    // The player's static combat stats, so combat predicates (`winnable?`) and the
    // fight `:cost`/`:sim` can simulate against a monster. Current `hp` above is
    // authoritative for the fight's starting HP; `combat.hp` is just a snapshot.
    t.set("combat", combat_stats_to_lua(lua, combat)?)?;
    Ok(t)
}

/// `[fire, earth, water, air]` → a keyed Lua table the Fennel/host layers read.
fn elem_table(lua: &Lua, arr: &[i32; 4]) -> LuaResult<LuaTable> {
    lua.create_table_from([
        ("fire", arr[0]),
        ("earth", arr[1]),
        ("water", arr[2]),
        ("air", arr[3]),
    ])
}

fn read_elem(t: &LuaTable, key: &str) -> [i32; 4] {
    let e: LuaTable = match t.get(key) {
        Ok(e) => e,
        Err(_) => return [0; 4],
    };
    [
        e.get("fire").unwrap_or(0),
        e.get("earth").unwrap_or(0),
        e.get("water").unwrap_or(0),
        e.get("air").unwrap_or(0),
    ]
}

/// Serialise a `CombatStats` into the Lua shape `host.simulate_fight` reads back.
fn combat_stats_to_lua(lua: &Lua, cs: &CombatStats) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;
    t.set("hp", cs.hp)?;
    t.set("initiative", cs.initiative)?;
    t.set("critical_strike", cs.critical_strike)?;
    t.set("haste", cs.haste)?;
    t.set("global_dmg", cs.global_dmg)?;
    t.set("attack", elem_table(lua, &cs.attack)?)?;
    t.set("res", elem_table(lua, &cs.res)?)?;
    t.set("dmg", elem_table(lua, &cs.dmg)?)?;
    Ok(t)
}

fn lua_to_combat_stats(t: &LuaTable) -> CombatStats {
    CombatStats {
        hp: t.get("hp").unwrap_or(0),
        initiative: t.get("initiative").unwrap_or(0),
        attack: read_elem(t, "attack"),
        res: read_elem(t, "res"),
        dmg: read_elem(t, "dmg"),
        global_dmg: t.get("global_dmg").unwrap_or(0),
        critical_strike: t.get("critical_strike").unwrap_or(0),
        haste: t.get("haste").unwrap_or(0),
    }
}

fn register_host_functions(
    lua: &Lua,
    character: Option<Character>,
    map: Option<Arc<GameMap>>,
    monsters: Option<Arc<MonsterData>>,
) -> LuaResult<()> {
    let host = lua.create_table()?;

    // Pure formula: cooldown_cost(op, params) -> seconds
    let cooldown_cost = lua.create_function(|_, (op, params): (String, LuaTable)| {
        let cost = match op.as_str() {
            "movement" => {
                let tiles: u32 = params.get("tiles").unwrap_or(0);
                formulas::movement(tiles)
            }
            "gathering" => {
                let level: u32 = params.get("level").unwrap_or(0);
                formulas::gathering(level)
            }
            "fight" => {
                let turns: u32 = params.get("turns").unwrap_or(1);
                let haste: i32 = params.get("haste").unwrap_or(0);
                formulas::fight(turns, haste)
            }
            "rest" => {
                let hp: u32 = params.get("hp_to_restore").unwrap_or(0);
                formulas::rest(hp)
            }
            "crafting" => {
                let qty: u32 = params.get("quantity").unwrap_or(1);
                formulas::crafting(qty)
            }
            "recycling" => {
                let qty: u32 = params.get("quantity").unwrap_or(1);
                formulas::recycling(qty)
            }
            "deposit" => {
                let n: u32 = params.get("distinct_types").unwrap_or(1);
                formulas::deposit(n)
            }
            _ => formulas::default_action(),
        };
        Ok(cost)
    })?;
    host.set("cooldown_cost", cooldown_cost)?;

    // gather_yield(tile) -> {code, quantity} for sim pass.
    let gather_yield = lua.create_function(|lua, tile: LuaTable| {
        let item = lua.create_table()?;
        let code: String = tile
            .get("resource")
            .unwrap_or_else(|_| "copper_ore".to_string());
        item.set("code", code)?;
        item.set("quantity", 1u32)?;
        Ok(item)
    })?;
    host.set("gather_yield", gather_yield)?;

    // resource_level(tile) -> u32 for sim pass.
    let resource_level = lua.create_function(|_, tile: LuaTable| {
        let level: u32 = tile.get("level").unwrap_or(1);
        Ok(level)
    })?;
    host.set("resource_level", resource_level)?;

    // path_hops(x1, y1, x2, y2) -> integer hop count via A* (or Manhattan fallback).
    // Used by travel-to :cost to predict movement cooldown without I/O.
    let map_for_pathfind = map.clone();
    let path_hops_fn = lua.create_function(move |_, (x1, y1, x2, y2): (i32, i32, i32, i32)| {
        let hops = match &map_for_pathfind {
            Some(m) => m.path_hops((x1, y1), (x2, y2)),
            // No map loaded — fall back to Manhattan.
            None => artifacts_core::map::manhattan((x1, y1), (x2, y2)),
        };
        Ok(hops)
    })?;
    host.set("path_hops", path_hops_fn)?;

    // find_tile(content_type, code) -> {x, y}: the nearest map tile carrying that
    // content (e.g. ("monster","chicken") or ("bank","bank")), measured from
    // spawn (0,0). This is how workflows target monsters and the bank without
    // hardcoding coordinates. With no map (offline generic plan) it returns spawn
    // so the workflow still loads; with a map but no match it errors loudly.
    let map_for_find = map;
    let find_tile = lua.create_function(move |lua, (kind, code): (String, String)| {
        let t = lua.create_table()?;
        let (x, y) = match &map_for_find {
            Some(m) => m.nearest_content((0, 0), &kind, &code).ok_or_else(|| {
                LuaError::RuntimeError(format!("no '{code}' tile of type '{kind}' on the map"))
            })?,
            None => (0, 0),
        };
        t.set("x", x)?;
        t.set("y", y)?;
        Ok(t)
    })?;
    host.set("find_tile", find_tile)?;

    // monster_stats(code) -> the monster's combat-stat table (plus its `drops`),
    // read from the TTL-cached /monsters dataset. Pure once loaded.
    let monster_data = monsters;
    let monster_stats = lua.create_function(move |lua, code: String| {
        let data = monster_data.as_ref().ok_or_else(|| {
            LuaError::RuntimeError(
                "monster data not loaded; plan/run with a character so /monsters can be fetched"
                    .into(),
            )
        })?;
        let m = data.get(&code).ok_or_else(|| {
            LuaError::RuntimeError(format!("unknown monster '{code}' in dataset"))
        })?;
        let t = combat_stats_to_lua(lua, &m.combat_stats())?;
        let drops = lua.create_table()?;
        for (i, d) in m.drops.iter().enumerate() {
            let dt = lua.create_table()?;
            dt.set("code", d.code.clone())?;
            dt.set("rate", d.rate)?;
            dt.set("min", d.min_quantity)?;
            dt.set("max", d.max_quantity)?;
            drops.set(i + 1, dt)?;
        }
        t.set("drops", drops)?;
        Ok(t)
    })?;
    host.set("monster_stats", monster_stats)?;

    // simulate_fight(st, monster_stats) -> {result, turns, hp_remaining}: the
    // deterministic crit-off prediction. Player HP comes from the live/seed `st.hp`
    // (post-rest), the rest of the player's stats from `st.combat`.
    let simulate_fight = lua.create_function(|lua, (st, monster): (LuaTable, LuaTable)| {
        let combat_tbl: LuaTable = st.get("combat")?;
        let mut player = lua_to_combat_stats(&combat_tbl);
        player.hp = st.get("hp").unwrap_or(player.hp);
        let monster = lua_to_combat_stats(&monster);
        let pred = combat::simulate(&player, &monster);
        let out = lua.create_table()?;
        out.set(
            "result",
            match pred.result {
                FightOutcome::Win => "win",
                FightOutcome::Lose => "lose",
            },
        )?;
        out.set("turns", pred.turns)?;
        out.set("hp_remaining", pred.player_hp_remaining)?;
        Ok(out)
    })?;
    host.set("simulate_fight", simulate_fight)?;

    if let Some(char) = character {
        register_run_host_fns(lua, &host, char)?;
    } else {
        // In plan context: stub run fns so accidental calls fail loudly.
        let stub = lua.create_function(|_, _: LuaMultiValue| -> LuaResult<()> {
            Err(LuaError::RuntimeError(
                "run-pass host fn called in plan context".into(),
            ))
        })?;
        for name in &[
            "gather",
            "move",
            "fight",
            "rest",
            "deposit_item",
            "deposit_all",
            "view",
        ] {
            host.set(*name, stub.clone())?;
        }
    }

    lua.globals().set("host", host)?;
    Ok(())
}

fn register_run_host_fns(lua: &Lua, host: &LuaTable, char: Character) -> LuaResult<()> {
    let char = Arc::new(char);

    let c = Arc::clone(&char);
    let gather_fn = lua.create_function(move |lua, _: ()| {
        let outcome = c
            .gather()
            .map_err(|e| LuaError::RuntimeError(e.to_string()))?;
        outcome_to_lua(lua, &outcome)
    })?;
    host.set("gather", gather_fn)?;

    let c = Arc::clone(&char);
    let move_fn = lua.create_function(move |lua, (x, y): (i32, i32)| {
        let outcome = c
            .move_to(x, y)
            .map_err(|e| LuaError::RuntimeError(e.to_string()))?;
        outcome_to_lua(lua, &outcome)
    })?;
    host.set("move", move_fn)?;

    let c = Arc::clone(&char);
    let fight_fn = lua.create_function(move |lua, _: ()| {
        let outcome = c
            .fight()
            .map_err(|e| LuaError::RuntimeError(e.to_string()))?;
        // Live loss-bail: a loss respawns the character at spawn with 1 HP, so
        // looping into another fight death-spirals. Stop the workflow instead.
        if let OutcomeKind::Fight(ref f) = outcome.kind {
            if f.result == FightOutcome::Lose {
                return Err(LuaError::RuntimeError(
                    "fight lost — bailing (character respawned at 1 HP); the plan pass should \
                     have flagged this as not winnable"
                        .into(),
                ));
            }
        }
        outcome_to_lua(lua, &outcome)
    })?;
    host.set("fight", fight_fn)?;

    let c = Arc::clone(&char);
    let rest_fn = lua.create_function(move |lua, _: ()| {
        let outcome = c
            .rest()
            .map_err(|e| LuaError::RuntimeError(e.to_string()))?;
        outcome_to_lua(lua, &outcome)
    })?;
    host.set("rest", rest_fn)?;

    let c = Arc::clone(&char);
    let deposit_item_fn = lua.create_function(move |lua, (code, qty): (String, u32)| {
        let outcome = c
            .deposit_item(code, qty)
            .map_err(|e| LuaError::RuntimeError(e.to_string()))?;
        outcome_to_lua(lua, &outcome)
    })?;
    host.set("deposit_item", deposit_item_fn)?;

    let c = Arc::clone(&char);
    let deposit_all_fn = lua.create_function(move |_, _: ()| {
        c.deposit_all()
            .map_err(|e| LuaError::RuntimeError(e.to_string()))?;
        Ok(())
    })?;
    host.set("deposit_all", deposit_all_fn)?;

    // view() -> the predicate-facing model-state table for the live character,
    // built through the same helper the plan pass uses so the two can't drift
    // (see predicate_state).
    let c = Arc::clone(&char);
    let view_fn = lua.create_function(move |lua, _: ()| {
        let v = c.view.get();
        predicate_state(
            lua,
            v.x,
            v.y,
            v.hp,
            v.max_hp,
            v.inventory_count(),
            v.inventory_max_items,
            &CombatStats::from(&v),
        )
    })?;
    host.set("view", view_fn)?;

    Ok(())
}

fn outcome_to_lua(lua: &Lua, outcome: &artifacts_core::step::Outcome) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;
    t.set("cooldown_remaining", outcome.cooldown.remaining_seconds)?;
    let cv = lua.create_table()?;
    cv.set("x", outcome.character.x)?;
    cv.set("y", outcome.character.y)?;
    cv.set("hp", outcome.character.hp)?;
    cv.set("max_hp", outcome.character.max_hp)?;
    cv.set("inventory_count", outcome.character.inventory_count())?;
    cv.set("inventory_max_items", outcome.character.inventory_max_items)?;
    cv.set("inventory_full", outcome.character.inventory_full())?;
    t.set("character", cv)?;
    Ok(t)
}

/// Compile and evaluate a Fennel source string in an already-set-up Lua state.
pub fn eval_fennel(lua: &Lua, src: &str, name: &str) -> LuaResult<LuaValue> {
    let fennel: LuaTable = lua.globals().get("fennel")?;
    let eval: LuaFunction = fennel.get("eval")?;
    let opts = lua.create_table_from([("filename", name)])?;
    eval.call((src, opts))
}
