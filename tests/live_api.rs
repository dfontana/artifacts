//! Live integration tests against the real Artifacts MMO API.
//!
//! These are `#[ignore]`d by default: they hit the network, require a valid
//! ARTIFACTS_SECRET (or ARTIFACTS_TOKEN), mutate real game state, and incur real
//! cooldowns. They must run SEQUENTIALLY (one character cannot act concurrently):
//!
//!   ARTIFACTS_SECRET=... cargo test -p artifacts-tests --test live_api \
//!     -- --ignored --test-threads=1 --nocapture
//!
//! They drive the real building blocks — HttpDriver, Core::handle_response,
//! CharacterView deserialization, GameMap + A* — to find where the
//! implementation diverges from the live API.

use artifacts::driver::{http::HttpDriver, Driver, DriverResult};
use artifacts_core::{
    combat::{simulate, CombatStats},
    cooldown::Cooldown,
    error::GameError,
    ident::{Code, ContentType},
    machine::{Core, Progress},
    step::{CharacterView, FightOutcome, Intent, Outcome, OutcomeKind, Step},
};

const CHARACTER: &str = "nillinbot";

// Known fixtures from the live overworld (verified via the API):
const COPPER: (i32, i32) = (2, 0); // copper_rocks, mining level 1
const BANK: (i32, i32) = (4, 1);
const SPAWN: (i32, i32) = (0, 0);

fn driver() -> HttpDriver {
    HttpDriver::from_env(CHARACTER).expect("ARTIFACTS_SECRET/ARTIFACTS_TOKEN must be set")
}

/// Drive a single intent through Core + HttpDriver, sleeping through any cooldown,
/// exactly as the scheduler does. Returns the parsed Outcome.
fn drive(
    driver: &mut HttpDriver,
    core: &mut Core,
    intent: Intent,
) -> Result<artifacts_core::step::Outcome, GameError> {
    core.enqueue(intent);
    loop {
        let now = driver.current_time();
        match core.next_step(now) {
            Step::Request { method, path, body } => {
                match driver.execute(Step::Request { method, path, body }) {
                    DriverResult::Response { status, body } => {
                        let after = driver.current_time();
                        match core.handle_response(status, &body, after)? {
                            Progress::Complete(outcome) => return Ok(*outcome),
                            Progress::Retry => continue, // transient (499/486/429)
                            Progress::NoOp => {
                                // 490 no-op: report success with the live view.
                                let character =
                                    driver.fetch_character().map_err(GameError::Network)?;
                                return Ok(Outcome {
                                    cooldown: Cooldown::none(),
                                    character,
                                    kind: OutcomeKind::NoOp,
                                });
                            }
                        }
                    }
                    DriverResult::Error { message } => return Err(GameError::Network(message)),
                    other => panic!("unexpected driver result: {other:?}"),
                }
            }
            Step::Sleep { until, reason } => {
                eprintln!("  sleeping for cooldown ({reason:?})...");
                driver.execute(Step::Sleep { until, reason });
            }
            Step::Done => panic!("Core returned Done with an intent queued"),
            Step::FetchData { .. } => unreachable!(),
        }
    }
}

fn inv_qty(view: &CharacterView, code: &str) -> u32 {
    view.inventory
        .iter()
        .filter_map(|s| s.as_ref())
        .filter(|i| i.code.as_str() == code)
        .map(|i| i.quantity)
        .sum()
}

// ─── Test 1: character fetch + deserialization ───────────────────────────────

#[test]
#[ignore = "live network"]
fn live_fetch_character() {
    let d = driver();
    let view = d.fetch_character().expect("fetch_character");

    assert_eq!(view.name.as_str(), CHARACTER, "character name round-trips");
    assert!(
        view.max_hp > 0,
        "max_hp should be populated, got {}",
        view.max_hp
    );
    assert!(
        !view.inventory.is_empty(),
        "inventory slots should deserialize (live returns a fixed slot array)"
    );

    eprintln!(
        "character '{}' at ({}, {}), hp {}/{}, {} inventory slots, max_items={}",
        view.name,
        view.x,
        view.y,
        view.hp,
        view.max_hp,
        view.inventory.len(),
        view.inventory_max_items
    );
    eprintln!(
        "  inventory_count={} slots_used={} full={}",
        view.inventory_count(),
        view.inventory_slots_used(),
        view.inventory_full()
    );
}

// ─── Test 2: map fetch + A* against real terrain ─────────────────────────────

#[test]
#[ignore = "live network"]
fn live_map_and_pathfinding() {
    let d = driver();
    let map = d.fetch_overworld_map().expect("fetch_overworld_map");

    assert!(map.tile_count() > 0, "overworld map should have tiles");
    eprintln!("loaded {} overworld tiles", map.tile_count());

    // Known content from the live map.
    let copper = map.get(COPPER.0, COPPER.1).expect("copper tile exists");
    let content = copper
        .interactions
        .content
        .as_ref()
        .expect("copper tile has content");
    assert_eq!(
        content.code,
        Code::from("copper_rocks"),
        "tile (2,0) is copper_rocks"
    );

    assert!(map.is_walkable(COPPER.0, COPPER.1), "copper tile walkable");
    assert!(map.is_walkable(BANK.0, BANK.1), "bank tile walkable");

    // A* hop counts the server would also produce on a clear route.
    let spawn_to_copper = map.path_hops(SPAWN, COPPER);
    let copper_to_bank = map.path_hops(COPPER, BANK);
    eprintln!("path_hops spawn->copper={spawn_to_copper}, copper->bank={copper_to_bank}");
    assert_eq!(spawn_to_copper, 2, "spawn->copper should be 2 hops");
    assert_eq!(copper_to_bank, 3, "copper->bank should be 3 hops");
}

