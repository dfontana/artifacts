//! Shared workflow data passed to planning and execution.
//!
//! Reference datasets are immutable snapshots loaded during bootstrap; bank is
//! live account state loaded alongside the character. Keeping them in one value
//! prevents CLI, campaign, and TUI paths from silently seeding different worlds
//! and keeps new datasets from expanding every positional API.

use std::sync::Arc;

use artifacts_core::map::GameMap;

use crate::data::{BankData, MonsterData, NpcItemData, RecipeData, ResourceData};

#[derive(Debug, Default, Clone)]
pub struct ExecutionContext {
    pub map: Option<Arc<GameMap>>,
    pub monsters: Option<Arc<MonsterData>>,
    pub resources: Option<Arc<ResourceData>>,
    pub recipes: Option<Arc<RecipeData>>,
    pub npc_items: Option<Arc<NpcItemData>>,
    pub bank: Option<Arc<BankData>>,
}

impl ExecutionContext {
    /// Clone the immutable references and replace the live bank snapshot.
    pub fn with_bank(&self, bank: Arc<BankData>) -> Self {
        Self {
            bank: Some(bank),
            ..self.clone()
        }
    }
}
