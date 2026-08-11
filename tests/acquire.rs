//! M5 acquire-generator tests (`plans/DYNAMIC_WORKFLOWS.md` §4.1–§4.2, §5.5).
//!
//! Everything is driven through the public entry points — `planner::plan` for
//! cost/feasibility/loop-count assertions and `workflow::load` (+ the interp
//! module's `number_nodes`/`skeleton`) for structural assertions — against one
//! small fixture world, per the entrypoint-driven testing standard:
//!   - the flagship path: craft.fnl item=copper_dagger walks the 3-level
//!     recipe DAG (dagger ← bars ← ore), withdraws what the bank holds, and
//!     gathers the rest — feasible, with nested "acquire …" groups in
//!     post-order and a deterministic skeleton;
//!   - no-route errors name every rejected source with its reason;
//!   - the buy / fight source kinds, the policy override, and capacity
//!     chunking each produce their documented shapes;
//!   - `host.item_sources` reports the craftable flag, the Rust-side sort
//!     orders, empty lists for an unknown item, and a loud error only when no
//!     dataset was supplied.

use std::sync::Arc;

use artifacts::data::{BankData, MonsterData, NpcItemData, RecipeData, ResourceData};
use artifacts::lua::{predicate_state, require_module, setup_lua, LuaSetupOptions};
use artifacts::planner::{self, PlanSeed};
use artifacts::workflow;
use artifacts_core::bank::BankItemView;
use artifacts_core::combat::{CombatStats, MonsterView};
use artifacts_core::drop::DropRate;
use artifacts_core::map::GameMap;
use artifacts_core::npc::NpcItemView;
use artifacts_core::recipe::{RecipeCraft, RecipeInput, RecipeView};
use artifacts_core::step::SkillLevels;
use mlua::prelude::*;

mod common;
use common::{make_map, make_resources};

const CRAFT_SRC: &str = include_str!("../fennel/workflows/craft.fnl");

// ─── the fixture world ───────────────────────────────────────────────────────
// A 7×2 grid: bank + mining/weaponcrafting workshops on the south row, the
// gather/fight/buy sites on the north row. Recipes form the 3-level DAG
// copper_dagger ← 2× copper_bar ← 2× copper_ore.

fn world_map() -> Arc<GameMap> {
    make_map(
        7,
        2,
        &[
            (0, 1, "bank", "bank"),
            (1, 0, "resource", "copper_rocks"),
            (2, 0, "resource", "iron_rocks"),
            (3, 0, "monster", "chicken"),
            (4, 0, "npc", "lumber_merchant"),
            (5, 0, "npc", "shady_trader"),
            (1, 1, "workshop", "mining"),
            (2, 1, "workshop", "weaponcrafting"),
        ],
    )
}

fn world_resources() -> Arc<ResourceData> {
    make_resources(&[
        ("copper_rocks", "mining", 1, "copper_ore"),
        ("iron_rocks", "mining", 10, "iron_ore"),
    ])
}

/// A chicken that drops eggs at rate 2 (expected 0.5/win) — winnable for the
/// standard seed (attack 50 vs 60 hp), unwinnable for a 0-attack seed (nobody
/// deals damage, so the 100-turn cap reads as a loss).
fn world_monsters() -> Arc<MonsterData> {
    Arc::new(MonsterData::from_vec(vec![MonsterView {
        code: "chicken".into(),
        name: "Chicken".into(),
        level: 1,
        hp: 60,
        attack_fire: 5,
        attack_earth: 0,
        attack_water: 0,
        attack_air: 0,
        res_fire: 0,
        res_earth: 0,
        res_water: 0,
        res_air: 0,
        critical_strike: 0,
        initiative: 0,
        drops: vec![DropRate {
            code: "egg".into(),
            rate: 2,
            min_quantity: 1,
            max_quantity: 1,
        }],
    }]))
}

fn recipe(code: &str, skill: &str, level: u32, input: &str, quantity: u32) -> RecipeView {
    RecipeView {
        code: code.into(),
        craft: Some(RecipeCraft {
            skill: Some(skill.into()),
            level: Some(level),
            items: vec![RecipeInput {
                code: input.into(),
                quantity,
            }],
            quantity: 1,
        }),
    }
}

