//! Opaque game identities.
//!
//! The API threads around a handful of short strings that are *identities*, not
//! free text: a tile's content category ("monster", "bank"), and the code that
//! names a monster/resource/item ("chicken", "copper_rocks"). They are only ever
//! compared and looked up — never concatenated or sliced. Wrapping them in
//! newtypes makes that intent checkable: you cannot accidentally pass a content
//! type where a code is expected, and `grep` traces every origin. `derive_more`
//! keeps the wrappers boilerplate-free; `#[from(forward)]` lets both `String`
//! and `&str` construct one.

use derive_more::{AsRef, Display, From};

/// A tile's content category: "monster", "bank", "resource", "workshop", …
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Display, From, AsRef, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
#[from(forward)]
#[as_ref(str)]
pub struct ContentType(String);

/// A game-object code identifying a monster, resource, item, or map content —
/// e.g. "chicken", "copper_rocks", "copper_ore". The shared identity used by
/// both map content and the `/monsters` dataset.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Display, From, AsRef, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
#[from(forward)]
#[as_ref(str)]
pub struct Code(String);
