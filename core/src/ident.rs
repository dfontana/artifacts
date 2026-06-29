//! Opaque game identities.
//!
//! The API threads around short strings that are *identities*, not free text: a
//! tile's content category ("monster", "bank"), the code that names a
//! monster/resource/item ("chicken", "copper_rocks"), a character's name, a map
//! layer. They are only ever compared and looked up — never concatenated or
//! sliced. Wrapping each in a distinct newtype makes that intent checkable: you
//! cannot accidentally pass an item code where a character name is expected, and
//! `grep` traces every origin.
//!
//! New identities should be added via the `identity!` macro below rather than as
//! bare `String`s — that is the pattern this codebase scales on.

/// Define an opaque string-identity newtype. `derive_more` keeps the wrappers
/// boilerplate-free; `#[from(forward)]` lets both `String` and `&str` construct
/// one; `#[serde(transparent)]` makes it (de)serialize as the bare string.
macro_rules! identity {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug,
            Clone,
            Default,
            PartialEq,
            Eq,
            Hash,
            derive_more::Display,
            derive_more::From,
            derive_more::AsRef,
            serde::Serialize,
            serde::Deserialize,
        )]
        #[serde(transparent)]
        #[from(forward)]
        #[as_ref(str)]
        pub struct $name(String);

        impl $name {
            /// Borrow the underlying identity as a string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// True if the identity is the empty string (e.g. an empty inventory
            /// slot the live API returns as `code: ""`).
            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }
        }
    };
}

identity! {
    /// A tile's content category: "monster", "bank", "resource", "workshop", …
    ContentType
}

identity! {
    /// A game-object code identifying a monster, resource, item, or map content —
    /// e.g. "chicken", "copper_rocks", "copper_ore". The shared identity used by
    /// map content, the `/monsters` dataset, and inventory/action item codes.
    Code
}

identity! {
    /// A character's name, e.g. "nillinbot". Identifies the character whose
    /// `/my/{name}/...` endpoints an action targets.
    CharacterName
}

identity! {
    /// A map layer, e.g. "overworld" or an interior. Tiles on different layers
    /// share an (x, y) grid, so the layer disambiguates them.
    Layer
}
