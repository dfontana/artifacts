use std::sync::{Arc, RwLock};

use artifacts_core::{
    error::GameError,
    ident::Code,
    step::{CharacterView, Intent, Outcome},
    wire,
};
use tokio::sync::{mpsc, oneshot};

use crate::scheduler::Submit;

/// A cheap, cloneable, synchronously readable snapshot of character state.
/// Refreshed after every Outcome. All predicates read from this.
///
/// The inner `Arc` is swapped on update, so `get()` is a pointer bump — the TUI
/// reads this every frame and must not deep-clone the inventory each time.
#[derive(Clone, Debug)]
pub struct SharedView(Arc<RwLock<Arc<CharacterView>>>);

impl SharedView {
    pub fn new(initial: CharacterView) -> Self {
        Self(Arc::new(RwLock::new(Arc::new(initial))))
    }

    pub fn get(&self) -> Arc<CharacterView> {
        Arc::clone(&self.0.read().expect("view lock poisoned"))
    }

    pub fn update(&self, view: CharacterView) {
        *self.0.write().expect("view lock poisoned") = Arc::new(view);
    }
}

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

    pub fn submit(&self, intent: Intent) -> Result<Outcome, GameError> {
        let (tx, rx) = oneshot::channel();
        self.to_scheduler
            .blocking_send(Submit { intent, reply: tx })
            .map_err(|_| GameError::Internal("scheduler channel closed".into()))?;
        rx.blocking_recv()
            .map_err(|_| GameError::Internal("scheduler reply channel closed".into()))?
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
