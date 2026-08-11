//! Bank holdings — one row per item code from `GET /my/bank/items` (the API's
//! `SimpleItemSchema`: `code` + `quantity`).
//!
//! Unlike the other reference-data view types (`npc.rs`'s `NpcItemView`,
//! `recipe.rs`'s `RecipeView`, `MonsterView`, …), this is **not** static
//! reference data — it is **live account state**: the bank's contents are
//! account-wide and change every time *any* of the account's characters
//! deposits or withdraws. A TTL disk cache would go silently wrong the moment
//! a sibling character (or this one, in a previous run) moved an item, so bank
//! holdings are deliberately fetched fresh on every invocation — the same
//! reasoning class as the Grand Exchange order book (a live player
//! marketplace, also never TTL-cached), and the mirror image of why NPC
//! prices/monster stats/recipes/the map *are* cacheable: those are fixed by
//! the game and only change on a patch.

use crate::ident::Code;

/// One bank holding: an item `code` and the account's current `quantity` of it.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct BankItemView {
    pub code: Code,
    pub quantity: u32,
}
