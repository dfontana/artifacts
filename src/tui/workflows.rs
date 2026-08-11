//! Scan `fennel/workflows/*.fnl` into a selectable list. The TUI reads workflow
//! sources from disk (relative to the working directory) so authoring a new
//! `.fnl` file makes it appear without a rebuild — matching how the CLI takes a
//! workflow path.

use std::path::Path;

use anyhow::{anyhow, Result};

use crate::lua::{setup_lua, LuaSetupOptions};
use crate::workflow::{self, WorkflowInfo};

/// The default location the TUI scans, relative to the working directory.
pub const DEFAULT_DIR: &str = "fennel/workflows";

#[derive(Debug, Clone)]
pub struct Workflow {
    /// Display name — the file stem (e.g. `farm`).
    pub name: String,
    /// The `.fnl` source, read at scan time.
    pub src: String,
    /// The workflow's declared `:doc` + `:params`, marshalled at scan time. A
    /// workflow whose `schema()` errors (e.g. a stray bare-AST file) stores the
    /// error string instead — the list still renders, the row just can't run.
    pub info: Result<WorkflowInfo, String>,
}

/// Scan a directory for `*.fnl` workflows, sorted by name. A missing directory
/// yields an empty list (the panel shows a hint) rather than an error. One bare
/// Lua state is built for the whole scan and each workflow's schema marshalled
/// through it — no build call, so it needs no character/map/reference data.
pub fn scan(dir: impl AsRef<Path>) -> Result<Vec<Workflow>> {
    let dir = dir.as_ref();
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let lua = setup_lua(LuaSetupOptions::default()).map_err(|e| anyhow!("setup_lua: {e}"))?;
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("fnl") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let src = std::fs::read_to_string(&path)?;
        let info = workflow::schema(&lua, &src, name).map_err(|e| e.to_string());
        out.push(Workflow {
            name: name.to_string(),
            src,
            info,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}
