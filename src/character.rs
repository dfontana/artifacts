use artifacts_core::{
    error::GameError,
    ident::{CharacterName, Code},
    step::{Intent, Outcome},
    wire,
};
use tokio::sync::{mpsc, oneshot};

use crate::{scheduler::Submit, view::SharedView};

/// Handle the workflow layer uses to drive one character.
/// All methods block the calling (script) thread; the async scheduler does the I/O.
#[derive(Clone)]
pub struct Character {
    to_scheduler: mpsc::Sender<Submit>,
    pub view: SharedView,
}

impl Character {
    pub fn new(to_scheduler: mpsc::Sender<Submit>, view: SharedView) -> Self {
        Self { to_scheduler, view }
    }

    fn submit(&self, intent: Intent) -> Result<Outcome, GameError> {
        let (tx, rx) = oneshot::channel();
        self.to_scheduler
            .blocking_send(Submit { intent, reply: tx })
            .map_err(|_| GameError::Internal("scheduler channel closed".into()))?;
        rx.blocking_recv()
            .map_err(|_| GameError::Internal("scheduler reply channel closed".into()))?
    }

    pub fn move_to(&self, x: i32, y: i32) -> Result<Outcome, GameError> {
        self.submit(Intent::Move(wire::Move { x, y }))
    }

    pub fn gather(&self) -> Result<Outcome, GameError> {
        self.submit(Intent::Gather(wire::Gather))
    }

    pub fn fight(&self) -> Result<Outcome, GameError> {
        self.submit(Intent::Fight(wire::Fight))
    }

    pub fn rest(&self) -> Result<Outcome, GameError> {
        self.submit(Intent::Rest(wire::Rest))
    }

    pub fn deposit_item(&self, code: impl Into<Code>, quantity: u32) -> Result<Outcome, GameError> {
        self.submit(Intent::DepositItem(wire::DepositItem {
            code: code.into(),
            quantity,
        }))
    }

    pub fn withdraw_item(
        &self,
        code: impl Into<Code>,
        quantity: u32,
    ) -> Result<Outcome, GameError> {
        self.submit(Intent::WithdrawItem(wire::WithdrawItem {
            code: code.into(),
            quantity,
        }))
    }

    pub fn craft(&self, code: impl Into<Code>, quantity: u32) -> Result<Outcome, GameError> {
        self.submit(Intent::Craft(wire::Craft {
            code: code.into(),
            quantity,
        }))
    }

    pub fn recycle(&self, code: impl Into<Code>, quantity: u32) -> Result<Outcome, GameError> {
        self.submit(Intent::Recycle(wire::Recycle {
            code: code.into(),
            quantity,
        }))
    }

    pub fn use_item(&self, code: impl Into<Code>, quantity: u32) -> Result<Outcome, GameError> {
        self.submit(Intent::UseItem(wire::UseItem {
            code: code.into(),
            quantity,
        }))
    }

    pub fn delete_item(&self, code: impl Into<Code>, quantity: u32) -> Result<Outcome, GameError> {
        self.submit(Intent::DeleteItem(wire::DeleteItem {
            code: code.into(),
            quantity,
        }))
    }

    /// Equip one item into `slot` (an `ItemSlot` name, e.g. `"weapon"`).
    /// `quantity` is only meaningful for stackable utility slots; 1 elsewhere.
    pub fn equip(
        &self,
        code: impl Into<Code>,
        slot: impl Into<String>,
        quantity: u32,
    ) -> Result<Outcome, GameError> {
        self.submit(Intent::Equip(wire::Equip {
            code: code.into(),
            slot: slot.into(),
            quantity,
        }))
    }

    pub fn unequip(&self, slot: impl Into<String>, quantity: u32) -> Result<Outcome, GameError> {
        self.submit(Intent::Unequip(wire::Unequip {
            slot: slot.into(),
            quantity,
        }))
    }

    pub fn deposit_gold(&self, quantity: u32) -> Result<Outcome, GameError> {
        self.submit(Intent::DepositGold(wire::DepositGold { quantity }))
    }

    pub fn withdraw_gold(&self, quantity: u32) -> Result<Outcome, GameError> {
        self.submit(Intent::WithdrawGold(wire::WithdrawGold { quantity }))
    }

    pub fn give_gold(
        &self,
        quantity: u32,
        character: impl Into<CharacterName>,
    ) -> Result<Outcome, GameError> {
        self.submit(Intent::GiveGold(wire::GiveGold {
            quantity,
            character: character.into(),
        }))
    }

    pub fn give_item(
        &self,
        code: impl Into<Code>,
        quantity: u32,
        character: impl Into<CharacterName>,
    ) -> Result<Outcome, GameError> {
        self.submit(Intent::GiveItem(wire::GiveItem {
            code: code.into(),
            quantity,
            character: character.into(),
        }))
    }

    pub fn npc_buy(&self, code: impl Into<Code>, quantity: u32) -> Result<Outcome, GameError> {
        self.submit(Intent::NpcBuy(wire::NpcBuy {
            code: code.into(),
            quantity,
        }))
    }

    pub fn npc_sell(&self, code: impl Into<Code>, quantity: u32) -> Result<Outcome, GameError> {
        self.submit(Intent::NpcSell(wire::NpcSell {
            code: code.into(),
            quantity,
        }))
    }

    pub fn ge_buy(&self, id: impl Into<String>, quantity: u32) -> Result<Outcome, GameError> {
        self.submit(Intent::GeBuy(wire::GeBuy {
            id: id.into(),
            quantity,
        }))
    }

    pub fn ge_cancel(&self, id: impl Into<String>) -> Result<Outcome, GameError> {
        self.submit(Intent::GeCancel(wire::GeCancel { id: id.into() }))
    }

    pub fn ge_fill(&self, id: impl Into<String>, quantity: u32) -> Result<Outcome, GameError> {
        self.submit(Intent::GeFill(wire::GeFill {
            id: id.into(),
            quantity,
        }))
    }

    pub fn task_new(&self) -> Result<Outcome, GameError> {
        self.submit(Intent::TaskNew(wire::TaskNew))
    }

    pub fn task_complete(&self) -> Result<Outcome, GameError> {
        self.submit(Intent::TaskComplete(wire::TaskComplete))
    }

    pub fn task_cancel(&self) -> Result<Outcome, GameError> {
        self.submit(Intent::TaskCancel(wire::TaskCancel))
    }

    pub fn task_exchange(&self) -> Result<Outcome, GameError> {
        self.submit(Intent::TaskExchange(wire::TaskExchange))
    }

    pub fn task_trade(&self, code: impl Into<Code>, quantity: u32) -> Result<Outcome, GameError> {
        self.submit(Intent::TaskTrade(wire::TaskTrade {
            code: code.into(),
            quantity,
        }))
    }

    pub fn transition(&self) -> Result<Outcome, GameError> {
        self.submit(Intent::Transition(wire::Transition))
    }

    /// Deposit every occupied inventory slot, one `DepositItem` per slot.
    /// `occupied_items` (the canonical slot filter) skips the live API's empty
    /// sentinels, so no zero-quantity deposits are submitted.
    pub fn deposit_all(&self) -> Result<Vec<Outcome>, GameError> {
        let view = self.view.get();
        let items: Vec<(Code, u32)> = view
            .occupied_items()
            .map(|(code, qty)| (Code::from(code), qty))
            .collect();

        let mut outcomes = Vec::new();
        for (code, qty) in items {
            let outcome = self.submit(Intent::DepositItem(wire::DepositItem {
                code,
                quantity: qty,
            }))?;
            outcomes.push(outcome);
        }
        Ok(outcomes)
    }
}
