//! Hermetic test that `host.find_tile` anchors "nearest" to the `origin`
//! passed to `setup_lua` (the plan/direct path: `character: None`, explicit
//! `Some(origin)`), not to the `(0,0)` `None` default.
//!
//! Why this exists: `tests/tui_alignment.rs::run_and_capture` passes
//! `origin: None` and starts its character at `(0,0)`, so origin-aware
//! `find_tile` is never exercised hermetically there. Because `(0,0)` (the
//! `None` default) coincides with the character's start, passing `None` looks
//! identical to passing the real origin — masking any regression in origin
//! threading. The only test covering `run_worker`'s combined path (which passes
//! `Some((initial_view.x, initial_view.y))`) is `#[ignore]`d (live network).
//!
//! This test targets the plan/direct path (`character: None`, `Some(origin)`),
//! which `src/planner.rs::plan` uses and which stays stable after CAND-12
//! (CAND-12 makes the RUN path's origin dynamic; it does not touch the plan
//! path's frozen-seed origin). It passes two distinct non-`(0,0)` origins and
//! asserts `find_tile` returns a different nearest tile for each — proving the
//! `origin` parameter threads through `setup_lua` into `find_tile`'s `find_from`
//! anchor and is actually used, vs. stuck at the `(0,0)` default.

use std::sync::Arc;

use artifacts::lua::{eval_fennel, setup_lua, LuaSetupOptions};
use artifacts_core::map::GameMap;
use mlua::prelude::*;

mod common;
use common::make_map;

// ─── map: two `:bank :bank` tiles at opposite ends of a 7×2 grid ───────────
//
// From different origins each becomes the nearest, so a correct origin anchor
// flips the result. An anchor stuck at `(0,0)` would always return `BANK_A`
// (Manhattan 1 from `(0,0)`, vs 6 to `BANK_B`), hiding the regression.
const BANK_A: (i32, i32) = (1, 0);
const BANK_B: (i32, i32) = (5, 1);

fn two_bank_map() -> Arc<GameMap> {
    make_map(
        7,
        2,
        &[
            (BANK_A.0, BANK_A.1, "bank", "bank"),
            (BANK_B.0, BANK_B.1, "bank", "bank"),
        ],
    )
}

/// Build a fresh Lua state (plan path: no character) with the given origin and
/// evaluate `(host.find_tile :bank :bank)`, returning the resolved `(x, y)`.
///
/// `origin` is captured into the `find_tile` closure at `setup_lua` time, so
/// each origin needs its own state.
fn find_bank_from(origin: Option<(i32, i32)>) -> (i32, i32) {
    let lua = setup_lua(LuaSetupOptions {
        map: Some(two_bank_map()),
        origin,
        ..Default::default()
    })
    .expect("setup_lua with origin");
    let v =
        eval_fennel(&lua, "(host.find_tile :bank :bank)", "find-bank.fnl").expect("eval find_tile");
    let t: LuaTable = LuaTable::from_lua(v, &lua).expect("find_tile returned a table");
    let x: i32 = t.get("x").expect("find_tile result has x");
    let y: i32 = t.get("y").expect("find_tile result has y");
    (x, y)
}

#[test]
fn find_tile_anchors_to_passed_origin() {
    // Baseline: `None` anchors to the `(0,0)` default → BANK_A is nearest
    // (Manhattan 1 vs 6). This is the exact masking `tui_alignment.rs` relies
    // on: with the character also at `(0,0)`, `None` looks like the real origin.
    assert_eq!(find_bank_from(None), BANK_A, "None origin anchors to (0,0)");

    // Origin (0,1): BANK_A dist 2, BANK_B dist 5 → BANK_A. A non-`(0,0)` origin
    // that still resolves to BANK_A, so this case alone wouldn't catch a stuck
    // anchor — the next case is the discriminator.
    assert_eq!(
        find_bank_from(Some((0, 1))),
        BANK_A,
        "origin (0,1) → BANK_A (dist 2 < 5)"
    );

    // Origin (6,0): BANK_A dist 5, BANK_B dist 2 → BANK_B. THIS is the
    // discriminator: if `origin` were ignored or stuck at the `(0,0)` default,
    // `find_tile` would return BANK_A here. Returning BANK_B proves the passed
    // origin threads through `setup_lua` into `find_tile`'s `find_from` and is
    // actually used.
    assert_eq!(
        find_bank_from(Some((6, 0))),
        BANK_B,
        "origin (6,0) → BANK_B (proves origin is threaded, not stuck at (0,0))"
    );

    // Defining proof: the result CHANGES with origin (BANK_A → BANK_B), which is
    // impossible if `find_from` were frozen at any single value.
    assert_ne!(
        find_bank_from(Some((0, 1))),
        find_bank_from(Some((6, 0))),
        "find_tile result must vary with origin"
    );
}
