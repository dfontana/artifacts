// fennel.lua vendor: v1.6.1
// sha256: c3d45602041e7d8ef8a212563573df040c48a85c648a29fb4597ebed4bc38ec2
// source: https://fennel-lang.org/downloads/fennel-1.6.1.lua
// Loaded once per Lua state at startup.

use mlua::prelude::*;
use std::sync::Arc;

use crate::character::Character;
use crate::character::SharedView;
use crate::data::{MonsterData, ResourceData};
use crate::progress::{NodeId, ProgressLog};
use artifacts_core::combat::{self, CombatStats};
use artifacts_core::cooldown::formulas;
use artifacts_core::ident::{Code, ContentType};
use artifacts_core::map::GameMap;
use artifacts_core::step::{FightOutcome, Intent, OutcomeKind};
use artifacts_core::wire;
use strum::IntoEnumIterator;

/// Optional inputs to [`setup_lua`]. All fields default to `None`; set only
/// what a given caller needs with `..Default::default()` instead of counting
/// positional `Option`s.
///
/// `character` is the player character, when running live. `None` for the
/// plan/CLI paths and bare tests.
///
/// `map` is optional: when present, `host.path_hops` uses A* against it and
/// `host.find_tile` resolves content; when absent, `path_hops` falls back to
/// Manhattan and `find_tile` fails loudly rather than fabricating coordinates.
///
/// `monsters` backs `host.monster_stats`; when absent (e.g. an offline plan with
/// no character/token) any combat-stat lookup fails loudly rather than guessing.
///
/// `resources` backs `host.active_resource`'s level lookup; when absent, gathering
/// while standing on a resource tile still fails loudly (same strictness as
/// `monsters` absent).
///
/// `origin` is the position `host.find_tile` measures "nearest" from. In the
/// RUN path (character present) `find_tile` ignores `origin` and re-derives its
/// anchor from the character's LIVE position on each call (so a mid-run
/// find_tile tracks the character as it moves); in the PLAN path (character
/// None) it's the frozen seed origin. `None` (position genuinely unknown, e.g.
/// a bare test state with no character) anchors to spawn `(0, 0)`.
///
/// `progress` is the TUI run panel's append-only id-log: when `Some`, `run-node`'s
/// `host.progress` appends each node id it enters; when `None`, `host.progress` is
/// a no-op stub (the `plan`/CLI-`run` paths, unchanged). See `plans/TUI.md` §3.6.
#[derive(Default)]
pub struct LuaSetupOptions {
    pub character: Option<Character>,
    pub map: Option<Arc<GameMap>>,
    pub monsters: Option<Arc<MonsterData>>,
    pub resources: Option<Arc<ResourceData>>,
    pub origin: Option<(i32, i32)>,
    pub progress: Option<ProgressLog>,
}

/// Bootstrap a Lua state with:
///  1. The Fennel compiler loaded into globals["fennel"]
///  2. A `host` table with all registered host functions
///  3. The Fennel lib files (actions, predicates, interp) evaluated and
///     registered as require-able modules via `package.loaded` (NOT installed
///     as globals — workflows `(require :fennel.lib.interp)` etc., which is
///     also what fennel-ls resolves statically).
///
/// See [`LuaSetupOptions`] for the optional inputs each caller can set.
pub fn setup_lua(opts: LuaSetupOptions) -> LuaResult<Lua> {
    let LuaSetupOptions {
        character,
        map,
        monsters,
        resources,
        origin,
        progress,
    } = opts;
    let lua = Lua::new();

    // 1. Load Fennel compiler.
    let fennel_src = include_str!("../vendor/fennel.lua");
    let fennel: LuaTable = lua.load(fennel_src).set_name("fennel.lua").eval()?;
    lua.globals().set("fennel", fennel.clone())?;

    // 2. Register host functions.
    register_host_functions(&lua, character, map, monsters, resources, origin, progress)?;

    // 3. Load Fennel library files and register each as a require-able module by
    //    seeding package.loaded[<dotted name>] = exports. Workflows then pull
    //    symbols via `(require :fennel.lib.interp)` etc. instead of these being
    //    globals — which is what fennel-ls resolves statically (extra-globals is
    //    no longer needed). actions → constructors; predicates → predicate
    //    fns; interp → the three passes + set_actions.
    let eval: LuaFunction = fennel.get("eval")?;
    let actions_ret = load_lib(
        &lua,
        &eval,
        include_str!("../fennel/lib/actions.fnl"),
        "actions.fnl",
        "fennel.lib.actions",
    )?;
    load_lib(
        &lua,
        &eval,
        include_str!("../fennel/lib/predicates.fnl"),
        "predicates.fnl",
        "fennel.lib.predicates",
    )?;
    let interp_ret = load_lib(
        &lua,
        &eval,
        include_str!("../fennel/lib/interp.fnl"),
        "interp.fnl",
        "fennel.lib.interp",
    )?;

    // Register the actions table via interp's set_actions (which stashes it in
    // _G._artifacts_actions for interp's get-action to read at plan/run time).
    let set_actions: LuaFunction = interp_ret.get("set_actions")?;
    let actions_tbl: LuaTable = actions_ret.get("actions")?;
    set_actions.call::<()>(actions_tbl)?;

    Ok(lua)
}

