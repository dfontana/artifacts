//! The shared drop-rate model — one struct for every `DropRateSchema` the API
//! returns (monster kills AND resource gathers). A drop is a `1/rate` chance per
//! success of `min_quantity..=max_quantity` of `code`. Hoisted out of `combat.rs`
//! (where it used to live as `MonsterDrop`) so `map::ResourceView` can reuse the
//! exact same shape instead of declaring a parallel copy — the payloads are
//! byte-identical on the wire.

use crate::ident::Code;

/// A single drop entry: a `1/rate` chance per success of
/// `min_quantity..=max_quantity` of `code`. Shared by monster kills
/// (`combat::MonsterView`) and resource gathers (`map::ResourceView`).
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct DropRate {
    pub code: Code,
    pub rate: u32,
    pub min_quantity: u32,
    pub max_quantity: u32,
}
