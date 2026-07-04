use std::sync::{Arc, RwLock};

use artifacts_core::step::CharacterView;

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
