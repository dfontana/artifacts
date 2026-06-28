// fennel.lua vendor: v1.6.1
// sha256: c3d45602041e7d8ef8a212563573df040c48a85c648a29fb4597ebed4bc38ec2
// source: https://fennel-lang.org/downloads/fennel-1.6.1.lua
// Loaded once per Lua state at startup.

use mlua::prelude::*;

use crate::character::Character;
use artifacts_core::cooldown::formulas;

/// Bootstrap a Lua state with:
///  1. The Fennel compiler loaded into globals["fennel"]
///  2. A `host` table with all registered host functions
///  3. The Fennel lib files (actions, predicates, interp) evaluated
pub fn setup_lua(character: Option<Character>) -> LuaResult<Lua> {
    let lua = Lua::new();

    // 1. Load Fennel compiler.
    let fennel_src = include_str!("../../../vendor/fennel.lua");
    let fennel: LuaTable = lua.load(fennel_src).set_name("fennel.lua").eval()?;
    lua.globals().set("fennel", fennel.clone())?;

    // 2. Register host functions.
    register_host_functions(&lua, character)?;

    // 3. Load Fennel library files and install their exports as globals.
    let eval: LuaFunction = fennel.get("eval")?;

    // actions.fnl → install into global _artifacts_actions via set-actions
    let opts = lua.create_table_from([("filename", "actions.fnl")])?;
    let actions_ret: LuaTable = eval.call((
        include_str!("../../../fennel/lib/actions.fnl"),
        opts,
    ))?;
    // Install action constructors as globals.
    install_table_as_globals(&lua, &actions_ret)?;

    // predicates.fnl
    let opts = lua.create_table_from([("filename", "predicates.fnl")])?;
    let preds_ret: LuaTable = eval.call((
        include_str!("../../../fennel/lib/predicates.fnl"),
        opts,
    ))?;
    install_table_as_globals(&lua, &preds_ret)?;

    // interp.fnl → install seq, action, repeat-until, run, estimate, simulate, set-actions
    let opts = lua.create_table_from([("filename", "interp.fnl")])?;
    let interp_ret: LuaTable = eval.call((
        include_str!("../../../fennel/lib/interp.fnl"),
        opts,
    ))?;
    install_table_as_globals(&lua, &interp_ret)?;

    // Register the actions table via the set_actions global installed above.
    let set_actions: LuaFunction = lua.globals().get("set_actions")?;
    let actions_tbl: LuaTable = actions_ret.get("actions")?;
    set_actions.call::<()>(actions_tbl)?;

    Ok(lua)
}

fn install_table_as_globals(lua: &Lua, t: &LuaTable) -> LuaResult<()> {
    let globals = lua.globals();
    for pair in t.clone().pairs::<LuaValue, LuaValue>() {
        let (k, v) = pair?;
        globals.set(k, v)?;
    }
    Ok(())
}

fn register_host_functions(lua: &Lua, character: Option<Character>) -> LuaResult<()> {
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
                formulas::fight(turns)
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
        let code: String = tile.get("resource").unwrap_or_else(|_| "copper_ore".to_string());
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

    if let Some(char) = character {
        register_run_host_fns(lua, &host, char)?;
    } else {
        // In estimate/simulate context: stub run fns so accidental calls fail loudly.
        let stub = lua.create_function(|_, _: LuaMultiValue| -> LuaResult<()> {
            Err(LuaError::RuntimeError(
                "run-pass host fn called in estimate/simulate context".into(),
            ))
        })?;
        for name in &["gather", "move", "fight", "rest", "deposit_item", "deposit_all", "view"] {
            host.set(*name, stub.clone())?;
        }
    }

    lua.globals().set("host", host)?;
    Ok(())
}

fn register_run_host_fns(lua: &Lua, host: &LuaTable, char: Character) -> LuaResult<()> {
    use std::sync::Arc;
    let char = Arc::new(char);

    let c = Arc::clone(&char);
    let gather_fn = lua.create_function(move |lua, _: ()| {
        let outcome = c.gather().map_err(|e| LuaError::RuntimeError(e.to_string()))?;
        outcome_to_lua(lua, &outcome)
    })?;
    host.set("gather", gather_fn)?;

    let c = Arc::clone(&char);
    let move_fn = lua.create_function(move |lua, (x, y): (i32, i32)| {
        let outcome = c.move_to(x, y).map_err(|e| LuaError::RuntimeError(e.to_string()))?;
        outcome_to_lua(lua, &outcome)
    })?;
    host.set("move", move_fn)?;

    let c = Arc::clone(&char);
    let fight_fn = lua.create_function(move |lua, _: ()| {
        let outcome = c.fight().map_err(|e| LuaError::RuntimeError(e.to_string()))?;
        outcome_to_lua(lua, &outcome)
    })?;
    host.set("fight", fight_fn)?;

    let c = Arc::clone(&char);
    let rest_fn = lua.create_function(move |lua, _: ()| {
        let outcome = c.rest().map_err(|e| LuaError::RuntimeError(e.to_string()))?;
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

    // view() -> lua table with current character state.
    // Keys use hyphens to match Fennel's natural identifier convention
    // (st.inventory-count). Changing to underscores breaks the predicates.
    let c = Arc::clone(&char);
    let view_fn = lua.create_function(move |lua, _: ()| {
        let v = c.view.get();
        let t = lua.create_table()?;
        t.set("x", v.x)?;
        t.set("y", v.y)?;
        t.set("hp", v.hp)?;
        t.set("max-hp", v.max_hp)?;
        t.set("inventory-count", v.inventory_count())?;
        t.set("inventory-max-items", v.inventory_max_items)?;
        t.set("inventory-full", v.inventory_full())?;
        Ok(t)
    })?;
    host.set("view", view_fn)?;

    Ok(())
}

fn outcome_to_lua(
    lua: &Lua,
    outcome: &artifacts_core::step::Outcome,
) -> LuaResult<LuaTable> {
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
