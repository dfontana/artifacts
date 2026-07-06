//! One-place intent definitions.
//!
//! Each intent's wire format (request building) and outcome parsing live
//! together on one struct implementing `IntentWire`. The `Intent` enum wraps
//! those structs, and `Intent::wire` is the single dispatcher — the only
//! match over intents in the crate; `request`/`outcome` are one-line facades
//! through it. Adding an intent on the `core` side is: one struct +
//! `IntentWire` impl below (usually just `request` — `outcome` defaults to the
//! generic `OutcomeKind::Action`, so only intents with a distinct outcome shape
//! override it), one enum variant, one `wire()` arm, and (for that rare new
//! shape) one `OutcomeKind` variant in `step.rs` — all compile-checked: a
//! variant without a `wire()` arm is a non-exhaustive match, an arm whose struct
//! lacks the impl won't coerce to `&dyn IntentWire`, and neither can fail at
//! runtime.
//!
//! The run host-fn binding lives in `src/lua.rs` (`register_intent`) and is
//! compile-enforced the same way: `Intent` derives `strum::EnumIter`, so the
//! registration loop runs for every variant, and `register_intent`'s match is
//! exhaustive — a new variant with no binding is a non-exhaustive-match compile
//! error, never a silent nil-call in a workflow. The Fennel action goes in
//! `fennel/lib/actions.fnl`; see the "Adding things" checklist in
//! `docs/ARCHITECTURE.md`, and `plans/ALL_INTENTS.md` for the full
//! action-surface rollout.

use serde_json::json;

use crate::ident::{CharacterName, Code, ItemSlot, OrderId};
use crate::step::{DropItem, FightOutcome, FightResult, Method, OutcomeKind, Step};

/// The two per-intent halves of the request/response cycle. `request` builds
/// the HTTP call; `outcome` interprets the action-specific part of a 2xx
/// response. Everything shared (envelope, cooldown, character snapshot, error
/// classification) stays in `machine.rs`. Object-safe by design — the only
/// consumer is `Intent::wire`, which hands impls out as `&dyn IntentWire`.
pub(crate) trait IntentWire {
    fn request(&self) -> Step;
    /// Parse the action-specific part of a 2xx response. The default covers the
    /// large family of actions with no distinct outcome shape — it reports any
    /// `details.items` under the generic `OutcomeKind::Action` (see its doc). The
    /// handful with a real payload (move/gather/fight/rest/deposit/withdraw)
    /// override it.
    fn outcome(&self, payload: ActionPayload) -> OutcomeKind {
        OutcomeKind::Action {
            items: detail_items(payload),
        }
    }
}

/// The action-specific portion of a 2xx action response — the fields whose
/// presence and shape depend on which intent was sent. `machine.rs` flattens
/// this into its response envelope and hands it to the pending intent's
/// `outcome`. A future intent needing a new response field adds it here.
#[derive(Debug, Default, serde::Deserialize)]
pub(crate) struct ActionPayload {
    #[serde(default)]
    pub fight: Option<FightData>,
    #[serde(default)]
    pub details: Option<DetailsData>,
    /// The moved items on a bank item transaction (deposit/withdraw). The live
    /// bank endpoints report them at `data.items` (top level, alongside a
    /// `data.bank`), not nested under `details`.
    #[serde(default)]
    pub items: Vec<DropItem>,
}

/// `action/fight`'s payload. The live API tucks xp/gold/drops inside
/// `fight.characters[]`, not at the top of `fight`.
#[derive(Debug, Default, serde::Deserialize)]
pub(crate) struct FightData {
    pub turns: u32,
    pub result: String,
    /// Per-character fight outcome (xp/gold/drops live here on the live API).
    #[serde(default)]
    pub characters: Vec<FightCharacter>,
}

#[derive(Debug, Default, serde::Deserialize)]
pub(crate) struct FightCharacter {
    #[serde(default)]
    pub xp: u32,
    #[serde(default)]
    pub gold: u32,
    #[serde(default)]
    pub drops: Vec<DropItem>,
}

/// The `details` payload (DropSchema / hp_restored) non-fight actions return.
#[derive(Debug, Default, serde::Deserialize)]
pub(crate) struct DetailsData {
    #[serde(default)]
    pub items: Vec<DropItem>,
    #[serde(default)]
    pub hp_restored: Option<u32>,
}

