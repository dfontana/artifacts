//! NPC merchant catalog reference data — one row per (item, merchant) pairing
//! from `GET /npcs/items` (the API's `NPCItemSchema`).
//!
//! This is **static reference data**, the same class as monsters/resources/
//! recipes: NPC prices are fixed by the game, changing only on a patch, so it is
//! fetched once and served from a TTL disk cache (`data::NpcItemData`). That is
//! the crucial difference from the Grand Exchange order book, which is a live
//! player marketplace and therefore deliberately *not* cached (GE trades take an
//! author-supplied price hint instead). `buy_price`/`sell_price` are nullable per
//! the spec — an NPC that only buys a given item has no `buy_price`, and vice
//! versa.

use crate::ident::Code;

/// One merchant listing for an item: the `code` is the ITEM's code, `npc` the
/// merchant selling/buying it, and `currency` is `"gold"` or (when it isn't
/// gold) the item code used to pay. Either price may be absent.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct NpcItemView {
    pub code: Code,
    pub npc: Code,
    pub currency: Code,
    pub buy_price: Option<u32>,
    pub sell_price: Option<u32>,
}