fn world_recipes() -> Arc<RecipeData> {
    Arc::new(RecipeData::from_items(vec![
        recipe("copper_bar", "mining", 1, "copper_ore", 2),
        recipe("copper_dagger", "weaponcrafting", 1, "copper_bar", 2),
    ]))
}

fn npc_listing(code: &str, npc: &str, currency: &str, buy_price: Option<u32>) -> NpcItemView {
    NpcItemView {
        code: code.into(),
        npc: npc.into(),
        currency: currency.into(),
        buy_price,
        sell_price: None,
    }
}

/// A gold-priced ash_wood, a gold-priced copper_bar (for the policy-override
/// test: craftable AND buyable), and a non-gold (item-currency) listing that
/// must never be auto-chosen.
fn world_npcs() -> Arc<NpcItemData> {
    Arc::new(NpcItemData::from_vec(vec![
        npc_listing("ash_wood", "lumber_merchant", "gold", Some(5)),
        npc_listing("copper_bar", "lumber_merchant", "gold", Some(20)),
        npc_listing("mystic_orb", "shady_trader", "soul_shard", Some(3)),
    ]))
}

/// The standard seed: at (0,0), full hp, big pack, mining+weaponcrafting 1,
/// a fire attack that beats the chicken.
fn world_seed() -> PlanSeed {
    PlanSeed {
        hp: 100,
        max_hp: 100,
        inventory_max_items: 100,
        combat: CombatStats {
            hp: 100,
            attack: [50, 0, 0, 0],
            ..Default::default()
        },
        skills: SkillLevels {
            mining: 1,
            weaponcrafting: 1,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// `planner::plan` against the full fixture world.
fn plan_world(
    src: &str,
    bank: Option<BankData>,
    seed: &PlanSeed,
    params: &[(&str, &str)],
) -> anyhow::Result<planner::PlanResult> {
    let params: Vec<(String, String)> = params
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let context = artifacts::context::ExecutionContext {
        map: Some(world_map()),
        monsters: Some(world_monsters()),
        resources: Some(world_resources()),
        recipes: Some(world_recipes()),
        npc_items: Some(world_npcs()),
        bank: bank.map(Arc::new),
    };
    planner::plan(src, &context, seed, &params)
}

// ─── skeleton helpers (structural assertions via workflow::load) ─────────────

fn setup_world_lua() -> Lua {
    setup_lua(LuaSetupOptions {
        map: Some(world_map()),
        monsters: Some(world_monsters()),
        resources: Some(world_resources()),
        recipes: Some(world_recipes()),
        npc_items: Some(world_npcs()),
        ..Default::default()
    })
    .expect("setup_lua")
}

/// The read-only `ctx` a `build` receives: the seed state through the single
/// `predicate_state` surface, plus a `bank` table (what `attach_bank` layers
/// on in the planner/live paths).
fn build_ctx(lua: &Lua, seed: &PlanSeed, bank: &[(&str, u32)]) -> LuaTable {
    let ctx = predicate_state(
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
    .expect("predicate_state");
    let b = lua.create_table().expect("bank table");
    for (code, qty) in bank {
        b.set(*code, *qty).expect("bank entry");
    }
    ctx.set("bank", b).expect("set bank");
    ctx
}

fn fmt_value(v: &LuaValue) -> String {
    match v {
        LuaValue::Table(t) => {
            let inner: Vec<String> = t
                .clone()
                .sequence_values::<LuaValue>()
                .flatten()
                .map(|x| fmt_value(&x))
                .collect();
            format!("[{}]", inner.join(","))
        }
        LuaValue::String(s) => s.to_string_lossy().to_string(),
        LuaValue::Integer(i) => i.to_string(),
        LuaValue::Number(n) => n.to_string(),
        LuaValue::Boolean(b) => b.to_string(),
        other => format!("<{}>", other.type_name()),
    }
}

/// Load `src` (a workflow module), run `number_nodes` + `skeleton`, and render
/// each row as a "depth kind detail" string — compact enough for row-order and
/// row-by-row-equality assertions.
fn build_skeleton(lua: &Lua, src: &str, params: &[(&str, &str)], ctx: LuaTable) -> Vec<String> {
    let params: Vec<(String, String)> = params
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let wf = workflow::load(lua, src, "wf.fnl", &params, ctx)
        .expect("workflow::load")
        .expect("build returned an AST");
    let interp = require_module(lua, "fennel.lib.interp").expect("require interp");
    let number: LuaFunction = interp.get("number_nodes").expect("number_nodes");
    number
        .call::<LuaValue>(wf.clone())
        .expect("number_nodes call");
    let skel: LuaFunction = interp.get("skeleton").expect("skeleton");
    let rows: LuaTable = skel.call(wf).expect("skeleton call");
    let mut out = Vec::new();
    for row in rows.sequence_values::<LuaTable>().flatten() {
        let depth: u32 = row.get("depth").expect("depth");
        let kind: String = row.get("kind").expect("kind");
        let s = match kind.as_str() {
            "action" => {
                let op: String = row.get("op").expect("op");
                let args: LuaTable = row.get("args").expect("args");
                let args: Vec<String> = args
                    .sequence_values::<LuaValue>()
                    .flatten()
                    .map(|v| fmt_value(&v))
                    .collect();
                format!("{depth} action {op} {}", args.join(" "))
            }
            "loop" => {
                let label: Option<String> = row.get("label").unwrap_or(None);
                format!("{depth} loop {}", label.unwrap_or_default())
            }
            "group" => format!(
                "{depth} group {}",
                row.get::<String>("label").expect("label")
            ),
            other => format!("{depth} {other}"),
        };
        out.push(s);
    }
    out
}

/// Index of the first row containing `needle`, with a loud miss.
fn row_index(rows: &[String], needle: &str) -> usize {
    rows.iter()
        .position(|r| r.contains(needle))
        .unwrap_or_else(|| panic!("no skeleton row contains '{needle}': {rows:#?}"))
}

// ─── the flagship path ───────────────────────────────────────────────────────

#[test]
fn flagship_craft_plans_feasibly_with_bank_withdraw() {
    // Bank holds 1 of the 4 copper_ore the dagger's DAG needs: the generator
    // withdraws it and gathers only the remaining 3, then smelts 2 bars and
    // crafts the dagger — 10 actions total:
    //   travel(bank) withdraw travel(copper) gather×3 travel(mining-shop)
    //   craft(bars) travel(weapon-shop) craft(dagger)
    let bank = BankData::from_vec(vec![BankItemView {
        code: "copper_ore".into(),
        quantity: 1,
    }]);
    let result = plan_world(
        CRAFT_SRC,
        Some(bank),
        &world_seed(),
        &[("item", "copper_dagger")],
    )
    .expect("craft.fnl should plan");

    assert!(
        result.feasible,
        "flagship plan must be feasible: {result:?}"
    );
    assert_eq!(
        result.assumptions,
        vec![("copper_ore-gathers".to_string(), 3)],
        "only the remaining 3 ore are gathered (1 came from the bank)"
    );
    assert_eq!(result.actions, 10, "action total: {result:?}");
}

#[test]
fn flagship_skeleton_nests_groups_post_order_and_is_deterministic() {
    // The same build through workflow::load, asserting STRUCTURE: the three
    // "acquire …" groups nest dagger ← bar ← ore (each one level deeper), and
    // the acquisition order is post-order — ore gathered before the bar craft
    // before the dagger craft. Built twice: identical row-by-row (identical
    // inputs must produce an identical AST).
    let lua = setup_world_lua();
    let seed = world_seed();
    let build = || {
        build_skeleton(
            &lua,
            CRAFT_SRC,
            &[("item", "copper_dagger")],
            build_ctx(&lua, &seed, &[("copper_ore", 1)]),
        )
    };
    let rows = build();

    let dagger_group = row_index(&rows, "0 group acquire 1× copper_dagger");
    let bar_group = row_index(&rows, "1 group acquire 2× copper_bar");
    let ore_group = row_index(&rows, "2 group acquire 4× copper_ore");
    assert!(
        dagger_group < bar_group && bar_group < ore_group,
        "group headers nest dagger → bar → ore: {rows:#?}"
    );

    // Post-order acquisition: the ore withdraw + gather loop precede the bar
    // craft, which precedes the dagger craft.
    let withdraw = row_index(&rows, "action withdraw-item [copper_ore,1]");
    let gathers = row_index(&rows, "loop copper_ore-gathers");
    let bar_craft = row_index(&rows, "action craft [copper_bar,2]");
    let dagger_craft = row_index(&rows, "action craft [copper_dagger,1]");
    assert!(
        withdraw < gathers && gathers < bar_craft && bar_craft < dagger_craft,
        "bank-first, then gather, then crafts leaves-first: {rows:#?}"
    );

    assert_eq!(
        rows,
        build(),
        "two identical builds must produce identical skeletons"
    );
}

// ─── no-route errors ─────────────────────────────────────────────────────────

#[test]
fn no_route_errors_name_every_rejected_source() {
    // (a) an item nothing in the world provides.
    let err = plan_world(CRAFT_SRC, None, &world_seed(), &[("item", "unobtanium")])
        .expect_err("unknown item must be a build error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("acquire unobtanium: no route") && msg.contains("no known sources"),
        "unknown item names itself and says no known sources, got: {msg}"
    );

    // (b) gatherable in principle, but the character's skill is too low — the
    // error names the resource, the requirement, and what the character has.
    let mut low_mining = world_seed();
    low_mining.skills.mining = 4;
    let err = plan_world(CRAFT_SRC, None, &low_mining, &[("item", "iron_ore")])
        .expect_err("iron_ore at mining 4 must be a build error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("gather: iron_rocks needs mining 10 (have 4)"),
        "skill-gated gather is named with its reason, got: {msg}"
    );

    // (c) buy-only item priced past the (defaulted-to-ctx.gold) budget.
    let mut poor = world_seed();
    poor.gold = 3;
    let err = plan_world(CRAFT_SRC, None, &poor, &[("item", "ash_wood")])
        .expect_err("ash_wood over budget must be a build error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("buy: 5 gold needed exceeds budget 3"),
        "over-budget buy is named with the amounts, got: {msg}"
    );
}

// ─── the buy path ────────────────────────────────────────────────────────────

#[test]
fn buy_path_within_budget_is_feasible_and_emits_npc_buy() {
    let mut rich = world_seed();
    rich.gold = 100;
    let result = plan_world(CRAFT_SRC, None, &rich, &[("item", "ash_wood")])
        .expect("ash_wood within budget should plan");
    assert!(result.feasible, "buy plan must be feasible: {result:?}");

    let lua = setup_world_lua();
    let rows = build_skeleton(
        &lua,
        CRAFT_SRC,
        &[("item", "ash_wood")],
        build_ctx(&lua, &rich, &[]),
    );
    row_index(&rows, "group acquire 1× ash_wood");
    row_index(&rows, "action npc-buy [ash_wood,1,5]");
}

// ─── the fight path ──────────────────────────────────────────────────────────

#[test]
fn fight_path_hunts_until_target_and_unwinnable_is_no_route() {
    // Winnable seed: eggs come only from chickens (rate 2 → expected 0.5/win),
    // so acquiring 1 egg is a 2-iteration "<code>-hunts" loop in the plan.
    let result = plan_world(CRAFT_SRC, None, &world_seed(), &[("item", "egg")])
        .expect("egg via chicken should plan");
    assert!(result.feasible, "hunt plan must be feasible: {result:?}");
    assert_eq!(
        result.assumptions,
        vec![("egg-hunts".to_string(), 2)],
        "rate-2 egg drop needs 2 predicted wins"
    );

    // Unwinnable seed (no attack): the fight source is rejected with the
    // monster named, and nothing else provides eggs.
    let mut weak = world_seed();
    weak.combat.attack = [0, 0, 0, 0];
    let err = plan_world(CRAFT_SRC, None, &weak, &[("item", "egg")])
        .expect_err("an unwinnable hunt must be a build error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("fight: would lose vs chicken"),
        "the rejected fight names the monster, got: {msg}"
    );
}

// ─── policy override ─────────────────────────────────────────────────────────

/// copper_bar is both craftable and gold-buyable; a [:buy] policy must take
/// the merchant and never touch the recipe.
const BUY_ONLY_BAR: &str = "\
(local acquire (require :fennel.lib.acquire))\n\
{:build (fn [_ ctx] (acquire.item :copper_bar 1 ctx {:policy [:buy]}))}";

#[test]
fn policy_override_buys_instead_of_crafting() {
    let mut rich = world_seed();
    rich.gold = 100;
    let lua = setup_world_lua();
    let rows = build_skeleton(&lua, BUY_ONLY_BAR, &[], build_ctx(&lua, &rich, &[]));
    row_index(&rows, "action npc-buy [copper_bar,1,20]");
    assert!(
        !rows
            .iter()
            .any(|r| r.contains(" craft ") || r.contains("gather")),
        "a [:buy] policy must not emit craft/gather steps: {rows:#?}"
    );
}

// ─── capacity chunking ───────────────────────────────────────────────────────

const GATHER_25_ORE: &str = "\
(local acquire (require :fennel.lib.acquire))\n\
{:build (fn [_ ctx] (acquire.item :copper_ore 25 ctx {}))}";

#[test]
fn chunking_splits_an_oversized_gather_into_bank_trips() {
    // Capacity 10, need 25: two full gather-until-full trips (banked between
    // trips) + a final 5-gather partial — and the whole plan stays feasible
    // (the pack never overflows).
    let mut small_pack = world_seed();
    small_pack.inventory_max_items = 10;
    let result =
        plan_world(GATHER_25_ORE, None, &small_pack, &[]).expect("chunked gather should plan");
    assert!(result.feasible, "chunked plan must be feasible: {result:?}");
    assert_eq!(
        result.assumptions,
        vec![
            ("copper_ore-gathers".to_string(), 5),
            ("copper_ore-gathers-trip-1".to_string(), 10),
            ("copper_ore-gathers-trip-2".to_string(), 10),
        ],
        "two full trips of 10 + a final partial of 5"
    );

    let lua = setup_world_lua();
    let rows = build_skeleton(&lua, GATHER_25_ORE, &[], build_ctx(&lua, &small_pack, &[]));
    let deposits: Vec<&String> = rows
        .iter()
        .filter(|r| r.contains("action deposit-item [copper_ore,10]"))
        .collect();
    assert_eq!(
        deposits.len(),
        2,
        "each full trip banks its load: {rows:#?}"
    );
    row_index(&rows, "loop copper_ore-gathers-trip-1");
    row_index(&rows, "loop copper_ore-gathers-trip-2");
    row_index(&rows, "loop copper_ore-gathers");
}

// ─── host.item_sources ───────────────────────────────────────────────────────

/// A build that returns raw `host.item_sources` results for direct assertions.
const READ_SOURCES: &str = "\
{:build (fn [_ _] {:dust (host.item_sources :magic_dust)\n\
                   :oil (host.item_sources :lamp_oil)\n\
                   :feather (host.item_sources :feather)\n\
                   :unknown (host.item_sources :unobtanium)})}";

#[test]
fn item_sources_reports_craftable_sort_orders_and_unknowns() {
    // A world crafted (pun intended) to exercise the Rust-side sort orders:
    // two resources dropping magic_dust at rates 5/2, two monsters dropping
    // feather at rates 3/2, two gold merchants selling lamp_oil at 7/5, plus
    // a sell-only (no buy_price) lamp_oil listing that must be dropped.
    let monster = |code: &str, drop: &str, rate: u32| MonsterView {
        code: code.into(),
        name: code.into(),
        level: 1,
        hp: 10,
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
        drops: vec![DropRate {
            code: drop.into(),
            rate,
            min_quantity: 1,
            max_quantity: 1,
        }],
    };
    let resource = |code: &str, skill: &str, level: u32, drop: &str, rate: u32| {
        artifacts_core::map::ResourceView {
            code: code.into(),
            level,
            skill: skill.into(),
            drops: vec![DropRate {
                code: drop.into(),
                rate,
                min_quantity: 1,
                max_quantity: 1,
            }],
        }
    };
    let lua = setup_lua(LuaSetupOptions {
        resources: Some(Arc::new(ResourceData::from_vec(vec![
            resource("swamp_pool", "alchemy", 12, "magic_dust", 5),
            resource("dust_grove", "woodcutting", 3, "magic_dust", 2),
        ]))),
        monsters: Some(Arc::new(MonsterData::from_vec(vec![
            monster("hawk", "feather", 3),
            monster("wisp", "feather", 2),
        ]))),
        recipes: Some(Arc::new(RecipeData::from_items(vec![recipe(
            "magic_dust",
            "alchemy",
            1,
            "lamp_oil",
            1,
        )]))),
        npc_items: Some(Arc::new(NpcItemData::from_vec(vec![
            npc_listing("lamp_oil", "trader_b", "gold", Some(7)),
            npc_listing("lamp_oil", "trader_a", "gold", Some(5)),
            npc_listing("lamp_oil", "collector", "gold", None), // sell-only: dropped
        ]))),
        ..Default::default()
    })
    .expect("setup_lua");

    let ctx = lua.create_table().expect("ctx");
    let out = workflow::load(&lua, READ_SOURCES, "sources.fnl", &[], ctx)
        .expect("load")
        .expect("non-nil build");
    let LuaValue::Table(out) = out else {
        panic!("expected a table of item_sources results");
    };

    let dust: LuaTable = out.get("dust").expect("dust");
    assert!(
        dust.get::<bool>("craftable").unwrap(),
        "magic_dust has a recipe"
    );
    let dust_resources: Vec<(String, u32)> = dust
        .get::<LuaTable>("resources")
        .unwrap()
        .sequence_values::<LuaTable>()
        .flatten()
        .map(|t| (t.get("code").unwrap(), t.get("rate").unwrap()))
        .collect();
    assert_eq!(
        dust_resources,
        vec![("dust_grove".to_string(), 2), ("swamp_pool".to_string(), 5)],
        "resources sorted by rate asc (guaranteed-ish yields first)"
    );

    let feather: LuaTable = out.get("feather").expect("feather");
    let feather_monsters: Vec<(String, u32)> = feather
        .get::<LuaTable>("monsters")
        .unwrap()
        .sequence_values::<LuaTable>()
        .flatten()
        .map(|t| (t.get("code").unwrap(), t.get("rate").unwrap()))
        .collect();
    assert_eq!(
        feather_monsters,
        vec![("wisp".to_string(), 2), ("hawk".to_string(), 3)],
        "monsters sorted by (rate asc, code)"
    );

    let oil: LuaTable = out.get("oil").expect("oil");
    let oil_npcs: Vec<(String, u32)> = oil
        .get::<LuaTable>("npcs")
        .unwrap()
        .sequence_values::<LuaTable>()
        .flatten()
        .map(|t| (t.get("npc").unwrap(), t.get("buy_price").unwrap()))
        .collect();
    assert_eq!(
        oil_npcs,
        vec![("trader_a".to_string(), 5), ("trader_b".to_string(), 7)],
        "npcs sorted by buy_price asc; sell-only listings dropped"
    );

    let unknown: LuaTable = out.get("unknown").expect("unknown");
    assert!(!unknown.get::<bool>("craftable").unwrap());
    for list in ["resources", "monsters", "npcs"] {
        assert_eq!(
            unknown.get::<LuaTable>(list).unwrap().raw_len(),
            0,
            "an unknown item has an empty {list} list (not an error)"
        );
    }
}

#[test]
fn item_sources_errors_loudly_with_zero_datasets() {
    let lua = setup_lua(LuaSetupOptions::default()).expect("setup_lua");
    let ctx = lua.create_table().expect("ctx");
    const READS_SOURCES: &str = "{:build (fn [_ _] (host.item_sources :anything))}";
    let err = workflow::load(&lua, READS_SOURCES, "sources.fnl", &[], ctx)
        .expect_err("item_sources with no datasets must error loudly");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("item sources not loaded"),
        "expected the loud 'item sources not loaded' message, got: {msg}"
    );
}
