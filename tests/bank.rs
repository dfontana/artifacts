//! Hermetic tests for bank holdings (M4, `plans/DYNAMIC_WORKFLOWS.md` §5.6):
//! `ctx.bank` for generators at build time, `host.bank()` for a loud
//! start-of-run snapshot read, and `BankData::from_vec`'s duplicate-summing.
use std::sync::Arc;

use artifacts::data::BankData;
use artifacts::lua::{setup_lua, LuaSetupOptions};
use artifacts::planner::{self, PlanSeed};
use artifacts::workflow;
use artifacts_core::bank::BankItemView;
use artifacts_core::step::SkillLevels;
use mlua::prelude::*;

mod common;
use common::{make_map, make_resources, INV_MAX};

/// A workflow MODULE whose `build` reads `ctx.bank`: withdraw
/// `min(NEED, ctx.bank[copper_ore])` from the bank, then gather the remainder
/// (a deterministic `repeat_n`, since the withdrawal amount is already known at
/// build time from the bank snapshot). With no bank data, `ctx.bank` is empty
/// (`workflow::attach_bank`'s documented quiet-empty-table path), so `withdraw`
/// is 0 and the whole `NEED` is gathered instead — proving both branches without
/// two separate workflow sources.
const NEED: u32 = 5;
const WORKFLOW_BANK_OR_GATHER: &str = "\
(local {: seq : action : repeat_n} (require :fennel.lib.interp))\n\
{:build (fn [_ ctx]\n\
          (let [need 5\n\
                have (or (. ctx.bank :copper_ore) 0)\n\
                withdraw (math.min need have)\n\
                gather-n (- need withdraw)]\n\
            (if (> withdraw 0)\n\
                (seq (action :withdraw-item [:copper_ore withdraw])\n\
                     (repeat_n gather-n (action :gather)))\n\
                (seq (repeat_n gather-n (action :gather))))))}";

/// A copper_rocks tile at the seed origin (0,0), mining level 1, so `:gather`'s
/// cost/sim have a resource to read and the skill gate passes.
fn copper_seed() -> PlanSeed {
    PlanSeed {
        inventory_max_items: INV_MAX,
        skills: SkillLevels {
            mining: 1,
            ..Default::default()
        },
        ..PlanSeed::default()
    }
}

fn copper_resources() -> Arc<artifacts::data::ResourceData> {
    make_resources(&[("copper_rocks", "mining", 1, "copper_ore")])
}

#[test]
fn ctx_bank_drives_the_withdraw_gather_split() {
    let bank = BankData::from_vec(vec![BankItemView {
        code: "copper_ore".into(),
        quantity: 3,
    }]);
    let result = planner::plan(
        WORKFLOW_BANK_OR_GATHER,
        Some(make_map(2, 2, &[(0, 0, "resource", "copper_rocks")])),
        None,
        Some(copper_resources()),
        None,
        None,
        Some(Arc::new(bank)),
        &copper_seed(),
        &[],
    )
    .expect("plan should succeed with bank data supplied");

    assert!(result.feasible, "plan should be feasible: {result:?}");
    // 3 in the bank -> withdraw 3 (1 action) + gather the remaining 2 (2 actions).
    assert_eq!(
        result.actions,
        1 + (NEED - 3),
        "withdraw 3 then gather the remaining 2: {result:?}"
    );
}

#[test]
fn no_bank_data_takes_the_all_gather_path() {
    let result = planner::plan(
        WORKFLOW_BANK_OR_GATHER,
        Some(make_map(2, 2, &[(0, 0, "resource", "copper_rocks")])),
        None,
        Some(copper_resources()),
        None,
        None,
        None, // no BankData supplied -> ctx.bank is empty, not an error
        &copper_seed(),
        &[],
    )
    .expect("plan should still succeed with no bank data (ctx.bank is just empty)");

    assert!(result.feasible, "plan should be feasible: {result:?}");
    // Nothing in the bank -> withdraw 0 (no withdraw-item action), gather all 5.
    assert_eq!(
        result.actions, NEED,
        "no bank data means the all-gather path: {result:?}"
    );
}

#[test]
fn host_bank_errors_loudly_with_no_bank_data() {
    // A tiny workflow whose build calls (host.bank) directly (not ctx.bank) —
    // the loud, start-of-run-snapshot path. No `bank` supplied to setup_lua, so
    // the call inside `build` must fail, and `workflow::load` surfaces that as
    // an Err (build errors propagate through `load`).
    let lua = setup_lua(LuaSetupOptions::default()).expect("setup_lua failed");
    let ctx = lua.create_table().expect("ctx table");
    const WORKFLOW_READS_HOST_BANK: &str = "{:build (fn [_ _] (host.bank))}";

    let err = workflow::load(&lua, WORKFLOW_READS_HOST_BANK, "reads-bank.fnl", &[], ctx)
        .expect_err("host.bank() with no bank data must error loudly");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("bank data not loaded"),
        "expected the loud 'bank data not loaded' message, got: {msg}"
    );
}

#[test]
fn host_bank_returns_supplied_quantities() {
    let bank = BankData::from_vec(vec![
        BankItemView {
            code: "copper_ore".into(),
            quantity: 7,
        },
        BankItemView {
            code: "iron_ore".into(),
            quantity: 2,
        },
    ]);
    let lua = setup_lua(LuaSetupOptions {
        bank: Some(Arc::new(bank)),
        ..Default::default()
    })
    .expect("setup_lua failed");
    let ctx = lua.create_table().expect("ctx table");
    const WORKFLOW_READS_HOST_BANK: &str = "{:build (fn [_ _] (host.bank))}";

    let ast = workflow::load(&lua, WORKFLOW_READS_HOST_BANK, "reads-bank.fnl", &[], ctx)
        .expect("workflow::load should succeed")
        .expect("build returned a value, not nil");
    let LuaValue::Table(t) = ast else {
        panic!("expected host.bank() to return a table, got {ast:?}");
    };
    assert_eq!(t.get::<u32>("copper_ore").unwrap(), 7);
    assert_eq!(t.get::<u32>("iron_ore").unwrap(), 2);
}

#[test]
fn bank_data_from_vec_sums_duplicate_codes() {
    let bank = BankData::from_vec(vec![
        BankItemView {
            code: "copper_ore".into(),
            quantity: 3,
        },
        BankItemView {
            code: "copper_ore".into(),
            quantity: 4,
        },
        BankItemView {
            code: "iron_ore".into(),
            quantity: 1,
        },
    ]);
    assert_eq!(bank.get(&"copper_ore".into()), 7, "duplicate codes summed");
    assert_eq!(bank.get(&"iron_ore".into()), 1);
    assert_eq!(bank.get(&"gold_ore".into()), 0, "missing code reads as 0");
    assert_eq!(bank.len(), 2);
}
