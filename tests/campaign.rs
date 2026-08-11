//! M7 campaign-loop tests (`plans/DYNAMIC_WORKFLOWS.md` §8 M7).
//!
//! Everything is driven through the one public entry point,
//! `campaign::run_until_done`, CLI-shaped exactly as `artifacts run <wf>
//! <character> k=v... --until-done` wires it (`src/main.rs`) — per the
//! entrypoint-driven testing standard. Fixtures use `:rest` (no map/resource
//! data needed) so the tests exercise the harness loop itself, not the acquire
//! generator (already covered by `tests/acquire.rs`):
//!   - a two-chunk campaign: `build` rests once, then sees a fresh (already
//!     rested) live view and returns nil — exactly one `build` call per
//!     iteration, exactly one run, clean stop on the nil sentinel;
//!   - a blocker abort: the fresh build's plan is infeasible (a skill-gated
//!     gather) — the campaign aborts loudly naming the blocker and never sends
//!     a single request to the driver;
//!   - exhaustion: a `build` that never returns nil burns `--max-iterations`
//!     and then errors, naming the flag.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use artifacts::campaign;
use artifacts::context::ExecutionContext;
use artifacts::data::BankData;
use artifacts::driver::mock::{CannedResponse, MockDriver, RequestLog};
use artifacts::driver::Driver;
use artifacts_core::step::CharacterView;

mod common;
use common::{char_json, make_map, make_resources, response, INV_MAX};

/// A workflow module whose `build` rests while under max hp, and returns nil
/// (the M7 done-sentinel) once the live `ctx` shows full hp — state-dependent
/// on exactly the field the campaign loop refetches each iteration.
const REST_UNTIL_FULL_SRC: &str = "(local {: seq : action} (require :fennel.lib.interp))\n\
     {:build (fn [_ ctx] (if (< ctx.hp ctx.max-hp) (seq (action :rest)) nil))}";

/// A minimal character snapshot at `hp`/`max_hp`, everything else defaulted
/// the way the mock fixtures elsewhere in this suite do.
fn view_at_hp(hp: u32, max_hp: u32) -> CharacterView {
    CharacterView {
        name: "kael".into(),
        x: 0,
        y: 0,
        hp,
        max_hp,
        level: 1,
        inventory_max_items: INV_MAX,
        inventory: vec![],
        ..Default::default()
    }
}

#[test]
fn two_chunk_campaign_builds_twice_runs_once_and_stops_on_nil() {
    // Iteration 1: hp 50/100 — build rests. Iteration 2: the campaign refetches
    // and this time the live view is already at full hp (simulating the rest
    // having landed server-side) — build returns nil and the loop stops.
    let fetch_count = Arc::new(AtomicU32::new(0));
    let fetch_count_hook = fetch_count.clone();
    // Every iteration's driver hands out its own request log before being
    // boxed away; stash each one so we can prove exactly one action request
    // (the rest, in iteration 1) went out across the whole campaign.
    let logs: Arc<Mutex<Vec<RequestLog>>> = Arc::default();
    let logs_hook = logs.clone();

    let result = campaign::run_until_done(
        REST_UNTIL_FULL_SRC,
        &ExecutionContext::default(),
        &[],
        10,
        move || {
            let n = fetch_count_hook.fetch_add(1, Ordering::SeqCst);
            let mut driver = MockDriver::new();
            driver.push_response(CannedResponse::new(
                "action/rest",
                200,
                response(5.0, char_json(0, 0, 0, 100)),
            ));
            logs_hook.lock().unwrap().push(driver.request_log());
            let view = if n == 0 {
                view_at_hp(50, 100)
            } else {
                view_at_hp(100, 100)
            };
            Ok((
                Box::new(driver) as Box<dyn Driver>,
                view,
                BankData::default(),
            ))
        },
    );

    assert!(result.is_ok(), "campaign should end cleanly: {result:?}");
    assert_eq!(
        fetch_count.load(Ordering::SeqCst),
        2,
        "build/fetch must run exactly twice: chunk 1 (rest), chunk 2 (nil)"
    );

    let logs = logs.lock().unwrap();
    assert_eq!(logs.len(), 2, "one driver constructed per iteration");
    assert_eq!(
        logs[0].lock().unwrap().len(),
        1,
        "iteration 1 must send exactly one request (the rest)"
    );
    assert!(
        logs[1].lock().unwrap().is_empty(),
        "iteration 2's build returns nil before any request is sent"
    );
}