/// Eval one Fennel lib source and register its exports as a require-able module
/// by seeding `package.loaded[module] = exports`; return the exports table so
/// callers can grab a specific export (e.g. interp's `set_actions`, or the
/// `actions` table). Seeding package.loaded means `(require :module)` returns
/// the table at runtime with no fennel searcher or fennel-path configured — the
/// libs are `include_str!`-ed, so the binary never touches the source tree.
fn load_lib(
    lua: &Lua,
    eval: &LuaFunction,
    src: &str,
    name: &str,
    module: &str,
) -> LuaResult<LuaTable> {
    let opts = lua.create_table_from([("filename", name)])?;
    let exports: LuaTable = eval.call((src, opts))?;
    let loaded: LuaTable = lua
        .globals()
        .get::<LuaTable>("package")?
        .get::<LuaTable>("loaded")?;
    loaded.set(module, exports.clone())?;
    Ok(exports)
}

/// Require a module by name in an already-set-up Lua state, returning its
/// exports table. Tests use this to fetch lib exports (e.g. interp's `plan`)
/// now that the libs are no longer installed as globals.
pub fn require_module(lua: &Lua, module: &str) -> LuaResult<LuaTable> {
    let require: LuaFunction = lua.globals().get("require")?;
    require.call::<LuaTable>(module)
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
    gold: u32,
    combat: &CombatStats,
) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;
    t.set("x", x)?;
    t.set("y", y)?;
    t.set("hp", hp)?;
    t.set("max-hp", max_hp)?;
    t.set("inventory-count", inventory_count)?;
    t.set("inventory-max-items", inventory_max_items)?;
    // Gold is on the single state surface now (same "one source, plan and run
    // can't drift" argument as x/hp/inventory above), so buy/sell/bank :sim
    // formulas and gold predicates (`gold_at_least`) read st.gold identically
    // whether st came from the plan seed or a live host.view snapshot.
    t.set("gold", gold)?;
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

/// Stringify any error into the Lua error shape host fns raise.
fn lua_err(e: impl std::fmt::Display) -> LuaError {
    LuaError::RuntimeError(e.to_string())
}