/// A bodyless POST action (character name rides in the path, set by the driver).
fn post(path: &str) -> Step {
    Step::Request {
        method: Method::Post,
        path: path.into(),
        body: None,
    }
}

/// A POST action carrying a JSON body.
fn post_json(path: &str, body: serde_json::Value) -> Step {
    Step::Request {
        method: Method::Post,
        path: path.into(),
        body: Some(serde_json::to_vec(&body).unwrap()),
    }
}

/// The intents the workflow layer can reach — each variant wraps its wire
/// struct (request + outcome parsing in one place, below) and is backed by a
/// run host fn in `src/lua.rs`. Every character action the API exposes has a
/// variant here; each is exercised end to end (lua -> core) by a cross-cutting
/// test in `tests/all_intents.rs`, so none is untested wire-format that rots.
#[derive(Debug, Clone, strum::EnumIter)]
pub enum Intent {
    Move(Move),
    Gather(Gather),
    Fight(Fight),
    Rest(Rest),
    DepositItem(DepositItem),
    WithdrawItem(WithdrawItem),
    Craft(Craft),
    Recycle(Recycle),
    UseItem(UseItem),
    DeleteItem(DeleteItem),
    Equip(Equip),
    Unequip(Unequip),
    DepositGold(DepositGold),
    WithdrawGold(WithdrawGold),
    GiveGold(GiveGold),
    GiveItem(GiveItem),
    NpcBuy(NpcBuy),
    NpcSell(NpcSell),
    GeBuy(GeBuy),
    GeCancel(GeCancel),
    GeFill(GeFill),
    TaskNew(TaskNew),
    TaskComplete(TaskComplete),
    TaskCancel(TaskCancel),
    TaskExchange(TaskExchange),
    TaskTrade(TaskTrade),
    Transition(Transition),
}

impl Intent {
    /// The single dispatcher: resolve this intent's `IntentWire` impl. This
    /// is the ONLY match over intents in the crate — every other consumer
    /// goes through the facades below. The `&dyn` borrow is transient (just
    /// this call), so the enum itself stays small, `Clone`, and plain: the
    /// queue never holds a trait object.
    fn wire(&self) -> &dyn IntentWire {
        match self {
            Intent::Move(w) => w,
            Intent::Gather(w) => w,
            Intent::Fight(w) => w,
            Intent::Rest(w) => w,
            Intent::DepositItem(w) => w,
            Intent::WithdrawItem(w) => w,
            Intent::Craft(w) => w,
            Intent::Recycle(w) => w,
            Intent::UseItem(w) => w,
            Intent::DeleteItem(w) => w,
            Intent::Equip(w) => w,
            Intent::Unequip(w) => w,
            Intent::DepositGold(w) => w,
            Intent::WithdrawGold(w) => w,
            Intent::GiveGold(w) => w,
            Intent::GiveItem(w) => w,
            Intent::NpcBuy(w) => w,
            Intent::NpcSell(w) => w,
            Intent::GeBuy(w) => w,
            Intent::GeCancel(w) => w,
            Intent::GeFill(w) => w,
            Intent::TaskNew(w) => w,
            Intent::TaskComplete(w) => w,
            Intent::TaskCancel(w) => w,
            Intent::TaskExchange(w) => w,
            Intent::TaskTrade(w) => w,
            Intent::Transition(w) => w,
        }
    }

    /// Build the HTTP request for this intent.
    pub(crate) fn request(&self) -> Step {
        self.wire().request()
    }

