//! One-place intent definitions.
//!
//! Each intent's wire format (request building) and outcome parsing live
//! together on one struct implementing `IntentWire`. The `Intent` enum wraps
//! those structs, and `Intent::wire` is the single dispatcher — the only
//! match over intents in the crate; `request`/`outcome` are one-line facades
//! through it. Adding an intent on the `core` side is: one struct +
//! `IntentWire` impl below, one enum variant, one `wire()` arm, and (if it
//! produces a new outcome shape) one `OutcomeKind` variant in `step.rs` —
//! all compile-checked: a variant without a `wire()` arm is a non-exhaustive
//! match, an arm whose struct lacks the impl won't coerce to
//! `&dyn IntentWire`, and neither can fail at runtime.
//!
//! The host-fn binding lives in `src/lua.rs` (`register_run_host_fns`) and
//! the Fennel action in `fennel/lib/actions.fnl`; see plans/INTENTS.md for
//! the full checklist.

use serde_json::json;

use crate::ident::Code;
use crate::step::{DropItem, FightOutcome, FightResult, Method, OutcomeKind, Step};

/// The two per-intent halves of the request/response cycle. `request` builds
/// the HTTP call; `outcome` interprets the action-specific part of a 2xx
/// response. Everything shared (envelope, cooldown, character snapshot, error
/// classification) stays in `machine.rs`. Object-safe by design — the only
/// consumer is `Intent::wire`, which hands impls out as `&dyn IntentWire`.
pub(crate) trait IntentWire {
    fn request(&self) -> Step;
    fn outcome(&self, payload: ActionPayload) -> OutcomeKind;
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

/// The intents the workflow layer can actually reach today — each variant
/// wraps its wire struct (request + outcome parsing in one place, below)
/// and is backed by a run host fn in `src/lua.rs`. Further live actions
/// (craft, equip, use, recycle, …) get a variant when their host fn lands,
/// not before — untested wire-format code only rots.
#[derive(Debug, Clone)]
pub enum Intent {
    Move(Move),
    Gather(Gather),
    Fight(Fight),
    Rest(Rest),
    DepositItem(DepositItem),
    WithdrawItem(WithdrawItem),
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

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
pub struct Gather;

impl IntentWire for Gather {
    fn request(&self) -> Step {
        post("action/gathering")
    }

    fn outcome(&self, payload: ActionPayload) -> OutcomeKind {
        OutcomeKind::Gather {
            items: payload.details.unwrap_or_default().items,
        }
    }
}

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
pub struct DepositItem {
    pub code: Code,
    pub quantity: u32,
}

impl IntentWire for DepositItem {
    fn request(&self) -> Step {
        post_json(
            "action/bank/deposit/item",
            json!({"code": self.code, "quantity": self.quantity}),
        )
    }

    fn outcome(&self, payload: ActionPayload) -> OutcomeKind {
        OutcomeKind::Deposit {
            items: payload.details.unwrap_or_default().items,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WithdrawItem {
    pub code: Code,
    pub quantity: u32,
}

impl IntentWire for WithdrawItem {
    fn request(&self) -> Step {
        post_json(
            "action/bank/withdraw/item",
            json!({"code": self.code, "quantity": self.quantity}),
        )
    }

    fn outcome(&self, payload: ActionPayload) -> OutcomeKind {
        OutcomeKind::Withdraw {
            items: payload.details.unwrap_or_default().items,
        }
    }
}
