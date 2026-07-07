//! Crafting recipe reference data — the `craft` block of the `/items` endpoint.
//!
//! This is static reference data, the same class as monsters and the overworld
//! map: it changes only on a game patch, so it is fetched once and served from a
//! TTL disk cache (`data::RecipeData`). It exists so the `craft` plan-pass `:sim`
//! can *consume the recipe inputs* and add the crafted output, instead of the
//! earlier output-only guess. Recycling deliberately does **not** get a table
//! here: the game returns a *random* subset of the craft materials on recycle
//! (only the returned *count* is deterministic), so there is no faithful static
//! salvage table to model — recycle stays an honest net-consume.

use crate::ident::Code;

/// One input material consumed by a craft, from the recipe's `items` list
/// (the API's `SimpleItemSchema`: an item code and a per-craft quantity).
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct RecipeInput {
    pub code: Code,
    pub quantity: u32,
}

/// A craftable item's recipe — the API's `CraftSchema`. Every field is optional
/// on the wire (`CraftSchema.required` is empty), so `skill`/`level` are
/// tolerated as absent and `quantity` defaults to 1 (one crafted item per
/// execution, the overwhelmingly common case). `items` are the inputs the sim
/// consumes; `quantity` is the crafted output count one execution yields.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct RecipeCraft {
    /// Skill required to craft (e.g. "weaponcrafting"). Informational for the
    /// sim today; the backwards-planner spike will use it.
    #[serde(default)]
    pub skill: Option<String>,
    /// Skill level required. Informational for the sim today.
    #[serde(default)]
    pub level: Option<u32>,
    /// Inputs consumed per craft execution.
    #[serde(default)]
    pub items: Vec<RecipeInput>,
    /// Crafted items produced per execution (default 1).
    #[serde(default = "one")]
    pub quantity: u32,
}

fn one() -> u32 {
    1
}

/// One item row from `/items`, reduced to what recipes care about: the item's
/// own `code` and its optional `craft` recipe. Every other item field
/// (effects, subtype, tradeable, …) is ignored by serde. Non-craftable items
/// deserialize with `craft: None` and are dropped when building [`RecipeData`].
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct RecipeView {
    pub code: Code,
    #[serde(default)]
    pub craft: Option<RecipeCraft>,
}
