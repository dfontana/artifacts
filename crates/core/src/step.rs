use std::time::Instant;

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

#[derive(Debug, Clone)]
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
    Move { x: i32, y: i32 },
    Gather,
    Fight,
    Rest,
    Craft { code: String, quantity: u32 },
    Equip { code: String, slot: Slot, quantity: u32 },
    Unequip { slot: Slot, quantity: u32 },
    DepositItem { code: String, quantity: u32 },
    WithdrawItem { code: String, quantity: u32 },
    DepositAll,
    UseItem { code: String, quantity: u32 },
    Recycle { code: String, quantity: u32 },
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct InventoryItem {
    pub slot: u32,
    pub code: String,
    pub quantity: u32,
}

/// A point-in-time snapshot of character data returned from the server.
#[derive(Debug, Clone, serde::Deserialize, Default)]
pub struct CharacterView {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub hp: u32,
    pub max_hp: u32,
    pub level: u32,
    pub inventory_max_items: u32,
    #[serde(default)]
    pub inventory: Vec<Option<InventoryItem>>,
    pub skin: Option<String>,
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
        // slots so `inventory_full` is correct against both mock and live data.
        self.inventory
            .iter()
            .filter_map(|s| s.as_ref())
            .filter(|i| !i.code.is_empty() && i.quantity > 0)
            .count() as u32
    }

    pub fn inventory_full(&self) -> bool {
        self.inventory_slots_used() >= self.inventory_max_items
    }

    pub fn hp_below(&self, threshold: u32) -> bool {
        self.hp < threshold
    }

    pub fn hp_percent(&self) -> f64 {
        if self.max_hp == 0 {
            return 1.0;
        }
        self.hp as f64 / self.max_hp as f64
    }
}

#[derive(Debug, Clone)]
pub struct FightResult {
    pub turns: u32,
    pub result: FightOutcome,
    pub xp: u32,
    pub gold: u32,
    pub drops: Vec<InventoryItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FightOutcome {
    Win,
    Lose,
}

#[derive(Debug, Clone)]
pub enum OutcomeKind {
    Move,
    Gather { items: Vec<InventoryItem> },
    Fight(FightResult),
    Rest { hp_restored: u32 },
    Craft { items: Vec<InventoryItem> },
    Deposit { items: Vec<InventoryItem> },
    Withdraw { items: Vec<InventoryItem> },
    Equip,
    Unequip,
    UseItem,
    Recycle { items: Vec<InventoryItem> },
    DepositAll { items: Vec<InventoryItem> },
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub cooldown: crate::cooldown::Cooldown,
    pub character: CharacterView,
    pub kind: OutcomeKind,
}
