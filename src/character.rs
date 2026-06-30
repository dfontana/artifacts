use artifacts_core::{
    error::GameError,
    ident::Code,
    step::{Intent, Outcome, Slot},
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
        self.submit(Intent::Move { x, y })
    }

    pub fn gather(&self) -> Result<Outcome, GameError> {
        self.submit(Intent::Gather)
    }

    pub fn fight(&self) -> Result<Outcome, GameError> {
        self.submit(Intent::Fight)
    }

    pub fn rest(&self) -> Result<Outcome, GameError> {
        self.submit(Intent::Rest)
    }

    pub fn craft(&self, code: impl Into<Code>, quantity: u32) -> Result<Outcome, GameError> {
        self.submit(Intent::Craft {
            code: code.into(),
            quantity,
        })
    }

    pub fn equip(
        &self,
        code: impl Into<Code>,
        slot: Slot,
        quantity: u32,
    ) -> Result<Outcome, GameError> {
        self.submit(Intent::Equip {
            code: code.into(),
            slot,
            quantity,
        })
    }

    pub fn deposit_item(&self, code: impl Into<Code>, quantity: u32) -> Result<Outcome, GameError> {
        self.submit(Intent::DepositItem {
            code: code.into(),
            quantity,
        })
    }

    pub fn deposit_all(&self) -> Result<Vec<Outcome>, GameError> {
        let view = self.view.get();
        let items: Vec<_> = view
            .inventory
            .iter()
            .filter_map(|s| s.as_ref())
            .map(|i| (i.code.clone(), i.quantity))
            .collect();

        let mut outcomes = Vec::new();
        for (code, qty) in items {
            let outcome = self.submit(Intent::DepositItem {
                code,
                quantity: qty,
            })?;
            outcomes.push(outcome);
        }
        Ok(outcomes)
    }
}
