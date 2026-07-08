use std::time::Instant;

use crate::ident::{CharacterName, Code};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

#[derive(Debug)]
pub enum Step {
    Request {
        method: Method,
        path: String,
        body: Option<Vec<u8>>,
    },
    Sleep {
        until: Instant,
        reason: SleepReason,
    },
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepReason {
    Cooldown,
    RateLimit,
}

pub use crate::wire::Intent;

/// An inventory slot (the character's `inventory` array). Always carries a slot
/// index on the live API; empty slots have `code: ""` and `quantity: 0` (they
/// are objects, never JSON null — see `occupied_items`).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct InventoryItem {
    pub slot: u32,
    pub code: Code,
    pub quantity: u32,
}

/// A dropped/gained item in an action's `details` (DropSchema / SimpleItemSchema).
/// Unlike an inventory slot, it has no `slot` field — just code + quantity.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct DropItem {
    pub code: Code,
    pub quantity: u32,
}

/// A point-in-time snapshot of character data returned from the server.
#[derive(Debug, Clone, serde::Deserialize, Default)]
pub struct CharacterView {
    pub name: CharacterName,
    pub x: i32,
    pub y: i32,
    pub hp: u32,
    pub max_hp: u32,
    pub level: u32,
    pub inventory_max_items: u32,
    #[serde(default)]
    pub inventory: Vec<InventoryItem>,

    // ─── header + cooldown-bar source (TUI) ───────────────────────────────────
    // All #[serde(default)] like the combat stats, so every mock/fixture that
    // omits them still deserializes. The live CharacterSchema returns all of
    // these; because fetch_character and every action response deserialize the
    // same CharacterView, they populate SharedView identically idle or running
    // (see plans/TUI.md §3.8). Keys match the API 1:1 (no serde renames).
    #[serde(default)]
    pub xp: u32,
    #[serde(default)]
    pub max_xp: u32,
    #[serde(default)]
    pub gold: u32,
    /// Whole seconds of cooldown remaining at fetch time.
    #[serde(default)]
    pub cooldown: u32,
    /// RFC3339 cooldown expiration; `""` when idle. The cooldown bar is derived
    /// from this vs the wall clock, not stored separately.
    #[serde(default)]
    pub cooldown_expiration: String,

    // ─── combat stats ─────────────────────────────────────────────────────────
    // All default to 0 so the many non-combat fixtures/mocks that omit them still
    // deserialize. `core::combat::CombatStats: From<&CharacterView>` reads these.
    #[serde(default)]
    pub initiative: i32,
    #[serde(default)]
    pub critical_strike: i32,
    /// Reduces fight cooldown (see `cooldown::formulas::fight`).
    #[serde(default)]
    pub haste: i32,
    #[serde(default)]
    pub attack_fire: i32,
    #[serde(default)]
    pub attack_earth: i32,
    #[serde(default)]
    pub attack_water: i32,
    #[serde(default)]
    pub attack_air: i32,
    /// Global damage bonus %, applied to every element.
    #[serde(default)]
    pub dmg: i32,
    #[serde(default)]
    pub dmg_fire: i32,
    #[serde(default)]
    pub dmg_earth: i32,
    #[serde(default)]
    pub dmg_water: i32,
    #[serde(default)]
    pub dmg_air: i32,
    #[serde(default)]
    pub res_fire: i32,
    #[serde(default)]
    pub res_earth: i32,
    #[serde(default)]
    pub res_water: i32,
    #[serde(default)]
    pub res_air: i32,

    // ─── gathering / crafting skill levels ────────────────────────────────────
    // The eight per-skill levels, `#[serde(default)]` like the combat block so
    // fixtures/mocks that omit them still deserialize. Keys match the live
    // CharacterSchema 1:1 (no serde renames). `SkillLevels: From<&CharacterView>`
    // reads these onto the model-state surface so gather/craft skill gates and
    // `skill_at_least` see them in both the plan and run passes.
    #[serde(default)]
    pub mining_level: u32,
    #[serde(default)]
    pub woodcutting_level: u32,
    #[serde(default)]
    pub fishing_level: u32,
    #[serde(default)]
    pub weaponcrafting_level: u32,
    #[serde(default)]
    pub gearcrafting_level: u32,
    #[serde(default)]
    pub jewelrycrafting_level: u32,
    #[serde(default)]
    pub cooking_level: u32,
    #[serde(default)]
    pub alchemy_level: u32,
}

