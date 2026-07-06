//! Scan `fennel/workflows/*.fnl` into a selectable list. The TUI reads workflow
//! sources from disk (relative to the working directory) so authoring a new
//! `.fnl` file makes it appear without a rebuild — matching how the CLI takes a
//! workflow path.

use std::path::Path;

use anyhow::Result;

/// The default location the TUI scans, relative to the working directory.
pub const DEFAULT_DIR: &str = "fennel/workflows";

#[derive(Debug, Clone)]
pub struct Workflow {
    /// Display name — the file stem (e.g. `farm-copper`).
    pub name: String,
    /// The `.fnl` source, read at scan time.
    pub src: String,
}

/// Scan a directory for `*.fnl` workflows, sorted by name. A missing directory
/// yields an empty list (the panel shows a hint) rather than an error.
pub fn scan(dir: impl AsRef<Path>) -> Result<Vec<Workflow>> {
    let dir = dir.as_ref();
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
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
        out.push(Workflow {
            name: name.to_string(),
            src,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}