fn register_host_functions(
    lua: &Lua,
    character: Option<Character>,
    map: Option<Arc<GameMap>>,
    monsters: Option<Arc<MonsterData>>,
    resources: Option<Arc<ResourceData>>,
    origin: Option<(i32, i32)>,
    progress: Option<ProgressLog>,
) -> LuaResult<()> {
    let host = lua.create_table()?;

    // progress(id) — the TUI run cursor. Always registered (`run-node` calls it
    // unconditionally, so leaving it unregistered would be a nil-call), but a
    // no-op unless a log was supplied. `id` is nil in the CLI run path (no
    // `number-nodes`), which is silently ignored.
    let progress_fn = lua.create_function(move |_, id: Option<NodeId>| {
        if let (Some(log), Some(id)) = (progress.as_ref(), id) {
            log.lock().expect("progress log poisoned").push(id);
        }
        Ok(())
    })?;
    host.set("progress", progress_fn)?;

    // Pure formula: cooldown_cost(op, params) -> seconds. An unknown op is a
    // loud error, not a silent 3s default — a typo'd op would otherwise make
    // the plan lie (the same failure class monster_stats/find_tile reject).
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
            "deposit" => {
                let n: u32 = params.get("distinct_types").unwrap_or(1);
                formulas::deposit(n)
            }
            "craft" => {
                let n: u32 = params.get("quantity").unwrap_or(1);
                formulas::craft(n)
            }
            "recycle" => {
                let n: u32 = params.get("quantity").unwrap_or(1);
                formulas::recycle(n)
            }
            // The flat-3s family (use, delete, equip/unequip, npc, grand
            // exchange, tasks, transition, bank/give gold) — no scaling params.
            "simple" => formulas::simple(),
            other => {
                return Err(lua_err(format!(
                    "cooldown_cost: unknown op '{other}' (no client-side formula)"
                )))
            }
        };
        Ok(cost)
    })?;
    host.set("cooldown_cost", cooldown_cost)?;

    // active_resource(x, y) -> {code, level}: the resource content of the tile
    // at (x, y) — always the caller's *current* position (`st.x`/`st.y`), never
    // a frozen seed. Used by :gather's :cost/:sim so a workflow that travels to
    // several different resources gets correct per-tile predictions instead of
    // one fixed assumption for the whole plan. Errors loudly (no map loaded, no
    // tile there, tile isn't a resource, or the resource is missing from the
    // /resources dataset) rather than fabricating a code/level — same
    // strictness as `find_tile`/`monster_stats`.
    let map_for_resource = map.clone();
    let resource_data = resources;
    let active_resource = lua.create_function(move |lua, (x, y): (i32, i32)| {
        let m = map_for_resource.as_ref().ok_or_else(|| {
            lua_err("gather: no map loaded; plan/run with a character so /maps can be fetched")
        })?;
        let tile = m
            .get(x, y)
            .ok_or_else(|| lua_err(format!("gather: no map tile at ({x}, {y})")))?;
        let content = tile.interactions.content.as_ref().ok_or_else(|| {
            lua_err(format!(
                "gather: tile at ({x}, {y}) has no resource (nothing to gather)"
            ))
        })?;
        if content.content_type.as_str() != "resource" {
            return Err(lua_err(format!(
                "gather: tile at ({x}, {y}) is a '{}', not a resource",
                content.content_type
            )));
        }
        let data = resource_data.as_ref().ok_or_else(|| {
            lua_err(
                "resource data not loaded; plan/run with a character so /resources can be fetched",
            )
        })?;
        let r = data
            .get(&content.code)
            .ok_or_else(|| lua_err(format!("unknown resource '{}' in dataset", content.code)))?;
        let out = lua.create_table()?;
        out.set("code", r.code.as_str())?;
        out.set("level", r.level)?;
        Ok(out)
    })?;
    host.set("active_resource", active_resource)?;

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
    // content (e.g. ("monster","chicken") or ("bank","bank")), measured from the
    // caller's anchor position. This is how workflows target monsters and the
    // bank without hardcoding coordinates. No map or no match errors loudly —
    // fabricating a coordinate would make every downstream travel cost a lie.
    //
    // The anchor is re-derived PER CALL, not frozen at setup_lua time:
    //  - RUN path (character present): the character's LIVE position, read from
    //    `live_view` (`SharedView::get`), so a mid-run find_tile (e.g. a workflow
    //    re-resolving a tile after traveling) anchors to where the character
    //    actually is, not the stale initial position frozen at setup_lua time.
    //  - PLAN path (character None): the frozen seed `origin` (the planner has
    //    no live position to read), with `(0, 0)` for a bare `None` test state.
    let map_for_find = map;
    let live_view: Option<SharedView> = character.as_ref().map(|c| c.view.clone());
    let fallback_origin = origin;
    let find_tile = lua.create_function(move |lua, (kind, code): (String, String)| {
        // The Lua layer passes bare strings; pin them to identity newtypes at the
        // boundary so the core lookup can't be handed an arbitrary string.
        let (kind, code) = (ContentType::from(kind), Code::from(code));
        let m = map_for_find.as_ref().ok_or_else(|| {
            lua_err("find_tile: no map loaded; plan/run with a character so /maps can be fetched")
        })?;
        let find_from = match &live_view {
            // RUN path: live character position, re-read each call.
            Some(v) => {
                let c = v.get();
                (c.x, c.y)
            }
            // PLAN path: frozen seed origin (`(0, 0)` for a bare `None` state).
            None => fallback_origin.unwrap_or((0, 0)),
        };
        let (x, y) = m
            .nearest_content(find_from, &kind, &code)
            .ok_or_else(|| lua_err(format!("no '{code}' tile of type '{kind}' on the map")))?;
        let t = lua.create_table()?;
        t.set("x", x)?;
        t.set("y", y)?;
        Ok(t)
    })?;
    host.set("find_tile", find_tile)?;

    // monster_stats(code) -> the monster's combat-stat table (plus its `drops`),
    // read from the TTL-cached /monsters dataset. Pure once loaded.
    let monster_data = monsters;
    let monster_stats = lua.create_function(move |lua, code: String| {
        let code = Code::from(code);
        let data = monster_data.as_ref().ok_or_else(|| {
            lua_err(
                "monster data not loaded; plan/run with a character so /monsters can be fetched",
            )
        })?;
        let m = data
            .get(&code)
            .ok_or_else(|| lua_err(format!("unknown monster '{code}' in dataset")))?;
        let t = combat_stats_to_lua(lua, &m.combat_stats())?;
        let drops = lua.create_table()?;
        for (i, d) in m.drops.iter().enumerate() {
            let dt = lua.create_table()?;
            dt.set("code", d.code.to_string())?;
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

    // Run-pass fns: registered against the live character when present; in
    // plan context (no character) the SAME registrations become loud stubs —
    // one list by construction, so the two paths can't drift.
    register_run_host_fns(lua, &host, character)?;

    lua.globals().set("host", host)?;
    Ok(())
}

/// Run fns return nothing: the run interpreter reads state through `host.view`
/// (the one `predicate_state` surface), never through per-action return values.
fn done(
    r: Result<artifacts_core::step::Outcome, artifacts_core::error::GameError>,
) -> LuaResult<()> {
    r.map(|_| ()).map_err(lua_err)
}

/// Register one run-pass host fn: one `lua.create_function` + `host.set`
/// pair. With a live `Character`, `body` runs against it; with `None` (plan
/// context) the registered closure raises loudly before `body` runs — the
/// same registration serves both paths, so they can't drift. `body` receives
/// the live `&Character`, the Lua handle (name it `_lua` when unused), and
/// the args mlua decoded (a tuple, or `()` for arg-less fns).
fn host_fn<A, R>(
    lua: &Lua,
    host: &LuaTable,
    char: &Option<Arc<Character>>,
    name: &str,
    body: impl Fn(&Character, &Lua, A) -> LuaResult<R> + Send + 'static,
) -> LuaResult<()>
where
    A: mlua::FromLuaMulti,
    R: mlua::IntoLuaMulti,
{
    let char = char.clone();
    let f = lua.create_function(move |lua, args: A| {
        let Some(c) = char.as_deref() else {
            return Err(lua_err("run-pass host fn called in plan context"));
        };
        body(c, lua, args)
    })?;
    host.set(name, f)
}

/// Register every run-pass host fn. Intent-backed fns are bound generically by
/// iterating `Intent::iter()` (see `register_intent`); the two non-intent fns
/// (`deposit_all`, `view`) are bound explicitly here — they are deliberately
/// not 1:1 with an `Intent` (plans/INTENTS.md §9).
fn register_run_host_fns(
    lua: &Lua,
    host: &LuaTable,
    character: Option<Character>,
) -> LuaResult<()> {
    let char: Option<Arc<Character>> = character.map(Arc::new);

    // One run host fn per Intent variant, bound through the exhaustive match in
    // `register_intent`. `Intent::iter()` is derived, so this runs for every
    // declared variant; a new variant that isn't bound fails to compile.
    for proto in Intent::iter() {
        register_intent(lua, host, &char, &proto)?;
    }

    host_fn(lua, host, &char, "deposit_all", |c, _lua, ()| {
        c.deposit_all().map_err(lua_err)?;
        Ok(())
    })?;
    // view() -> the predicate-facing model-state table for the live
    // character, built through the same helper the plan pass uses so the
    // two can't drift (see predicate_state).
    host_fn(lua, host, &char, "view", |c, lua, ()| {
        let v = c.view.get();
        predicate_state(
            lua,
            v.x,
            v.y,
            v.hp,
            v.max_hp,
            v.inventory_count(),
            v.inventory_max_items,
            v.gold,
            &CombatStats::from(&*v),
        )
    })?;

    Ok(())
}

/// Bind one intent's run host fn, keyed by the (prototype) variant. This match
/// is exhaustive over `Intent`, so a new variant fails to compile until it is
/// bound here — combined with the derived `Intent::iter()` driving it, a
/// declared intent can never be left unregistered (which would surface as a
/// silent nil-call in a workflow). Each arm binds a typed closure, so a change
/// to an intent's `Character` wrapper signature is a compile error too. The
/// `proto` payload is a `Default` placeholder from `Intent::iter()`; only the
/// variant is read.
fn register_intent(
    lua: &Lua,
    host: &LuaTable,
    char: &Option<Arc<Character>>,
    proto: &Intent,
) -> LuaResult<()> {
    match proto {
        Intent::Move(_) => host_fn(lua, host, char, "move", |c, _lua, (x, y): (i32, i32)| {
            done(c.submit(Intent::Move(wire::Move { x, y })))
        }),
        Intent::Gather(_) => host_fn(lua, host, char, "gather", |c, _lua, ()| {
            done(c.submit(Intent::Gather(wire::Gather)))
        }),
        Intent::Fight(_) => host_fn(lua, host, char, "fight", |c, _lua, ()| {
            let outcome = c.submit(Intent::Fight(wire::Fight)).map_err(lua_err)?;
            // Live loss-bail: a loss respawns the character at spawn with 1 HP,
            // so looping into another fight death-spirals. Stop the workflow.
            if let OutcomeKind::Fight(ref f) = outcome.kind {
                if f.result == FightOutcome::Lose {
                    return Err(lua_err(
                        "fight lost — bailing (character respawned at 1 HP); the plan \
                         pass should have flagged this as not winnable",
                    ));
                }
            }
            Ok(())
        }),
        Intent::Rest(_) => host_fn(lua, host, char, "rest", |c, _lua, ()| {
            done(c.submit(Intent::Rest(wire::Rest)))
        }),
        Intent::DepositItem(_) => host_fn(
            lua,
            host,
            char,
            "deposit_item",
            |c, _lua, (code, quantity): (String, u32)| {
                done(c.submit(Intent::DepositItem(wire::DepositItem {
                    code: code.into(),
                    quantity,
                })))
            },
        ),
        Intent::WithdrawItem(_) => host_fn(
            lua,
            host,
            char,
            "withdraw_item",
            |c, _lua, (code, quantity): (String, u32)| {
                done(c.submit(Intent::WithdrawItem(wire::WithdrawItem {
                    code: code.into(),
                    quantity,
                })))
            },
        ),
        Intent::Craft(_) => host_fn(
            lua,
            host,
            char,
            "craft",
            |c, _lua, (code, quantity): (String, u32)| {
                done(c.submit(Intent::Craft(wire::Craft {
                    code: code.into(),
                    quantity,
                })))
            },
        ),
        Intent::Recycle(_) => host_fn(
            lua,
            host,
            char,
            "recycle",
            |c, _lua, (code, quantity): (String, u32)| {
                done(c.submit(Intent::Recycle(wire::Recycle {
                    code: code.into(),
                    quantity,
                })))
            },
        ),
        Intent::UseItem(_) => host_fn(
            lua,
            host,
            char,
            "use_item",
            |c, _lua, (code, quantity): (String, u32)| {
                done(c.submit(Intent::UseItem(wire::UseItem {
                    code: code.into(),
                    quantity,
                })))
            },
        ),
        Intent::DeleteItem(_) => host_fn(
            lua,
            host,
            char,
            "delete_item",
            |c, _lua, (code, quantity): (String, u32)| {
                done(c.submit(Intent::DeleteItem(wire::DeleteItem {
                    code: code.into(),
                    quantity,
                })))
            },
        ),
        Intent::Equip(_) => host_fn(
            lua,
            host,
            char,
            "equip",
            |c, _lua, (code, slot, quantity): (String, String, u32)| {
                done(c.submit(Intent::Equip(wire::Equip {
                    code: code.into(),
                    slot: slot.into(),
                    quantity,
                })))
            },
        ),
        Intent::Unequip(_) => host_fn(
            lua,
            host,
            char,
            "unequip",
            |c, _lua, (slot, quantity): (String, u32)| {
                done(c.submit(Intent::Unequip(wire::Unequip {
                    slot: slot.into(),
                    quantity,
                })))
            },
        ),
        Intent::DepositGold(_) => {
            host_fn(lua, host, char, "deposit_gold", |c, _lua, quantity: u32| {
                done(c.submit(Intent::DepositGold(wire::DepositGold { quantity })))
            })
        }
        Intent::WithdrawGold(_) => host_fn(
            lua,
            host,
            char,
            "withdraw_gold",
            |c, _lua, quantity: u32| {
                done(c.submit(Intent::WithdrawGold(wire::WithdrawGold { quantity })))
            },
        ),
        Intent::GiveGold(_) => host_fn(
            lua,
            host,
            char,
            "give_gold",
            |c, _lua, (quantity, character): (u32, String)| {
                done(c.submit(Intent::GiveGold(wire::GiveGold {
                    quantity,
                    character: character.into(),
                })))
            },
        ),
        Intent::GiveItem(_) => host_fn(
            lua,
            host,
            char,
            "give_item",
            |c, _lua, (code, quantity, character): (String, u32, String)| {
                done(c.submit(Intent::GiveItem(wire::GiveItem {
                    code: code.into(),
                    quantity,
                    character: character.into(),
                })))
            },
        ),
        Intent::NpcBuy(_) => host_fn(
            lua,
            host,
            char,
            "npc_buy",
            |c, _lua, (code, quantity): (String, u32)| {
                done(c.submit(Intent::NpcBuy(wire::NpcBuy {
                    code: code.into(),
                    quantity,
                })))
            },
        ),
        Intent::NpcSell(_) => host_fn(
            lua,
            host,
            char,
            "npc_sell",
            |c, _lua, (code, quantity): (String, u32)| {
                done(c.submit(Intent::NpcSell(wire::NpcSell {
                    code: code.into(),
                    quantity,
                })))
            },
        ),
        Intent::GeBuy(_) => host_fn(
            lua,
            host,
            char,
            "ge_buy",
            |c, _lua, (id, quantity): (String, u32)| {
                done(c.submit(Intent::GeBuy(wire::GeBuy {
                    id: id.into(),
                    quantity,
                })))
            },
        ),
        Intent::GeCancel(_) => host_fn(lua, host, char, "ge_cancel", |c, _lua, id: String| {
            done(c.submit(Intent::GeCancel(wire::GeCancel { id: id.into() })))
        }),
        Intent::GeFill(_) => host_fn(
            lua,
            host,
            char,
            "ge_fill",
            |c, _lua, (id, quantity): (String, u32)| {
                done(c.submit(Intent::GeFill(wire::GeFill {
                    id: id.into(),
                    quantity,
                })))
            },
        ),
        Intent::TaskNew(_) => host_fn(lua, host, char, "task_new", |c, _lua, ()| {
            done(c.submit(Intent::TaskNew(wire::TaskNew)))
        }),
        Intent::TaskComplete(_) => host_fn(lua, host, char, "task_complete", |c, _lua, ()| {
            done(c.submit(Intent::TaskComplete(wire::TaskComplete)))
        }),
        Intent::TaskCancel(_) => host_fn(lua, host, char, "task_cancel", |c, _lua, ()| {
            done(c.submit(Intent::TaskCancel(wire::TaskCancel)))
        }),
        Intent::TaskExchange(_) => host_fn(lua, host, char, "task_exchange", |c, _lua, ()| {
            done(c.submit(Intent::TaskExchange(wire::TaskExchange)))
        }),
        Intent::TaskTrade(_) => host_fn(
            lua,
            host,
            char,
            "task_trade",
            |c, _lua, (code, quantity): (String, u32)| {
                done(c.submit(Intent::TaskTrade(wire::TaskTrade {
                    code: code.into(),
                    quantity,
                })))
            },
        ),
        Intent::Transition(_) => host_fn(lua, host, char, "transition", |c, _lua, ()| {
            done(c.submit(Intent::Transition(wire::Transition)))
        }),
    }
}

/// Compile and evaluate a Fennel source string in an already-set-up Lua state.
pub fn eval_fennel(lua: &Lua, src: &str, name: &str) -> LuaResult<LuaValue> {
    let fennel: LuaTable = lua.globals().get("fennel")?;
    let eval: LuaFunction = fennel.get("eval")?;
    let opts = lua.create_table_from([("filename", name)])?;
    eval.call((src, opts))
}