    /// Parse the action-specific outcome of a 2xx response.
    pub(crate) fn outcome(&self, payload: ActionPayload) -> OutcomeKind {
        self.wire().outcome(payload)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Move {
    pub x: i32,
    pub y: i32,
}

impl IntentWire for Move {
    fn request(&self) -> Step {
        post_json("action/move", json!({"x": self.x, "y": self.y}))
    }

    fn outcome(&self, _payload: ActionPayload) -> OutcomeKind {
        OutcomeKind::Move
    }
}

#[derive(Debug, Clone, Default)]
pub struct Gather;

impl IntentWire for Gather {
    fn request(&self) -> Step {
        post("action/gathering")
    }

    fn outcome(&self, payload: ActionPayload) -> OutcomeKind {
        OutcomeKind::Gather {
            items: detail_items(payload),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Fight;

impl IntentWire for Fight {
    fn request(&self) -> Step {
        post("action/fight")
    }

    fn outcome(&self, payload: ActionPayload) -> OutcomeKind {
        let f = payload.fight.unwrap_or_default();
        // xp/gold/drops are reported per character; this client drives one.
        let per_char = f.characters.into_iter().next().unwrap_or_default();
        OutcomeKind::Fight(FightResult {
            turns: f.turns,
            result: if f.result == "win" {
                FightOutcome::Win
            } else {
                FightOutcome::Lose
            },
            xp: per_char.xp,
            gold: per_char.gold,
            drops: per_char.drops,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct Rest;

impl IntentWire for Rest {
    fn request(&self) -> Step {
        post("action/rest")
    }

    fn outcome(&self, payload: ActionPayload) -> OutcomeKind {
        OutcomeKind::Rest {
            hp_restored: payload.details.unwrap_or_default().hp_restored.unwrap_or(0),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct DepositItem {
    pub code: Code,
    pub quantity: u32,
}

impl IntentWire for DepositItem {
    fn request(&self) -> Step {
        // The bank item endpoints take a JSON array of {code, quantity}.
        post_json(
            "action/bank/deposit/item",
            json!([{"code": self.code, "quantity": self.quantity}]),
        )
    }

    fn outcome(&self, payload: ActionPayload) -> OutcomeKind {
        OutcomeKind::Deposit {
            items: payload.items,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct WithdrawItem {
    pub code: Code,
    pub quantity: u32,
}

impl IntentWire for WithdrawItem {
    fn request(&self) -> Step {
        // The bank item endpoints take a JSON array of {code, quantity}.
        post_json(
            "action/bank/withdraw/item",
            json!([{"code": self.code, "quantity": self.quantity}]),
        )
    }

    fn outcome(&self, payload: ActionPayload) -> OutcomeKind {
        OutcomeKind::Withdraw {
            items: payload.items,
        }
    }
}

/// The items an action reported under `data.details` (crafted/recycled/consumed
/// items). Shared by every intent whose outcome only needs that list — the
/// character view already carries the authoritative post-action state.
fn detail_items(payload: ActionPayload) -> Vec<DropItem> {
    payload.details.unwrap_or_default().items
}

// Every intent below has the generic `Action { items }` outcome, so it relies on
// `IntentWire::outcome`'s default and defines only `request`. `Default` is
// derived because `strum::EnumIter` constructs a placeholder of each variant.

// ─── skilling: crafting / recycling ──────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct Craft {
    pub code: Code,
    pub quantity: u32,
}

impl IntentWire for Craft {
    fn request(&self) -> Step {
        post_json(
            "action/crafting",
            json!({"code": self.code, "quantity": self.quantity}),
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct Recycle {
    pub code: Code,
    pub quantity: u32,
}

impl IntentWire for Recycle {
    fn request(&self) -> Step {
        post_json(
            "action/recycling",
            json!({"code": self.code, "quantity": self.quantity}),
        )
    }
}

// ─── inventory: use / delete ─────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct UseItem {
    pub code: Code,
    pub quantity: u32,
}

impl IntentWire for UseItem {
    fn request(&self) -> Step {
        post_json(
            "action/use",
            json!({"code": self.code, "quantity": self.quantity}),
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct DeleteItem {
    pub code: Code,
    pub quantity: u32,
}

impl IntentWire for DeleteItem {
    fn request(&self) -> Step {
        post_json(
            "action/delete",
            json!({"code": self.code, "quantity": self.quantity}),
        )
    }
}

// ─── equipment: equip / unequip ──────────────────────────────────────────────
// Both endpoints take a JSON array of entries (the API batches up to 20); this
// client drives one slot at a time, so the array holds a single element.

#[derive(Debug, Clone, Default)]
pub struct Equip {
    pub code: Code,
    pub slot: ItemSlot,
    pub quantity: u32,
}

impl IntentWire for Equip {
    fn request(&self) -> Step {
        post_json(
            "action/equip",
            json!([{"code": self.code, "slot": self.slot, "quantity": self.quantity}]),
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct Unequip {
    pub slot: ItemSlot,
    pub quantity: u32,
}

impl IntentWire for Unequip {
    fn request(&self) -> Step {
        post_json(
            "action/unequip",
            json!([{"slot": self.slot, "quantity": self.quantity}]),
        )
    }
}

// ─── bank gold ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct DepositGold {
    pub quantity: u32,
}

impl IntentWire for DepositGold {
    fn request(&self) -> Step {
        post_json(
            "action/bank/deposit/gold",
            json!({"quantity": self.quantity}),
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct WithdrawGold {
    pub quantity: u32,
}

impl IntentWire for WithdrawGold {
    fn request(&self) -> Step {
        post_json(
            "action/bank/withdraw/gold",
            json!({"quantity": self.quantity}),
        )
    }
}

// ─── give (to another character) ─────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct GiveGold {
    pub quantity: u32,
    pub character: CharacterName,
}

impl IntentWire for GiveGold {
    fn request(&self) -> Step {
        post_json(
            "action/give/gold",
            json!({"quantity": self.quantity, "character": self.character}),
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct GiveItem {
    pub code: Code,
    pub quantity: u32,
    pub character: CharacterName,
}

impl IntentWire for GiveItem {
    fn request(&self) -> Step {
        // The give/item endpoint takes an `items` array plus the recipient name.
        post_json(
            "action/give/item",
            json!({
                "items": [{"code": self.code, "quantity": self.quantity}],
                "character": self.character,
            }),
        )
    }
}

// ─── NPC merchant ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct NpcBuy {
    pub code: Code,
    pub quantity: u32,
}

impl IntentWire for NpcBuy {
    fn request(&self) -> Step {
        post_json(
            "action/npc/buy",
            json!({"code": self.code, "quantity": self.quantity}),
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct NpcSell {
    pub code: Code,
    pub quantity: u32,
}

impl IntentWire for NpcSell {
    fn request(&self) -> Step {
        post_json(
            "action/npc/sell",
            json!({"code": self.code, "quantity": self.quantity}),
        )
    }
}

// ─── Grand Exchange ──────────────────────────────────────────────────────────
// Orders are addressed by their server-assigned id (from the GE order listing
// endpoints), so the intents carry an opaque `OrderId` rather than a `Code`.

#[derive(Debug, Clone, Default)]
pub struct GeBuy {
    pub id: OrderId,
    pub quantity: u32,
}

impl IntentWire for GeBuy {
    fn request(&self) -> Step {
        post_json(
            "action/grandexchange/buy",
            json!({"id": self.id, "quantity": self.quantity}),
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct GeCancel {
    pub id: OrderId,
}

impl IntentWire for GeCancel {
    fn request(&self) -> Step {
        post_json("action/grandexchange/cancel", json!({"id": self.id}))
    }
}

#[derive(Debug, Clone, Default)]
pub struct GeFill {
    pub id: OrderId,
    pub quantity: u32,
}

impl IntentWire for GeFill {
    fn request(&self) -> Step {
        post_json(
            "action/grandexchange/fill",
            json!({"id": self.id, "quantity": self.quantity}),
        )
    }
}

// ─── tasks ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct TaskNew;

impl IntentWire for TaskNew {
    fn request(&self) -> Step {
        post("action/task/new")
    }
}

#[derive(Debug, Clone, Default)]
pub struct TaskComplete;

impl IntentWire for TaskComplete {
    fn request(&self) -> Step {
        post("action/task/complete")
    }
}

#[derive(Debug, Clone, Default)]
pub struct TaskCancel;

impl IntentWire for TaskCancel {
    fn request(&self) -> Step {
        post("action/task/cancel")
    }
}

#[derive(Debug, Clone, Default)]
pub struct TaskExchange;

impl IntentWire for TaskExchange {
    fn request(&self) -> Step {
        post("action/task/exchange")
    }
}

#[derive(Debug, Clone, Default)]
pub struct TaskTrade {
    pub code: Code,
    pub quantity: u32,
}

impl IntentWire for TaskTrade {
    fn request(&self) -> Step {
        post_json(
            "action/task/trade",
            json!({"code": self.code, "quantity": self.quantity}),
        )
    }
}

// ─── map transition ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct Transition;

impl IntentWire for Transition {
    fn request(&self) -> Step {
        post("action/transition")
    }
}