#[test]
fn blocker_abort_stops_before_running_and_names_the_blocker() {
    // A gather workflow against a resource requiring mining 10 while the
    // character's mining is 0 — the plan pass flags a hard blocker
    // (`--pending-blocker`, `fennel/lib/actions.fnl`'s :gather :sim), so the
    // campaign must abort loudly, never invoke `interp.run`, and never loop
    // to a second iteration.
    const SRC: &str = "(local {: seq : action} (require :fennel.lib.interp))\n\
        {:build (fn [_ _] (seq (action :travel-to [1 0]) (action :gather)))}";

    let reference = ExecutionContext {
        map: Some(make_map(
            2,
            1,
            &[(1, 0, "resource", "iron_rocks"), (0, 0, "bank", "bank")],
        )),
        resources: Some(make_resources(&[("iron_rocks", "mining", 10, "iron_ore")])),
        ..Default::default()
    };

    let fetch_count = Arc::new(AtomicU32::new(0));
    let fetch_count_hook = fetch_count.clone();
    let log_slot: Arc<Mutex<Option<RequestLog>>> = Arc::default();
    let log_slot_hook = log_slot.clone();

    let result = campaign::run_until_done(SRC, &reference, &[], 10, move || {
        fetch_count_hook.fetch_add(1, Ordering::SeqCst);
        let driver = MockDriver::new();
        *log_slot_hook.lock().unwrap() = Some(driver.request_log());
        let mut view = view_at_hp(100, 100);
        view.mining_level = 0; // below the resource's required level 10
        Ok((
            Box::new(driver) as Box<dyn Driver>,
            view,
            BankData::default(),
        ))
    });

    let err = result.expect_err("an infeasible plan must abort the campaign");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("mining") && msg.contains("10"),
        "error should name the skill blocker, got: {msg}"
    );
    assert_eq!(
        fetch_count.load(Ordering::SeqCst),
        1,
        "fetch runs once; the plan gate aborts before a second iteration"
    );
    let log = log_slot.lock().unwrap().clone().expect("fetch ran once");
    assert!(
        log.lock().unwrap().is_empty(),
        "no request should reach the driver when the plan gate blocks"
    );
}

#[test]
fn exhaustion_stops_at_max_iterations_and_names_the_flag() {
    // A build that unconditionally emits work (never nil, regardless of live
    // state) must burn exactly `--max-iterations` iterations and then error,
    // naming the flag (`DYNAMIC_WORKFLOWS` §8 M7's MAX-ITERS-analogue guard).
    const SRC: &str = "(local {: seq : action} (require :fennel.lib.interp))\n\
        {:build (fn [_ _] (seq (action :rest)))}";

    let fetch_count = Arc::new(AtomicU32::new(0));
    let fetch_count_hook = fetch_count.clone();

    let result = campaign::run_until_done(SRC, &ExecutionContext::default(), &[], 3, move || {
        fetch_count_hook.fetch_add(1, Ordering::SeqCst);
        let mut driver = MockDriver::new();
        driver.push_response(CannedResponse::new(
            "action/rest",
            200,
            response(5.0, char_json(0, 0, 0, 100)),
        ));
        Ok((
            Box::new(driver) as Box<dyn Driver>,
            view_at_hp(50, 100),
            BankData::default(),
        ))
    });

    let err = result.expect_err("a build that never returns nil must exhaust the cap");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("--max-iterations"),
        "exhaustion error should name the flag, got: {msg}"
    );
    assert_eq!(
        fetch_count.load(Ordering::SeqCst),
        3,
        "exactly 3 iterations must run before exhaustion"
    );
}
