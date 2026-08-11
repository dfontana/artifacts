//! Shared mock fixtures for the hermetic integration tests. One copy of the
//! map/character/response builders, so an API-schema tweak (the fight envelope
//! already changed shape once) is updated in one place.
#![allow(dead_code)] // each test crate uses a subset

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread::JoinHandle;

use artifacts::character::Character;
use artifacts::character::SharedView;
use artifacts::context::ExecutionContext;
use artifacts::data::ResourceData;
use artifacts::driver::Driver;
use artifacts::live;
use artifacts_core::drop::DropRate;
use artifacts_core::map::{
    AccessSchema, GameMap, InteractionSchema, MapAccessType, MapContentSchema, MapTile,
    ResourceView,
};
use artifacts_core::step::CharacterView;

/// Inventory cap used by every mock character.
pub const INV_MAX: u32 = 10;

/// A clear `w`×`h` overworld grid with the given `(x, y, content_type, code)`
/// content tiles — enough for A* hops and `host.find_tile` lookups.
pub fn make_map(w: i32, h: i32, content: &[(i32, i32, &str, &str)]) -> Arc<GameMap> {
    let mut m = GameMap::new();
    for y in 0..h {
        for x in 0..w {
            let interactions = content
                .iter()
                .find(|(cx, cy, _, _)| (*cx, *cy) == (x, y))
                .map(|(_, _, kind, code)| InteractionSchema {
                    content: Some(MapContentSchema {
                        content_type: (*kind).into(),
                        code: (*code).into(),
                    }),
                })
                .unwrap_or_default();
            m.insert(MapTile {
                map_id: y * 10 + x,
                name: format!("{x},{y}"),
                skin: "grass".into(),
                x,
                y,
                layer: "overworld".into(),
                access: AccessSchema {
                    access_type: MapAccessType::Standard,
                },
                interactions,
            });
        }
    }
    Arc::new(m)
}

/// Resource reference data backing `host.active_resource`, for tests whose
/// workflow gathers a resource tile built by `make_map`. Each spec is
/// `(code, skill, level, primary_drop)` — every fixture now declares the skill
/// that gates the gather and a single rate-1 primary drop (the real item a
/// gather yields, e.g. `copper_rocks` → `copper_ore`), matching the strict
/// `ResourceView` shape M3 introduced.
pub fn make_resources(specs: &[(&str, &str, u32, &str)]) -> Arc<ResourceData> {
    Arc::new(ResourceData::from_vec(
        specs
            .iter()
            .map(|(code, skill, level, drop)| ResourceView {
                code: (*code).into(),
                level: *level,
                skill: (*skill).to_string(),
                drops: vec![DropRate {
                    code: (*drop).into(),
                    rate: 1,
                    min_quantity: 1,
                    max_quantity: 1,
                }],
            })
            .collect(),
    ))
}

/// An execution context backed by API-shaped map and resource records.
/// Each call returns fresh reference-data snapshots for planner/live tests.
pub fn resource_context(
    w: i32,
    h: i32,
    content: &[(i32, i32, &str, &str)],
    resources: &[(&str, &str, u32, &str)],
) -> ExecutionContext {
    ExecutionContext {
        map: Some(make_map(w, h, content)),
        resources: Some(make_resources(resources)),
        ..Default::default()
    }
}

/// The mock character schema: `inv_count` copper_ore in slot 1 (0 = empty).
/// Carries a fire attack so `is_winnable` is true against a stat-less monster
/// (which is what exercises the when-SKIP path in the chickens shape).
/// Delegates to `char_json_gold` with gold 0 — the common case, since most
/// hermetic tests don't exercise gold.
pub fn char_json(x: i32, y: i32, inv_count: u32, hp: u32) -> serde_json::Value {
    char_json_gold(x, y, inv_count, hp, 0)
}

/// Same as `char_json`, plus a `gold` field — for tests (e.g. buy-from-merchant)
/// whose run pass reads `host.view`'s gold each iteration via `CharacterView`.
pub fn char_json_gold(x: i32, y: i32, inv_count: u32, hp: u32, gold: u32) -> serde_json::Value {
    let mut inventory = vec![];
    if inv_count > 0 {
        inventory.push(serde_json::json!({"slot": 1, "code": "copper_ore", "quantity": inv_count}));
    }
    serde_json::json!({
        "name": "kael", "x": x, "y": y, "hp": hp, "max_hp": 100, "level": 1,
        "attack_fire": 50, "gold": gold,
        "inventory_max_items": INV_MAX, "inventory": inventory
    })
}

/// The standard action-response envelope: cooldown + character (+ empty details).
pub fn response(cooldown: f64, character: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "data": {
            "cooldown": {
                "total_seconds": cooldown, "remaining_seconds": cooldown,
                "started_at": "2024-01-01T00:00:00Z", "expiration": "2024-01-01T00:01:00Z",
                "reason": "action"
            },
            "character": character,
            "details": { "items": [] }
        }
    }))
    .unwrap()
}

/// The `action/fight` envelope, which nests xp/gold/drops per character.
pub fn fight_win(character: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "data": {
            "cooldown": {
                "total_seconds": 10.0, "remaining_seconds": 10.0,
                "started_at": "2024-01-01T00:00:00Z", "expiration": "2024-01-01T00:01:00Z",
                "reason": "fight"
            },
            "character": character,
            "fight": {
                "xp": 10, "gold": 1, "drops": [], "turns": 1, "result": "win",
                "logs": [], "monster_blocked_hits": {}, "player_blocked_hits": {}
            }
        }
    }))
    .unwrap()
}

/// Spin up the production scheduler wiring (`live::spawn_scheduler`) around any
/// `Driver` (mock or live), returning the `Character` handle, the shared view,
/// and the scheduler's join handle. Drop the `Character` (and any Lua state
/// holding it) to end the scheduler, then join the handle.
pub fn spawn_mock<D: Driver>(
    driver: D,
    initial: CharacterView,
) -> (Character, SharedView, JoinHandle<()>) {
    let view = SharedView::new(initial);
    let abort = Arc::new(AtomicBool::new(false));
    let (character, handle) = live::spawn_scheduler(Box::new(driver), view.clone(), abort);
    (character, view, handle)
}