// ─── Test 3: real action cycle (move + gather) ───────────────────────────────

#[test]
#[ignore = "live network; mutates state; ~30s of cooldowns"]
fn live_action_cycle() {
    let mut d = driver();
    let mut core = Core::new();

    // Sync Core's clock baseline by reading the current character (also proves
    // we are not starting mid-cooldown).
    let start = d.fetch_character().expect("fetch_character");
    eprintln!(
        "start: at ({}, {}), copper held={}",
        start.x,
        start.y,
        inv_qty(&start, "copper_ore")
    );

    // 1. Move to the copper tile. 490 (already there) is now a benign no-op, so
    //    this succeeds whether or not the character was already on the tile.
    eprintln!("moving to copper {COPPER:?}...");
    let o = drive(
        &mut d,
        &mut core,
        Intent::Move {
            x: COPPER.0,
            y: COPPER.1,
        },
    )
    .expect("move (490 should be a no-op, not an error)");
    assert_eq!(o.character.x, COPPER.0, "x is at copper");
    assert_eq!(o.character.y, COPPER.1, "y is at copper");
    eprintln!(
        "  at copper; cooldown {:.0}s (0 = was already there)",
        o.cooldown.total_seconds
    );

    // 2. Gather copper. This waits out the move cooldown first.
    eprintln!("gathering copper...");
    let before = inv_qty(&d.fetch_character().expect("refetch"), "copper_ore");
    let outcome = drive(&mut d, &mut core, Intent::Gather).expect("gather");
    let after = inv_qty(&outcome.character, "copper_ore");

    eprintln!(
        "  gather cooldown {:.0}s, copper_ore {before} -> {after}",
        outcome.cooldown.total_seconds
    );
    assert!(
        after > before,
        "gather should add copper_ore (was {before}, now {after})"
    );
    assert!(
        outcome.cooldown.total_seconds > 0.0,
        "gather returns a cooldown"
    );
}

// ─── Test 4: combat — live fight parses, and matches the simulator ───────────

/// The load-bearing combat test. It proves two things at once against the real
/// server:
///   1. `Core::handle_response` parses the live `action/fight` schema (which
///      nests the character array and the xp/gold/drops, unlike other actions).
///   2. `core::combat::simulate` (deterministic, crits off) predicts the same
///      turn count and win/lose result the server produces — i.e. the plan pass
///      tells the truth about combat.
#[test]
#[ignore = "live network; mutates state; ~1min fight cooldown"]
fn live_fight_matches_simulation() {
    let mut d = driver();
    let mut core = Core::new();

    // Find the chicken tile from map content (no hardcoded coordinates).
    let map = d.fetch_overworld_map().expect("fetch map");
    let (cx, cy) = map
        .nearest_content(
            (0, 0),
            &ContentType::from("monster"),
            &Code::from("chicken"),
        )
        .expect("a chicken tile exists on the overworld");
    eprintln!("chicken tile at ({cx}, {cy})");
    drive(&mut d, &mut core, Intent::Move { x: cx, y: cy }).expect("move to chicken");

    // Rest, then snapshot the HP we'll actually go into the fight with.
    let _ = drive(&mut d, &mut core, Intent::Rest);
    let before = d.fetch_character().expect("refetch before fight");
    eprintln!("entering fight at hp {}/{}", before.hp, before.max_hp);

    // Predict from the live character + cached monster stats.
    let chicken = d
        .fetch_all_monsters()
        .expect("fetch monsters")
        .into_iter()
        .find(|m| m.code == Code::from("chicken"))
        .expect("chicken in /monsters");
    let pred = simulate(&CombatStats::from(&before), &chicken.combat_stats());
    eprintln!(
        "simulated: {:?} in {} turns, {} hp left",
        pred.result, pred.turns, pred.player_hp_remaining
    );

    // Fight for real — this is the path that exercises the schema fix.
    let outcome = drive(&mut d, &mut core, Intent::Fight).expect("fight parses + completes");
    let OutcomeKind::Fight(f) = outcome.kind else {
        panic!("expected a Fight outcome");
    };
    eprintln!(
        "live:      {:?} in {} turns, character now {} hp",
        f.result, f.turns, outcome.character.hp
    );

    assert_eq!(
        f.result,
        FightOutcome::Win,
        "nillinbot should beat a chicken"
    );
    assert_eq!(
        pred.turns, f.turns,
        "simulator turn count must match the live fight"
    );
    assert_eq!(
        pred.player_hp_remaining as u32, outcome.character.hp,
        "simulator HP-remaining must match the live final HP"
    );
}