/// The character's eight skill levels, lifted off `CharacterView` into a compact
/// block the model-state surface (`predicate_state`) exposes as `st.skills.*`.
/// The field names are the game's lowercase skill codes (the same values
/// `ResourceSchema.skill` and `RecipeCraft.skill` carry), so a gather/craft gate
/// can index `st.skills[resource.skill]` directly. Cheap to clone/compare, so
/// `state-eq` can treat `:skills` by identity like `:combat` — no `:sim` mutates
/// it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillLevels {
    pub mining: u32,
    pub woodcutting: u32,
    pub fishing: u32,
    pub weaponcrafting: u32,
    pub gearcrafting: u32,
    pub jewelrycrafting: u32,
    pub cooking: u32,
    pub alchemy: u32,
}

impl From<&CharacterView> for SkillLevels {
    fn from(v: &CharacterView) -> Self {
        Self {
            mining: v.mining_level,
            woodcutting: v.woodcutting_level,
            fishing: v.fishing_level,
            weaponcrafting: v.weaponcrafting_level,
            gearcrafting: v.gearcrafting_level,
            jewelrycrafting: v.jewelrycrafting_level,
            cooking: v.cooking_level,
            alchemy: v.alchemy_level,
        }
    }
}

impl CharacterView {
    pub fn inventory_count(&self) -> u32 {
        self.inventory.iter().map(|i| i.quantity).sum()
    }

    /// The occupied inventory slots as `(code, quantity)` pairs. The live API
    /// always returns every slot as an object; empty slots carry `code: ""` and
    /// `quantity: 0` rather than JSON null (`Code::is_empty` mirrors that
    /// sentinel), so those are filtered out. Borrows — no allocation — so
    /// per-frame UI code can format straight from it.
    pub fn occupied_items(&self) -> impl Iterator<Item = (&str, u32)> {
        self.inventory
            .iter()
            .filter(|i| !i.code.is_empty() && i.quantity > 0)
            .map(|i| (i.code.as_str(), i.quantity))
    }

    pub fn inventory_slots_used(&self) -> u32 {
        self.occupied_items().count() as u32
    }

    /// Occupied inventory as summed `(code, quantity)` pairs: duplicate codes
    /// spread across several slots are merged, empty slots skipped. This is the
    /// exact shape `predicate_state` builds `st.inventory` from, so the live
    /// `host.view` and the plan seed (`PlanSeed::from_view`) feed the surface the
    /// same map — the inventory-on-the-live-surface fix (`DYNAMIC_WORKFLOWS` §5.3).
    pub fn inventory_pairs(&self) -> Vec<(Code, u32)> {
        let mut by_code: std::collections::HashMap<Code, u32> = std::collections::HashMap::new();
        for (code, qty) in self.occupied_items() {
            *by_code.entry(Code::from(code)).or_default() += qty;
        }
        by_code.into_iter().collect()
    }
}

#[derive(Debug, Clone)]
pub struct FightResult {
    pub turns: u32,
    pub result: FightOutcome,
    pub xp: u32,
    pub gold: u32,
    pub drops: Vec<DropItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FightOutcome {
    Win,
    Lose,
}

#[derive(Debug, Clone)]
pub enum OutcomeKind {
    Move,
    Gather {
        items: Vec<DropItem>,
    },
    Fight(FightResult),
    Rest {
        hp_restored: u32,
    },
    Deposit {
        items: Vec<DropItem>,
    },
    Withdraw {
        items: Vec<DropItem>,
    },
    /// The generic outcome for every action beyond the handful above (craft,
    /// recycle, use, delete, equip/unequip, bank/give gold, npc buy/sell, grand
    /// exchange, tasks, map transition). The authoritative state change is on the
    /// refreshed `character` view; `items` carries whatever the response reported
    /// under `details` (e.g. crafted/consumed items) for logging, and is empty
    /// when it reported none. These actions aren't distinguished at the type
    /// level because nothing branches on them — the view is the source of truth;
    /// a per-action label, if ever needed, belongs as data on this variant, not
    /// as a fresh enum arm no code matches.
    Action {
        items: Vec<DropItem>,
    },
    /// The action was a benign no-op — e.g. a move to the tile the character is
    /// already on (HTTP 490). No state changed and no cooldown was incurred.
    NoOp,
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub cooldown: crate::cooldown::Cooldown,
    pub character: CharacterView,
    pub kind: OutcomeKind,
}
