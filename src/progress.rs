//! Workflow progress observability: the node-id log the `run` pass appends to
//! via `host.progress`. Defined here — below the Lua bridge, not in the TUI —
//! so `lua.rs` never depends on a presentation layer; the TUI re-exports these
//! (`crate::tui::{NodeId, ProgressLog}`) for its consumers.

use std::sync::{Arc, Mutex};

/// A workflow AST node's identity. Stamped by the Fennel `number-nodes` walk in
/// pre-order (visit order), it is the join key between the skeleton, the plan's
/// loop counts, and the run's progress log — see `plans/TUI.md` §3.1.
pub type NodeId = u64;

/// The append-only ordered log the `run` pass appends a `NodeId` to on entry to
/// every node (via `host.progress`). Named `ProgressLog` — not `Progress` — to
/// avoid confusion with `core::machine::Progress` (§9). A `Vec`, not a single
/// slot, so microsecond-apart fires (a when-skip immediately followed by its
/// sibling) are never lost between UI frames.
pub type ProgressLog = Arc<Mutex<Vec<NodeId>>>;

/// Build a fresh, empty progress log.
pub fn new_progress_log() -> ProgressLog {
    Arc::new(Mutex::new(Vec::new()))
}
