//! Monster reference data with a TTL disk cache.
//!
//! `/monsters` is static-ish data that changes rarely, so we fetch it once and
//! cache it to disk **outside version control** with a ~1-day TTL, refetching
//! only when the cache is missing or stale. We deliberately do **not** vendor a
//! snapshot into the repo: it would go stale silently. The plan and run passes
//! are both allowed to populate this cache from the network; the in-memory
//! `MonsterData` is then handed to the Lua host so `host.monster_stats` is a
//! pure lookup.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use artifacts_core::combat::MonsterView;

use crate::driver::http::HttpDriver;

/// One day. Monster stats don't move faster than game patches.
const TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// All monsters, keyed by code (e.g. "chicken"), ready for `host.monster_stats`.
#[derive(Debug, Default, Clone)]
pub struct MonsterData {
    by_code: HashMap<String, MonsterView>,
}

impl MonsterData {
    pub fn get(&self, code: &str) -> Option<&MonsterView> {
        self.by_code.get(code)
    }

    pub fn len(&self) -> usize {
        self.by_code.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_code.is_empty()
    }

    fn from_vec(monsters: Vec<MonsterView>) -> Self {
        Self {
            by_code: monsters.into_iter().map(|m| (m.code.clone(), m)).collect(),
        }
    }

    /// Load monster data, preferring a fresh on-disk cache and falling back to a
    /// network fetch (which then refreshes the cache). A stale or unreadable
    /// cache is simply refetched; cache write failures are non-fatal.
    pub fn load(driver: &HttpDriver) -> Result<Self> {
        let path = cache_path();

        if let Some(monsters) = path.as_ref().and_then(read_fresh_cache) {
            return Ok(Self::from_vec(monsters));
        }

        let monsters = driver
            .fetch_all_monsters()
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("fetching /monsters")?;

        if let Some(p) = &path {
            write_cache(p, &monsters); // best effort
        }

        Ok(Self::from_vec(monsters))
    }
}

/// `$XDG_CACHE_HOME/artifacts-mmo/monsters.json`, falling back to
/// `$HOME/.cache/...`. Returns `None` if no home/cache dir can be determined, in
/// which case we just skip caching and always fetch.
fn cache_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("artifacts-mmo").join("monsters.json"))
}

/// Read and parse the cache iff it exists and is younger than the TTL.
fn read_fresh_cache(path: &PathBuf) -> Option<Vec<MonsterView>> {
    let meta = std::fs::metadata(path).ok()?;
    let age = meta.modified().ok()?.elapsed().unwrap_or(Duration::MAX);
    if age > TTL {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_cache(path: &PathBuf, monsters: &[MonsterView]) {
    let _ = (|| -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let bytes = serde_json::to_vec(monsters).map_err(std::io::Error::other)?;
        std::fs::write(path, bytes)
    })();
}
