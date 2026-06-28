use std::sync::{Arc, RwLock};

use artifacts_core::step::CharacterView;

/// A cheap, cloneable, synchronously readable snapshot of character state.
/// Refreshed after every Outcome. All predicates read from this.
#[derive(Clone, Debug)]
pub struct SharedView(Arc<RwLock<CharacterView>>);

impl SharedView {
    pub fn new(initial: CharacterView) -> Self {
        Self(Arc::new(RwLock::new(initial)))
    }

    pub fn get(&self) -> CharacterView {
        self.0.read().expect("view lock poisoned").clone()
    }

    pub fn update(&self, view: CharacterView) {
        *self.0.write().expect("view lock poisoned") = view;
    }
}
