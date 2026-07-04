use artifacts_core::{
    error::GameError,
    ident::Code,
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
