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
    FetchData {
        path: String,
    },
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepReason {
    Cooldown,
    RateLimit,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Slot {
    Weapon,
    Shield,
    Helmet,
    BodyArmor,
    LegArmor,
    Boots,
    Ring1,
    Ring2,
    Amulet,
    Artifact1,
    Artifact2,
    Artifact3,
    Utility1,
    Utility2,
}

#[derive(Debug, Clone)]
pub enum Intent {
    Move {
        x: i32,
        y: i32,
    },
    Gather,
    Fight,
    Rest,
    Craft {
        code: Code,
        quantity: u32,
    },
    Equip {
        code: Code,
        slot: Slot,
        quantity: u32,
    },
    Unequip {
        slot: Slot,
        quantity: u32,
    },
    DepositItem {
        code: Code,
        quantity: u32,
    },
    WithdrawItem {
        code: Code,
        quantity: u32,
    },
    DepositAll,
    UseItem {
        code: Code,
        quantity: u32,
    },
    Recycle {
        code: Code,
        quantity: u32,
    },
}

/// An inventory slot (the character's `inventory` array). Always carries a slot
/// index on the live API; empty slots have `code: ""` and `quantity: 0`.
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
    pub inventory: Vec<Option<InventoryItem>>,

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
}

impl CharacterView {
    pub fn inventory_count(&self) -> u32 {
        self.inventory
            .iter()
            .filter_map(|s| s.as_ref())
            .map(|i| i.quantity)
            .sum()
    }

    pub fn inventory_slots_used(&self) -> u32 {
        // The live API always returns every slot as an object; empty slots carry
        // `code: ""` and `quantity: 0` rather than JSON null. Count only occupied
        // slots.
        self.inventory
            .iter()
            .filter_map(|s| s.as_ref())
            .filter(|i| !i.code.is_empty() && i.quantity > 0)
            // Code::is_empty mirrors the live API's empty-slot sentinel (code: "").
            .count() as u32
    }

    pub fn inventory_full(&self) -> bool {
        // `inventory_max_items` is the total *quantity* cap (e.g. 100), NOT a slot
        // count — the inventory has a fixed, smaller number of slots (20 on live).
        // Fullness is therefore measured against summed quantity, not slots used.
        self.inventory_count() >= self.inventory_max_items
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
    Craft {
        items: Vec<DropItem>,
    },
    Deposit {
        items: Vec<DropItem>,
    },
    Withdraw {
        items: Vec<DropItem>,
    },
    Equip,
    Unequip,
    UseItem,
    Recycle {
        items: Vec<DropItem>,
    },
    DepositAll {
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
