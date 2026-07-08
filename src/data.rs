//! Reference data (monsters, overworld map) with a TTL disk cache.
//!
//! `/monsters` and `/maps` are static-ish data that change rarely, so we fetch
//! them once and cache to disk **outside version control** with a ~1-day TTL,
//! refetching only when the cache is missing or stale. We deliberately do
//! **not** vendor snapshots into the repo: they would go stale silently. The
//! plan and run passes are both allowed to populate these caches from the
//! network; the in-memory data is then handed to the Lua host so lookups
//! (`host.monster_stats`, `host.find_tile`, A*) are pure.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use artifacts_core::combat::MonsterView;
use artifacts_core::ident::Code;
use artifacts_core::map::{GameMap, MapTile, ResourceView};
use artifacts_core::npc::NpcItemView;
use artifacts_core::recipe::{RecipeCraft, RecipeView};

use crate::driver::http::HttpDriver;

/// One day. Reference data doesn't move faster than game patches.
const TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Cache schema version. Bump whenever a cached type's serialised shape changes
/// (a field added/removed/renamed on `MapTile`, `MonsterView`, or any future
/// cached type). [`read_fresh_cache`] compares the on-disk `version` against
/// this and discards a mismatched cache for a refetch, so a stale schema
/// written by an older code version is never silently served with
/// `#[serde(default)]` holes for the whole TTL. A pre-versioning cache file
/// (raw payload, no envelope) fails to deserialize as [`CacheEnvelope`] and is
/// discarded too — so introducing versioning (or bumping it) cleanly
/// invalidates every affected cache exactly once.
/// Bumped to 2 in M3: `ResourceView` gained strict `skill`/`drops` fields, so a
/// v1 `resources.json` (which lacks them) must be retired rather than fail the
/// now-non-defaulted parse.
const CACHE_SCHEMA_VERSION: u32 = 2;

/// Versioned wrapper around a cached payload. The `version` marker lets
/// [`read_fresh_cache`] detect a cache written by an older code version and
/// discard it for a refetch rather than serving a degraded parse propped up by
/// `#[serde(default)]` holes.
#[derive(serde::Serialize, serde::Deserialize)]
struct CacheEnvelope<T> {
    version: u32,
    data: T,
}

/// All monsters, keyed by code (e.g. "chicken"), ready for `host.monster_stats`.
#[derive(Debug, Default, Clone)]
pub struct MonsterData {
    by_code: HashMap<Code, MonsterView>,
}

impl MonsterData {
    pub fn get(&self, code: &Code) -> Option<&MonsterView> {
        self.by_code.get(code)
    }

    pub fn len(&self) -> usize {
        self.by_code.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_code.is_empty()
    }

    /// Build directly from a list of monster views (no network) — for the network
    /// load path, tests, and callers that already hold the reference data.
    pub fn from_vec(monsters: Vec<MonsterView>) -> Self {
        Self {
            by_code: monsters.into_iter().map(|m| (m.code.clone(), m)).collect(),
        }
    }

    /// Load monster data, preferring a fresh on-disk cache and falling back to a
    /// network fetch (which then refreshes the cache).
    pub fn load(driver: &HttpDriver) -> Result<Self> {
        let monsters = load_cached("monsters.json", || driver.fetch_all_monsters())
            .context("fetching /monsters")?;
        Ok(Self::from_vec(monsters))
    }
}

/// All gatherable resources, keyed by code (e.g. "copper_rocks"), ready for
/// `host.active_resource`'s level lookup.
#[derive(Debug, Default, Clone)]
pub struct ResourceData {
    by_code: HashMap<Code, ResourceView>,
}

impl ResourceData {
    pub fn get(&self, code: &Code) -> Option<&ResourceView> {
        self.by_code.get(code)
    }

    pub fn len(&self) -> usize {
        self.by_code.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_code.is_empty()
    }

    /// Build directly from a list of resource views (no network) — for the
    /// network load path, tests, and callers that already hold the data.
    pub fn from_vec(resources: Vec<ResourceView>) -> Self {
        Self {
            by_code: resources.into_iter().map(|r| (r.code.clone(), r)).collect(),
        }
    }

    /// Load resource data, preferring a fresh on-disk cache and falling back to
    /// a network fetch (which then refreshes the cache).
    pub fn load(driver: &HttpDriver) -> Result<Self> {
        let resources = load_cached("resources.json", || driver.fetch_all_resources())
            .context("fetching /resources")?;
        Ok(Self::from_vec(resources))
    }
}

/// All craftable recipes, keyed by the crafted item's code (e.g. "copper_dagger"),
/// ready for `host.recipe`. Built from `/items` by dropping every non-craftable
/// item — the map only holds items that actually have a `craft` block.
#[derive(Debug, Default, Clone)]
pub struct RecipeData {
    by_output: HashMap<Code, RecipeCraft>,
}

impl RecipeData {
    /// The recipe producing `code`, or `None` if that item isn't craftable.
    pub fn get(&self, code: &Code) -> Option<&RecipeCraft> {
        self.by_output.get(code)
    }

    pub fn len(&self) -> usize {
        self.by_output.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_output.is_empty()
    }

    /// Build directly from `/items` rows (no network) — the network load path,
    /// tests, and callers that already hold the reference data. Non-craftable
    /// items (`craft: None`) are dropped.
    pub fn from_items(items: Vec<RecipeView>) -> Self {
        Self {
            by_output: items
                .into_iter()
                .filter_map(|i| i.craft.map(|c| (i.code, c)))
                .collect(),
        }
    }

    /// Load recipe data, preferring a fresh on-disk cache and falling back to a
    /// paginated `/items` fetch (which then refreshes the cache).
    pub fn load(driver: &HttpDriver) -> Result<Self> {
        let items =
            load_cached("items.json", || driver.fetch_all_items()).context("fetching /items")?;
        Ok(Self::from_items(items))
    }
}

/// The NPC merchant catalog, keyed by ITEM code → every merchant listing for
/// that item (one item can be sold/bought by several NPCs, so the value is a
/// `Vec`). Static prices, cached as `npc_items.json` on the same TTL as the
/// other reference data (`DYNAMIC_WORKFLOWS` §5.5 — cacheable precisely because
/// NPC prices are fixed, unlike the live Grand Exchange order book). The
/// host-side item-sources index that consumes this lands in M5; this is the
/// data-only groundwork.
#[derive(Debug, Default, Clone)]
pub struct NpcItemData {
    by_item: HashMap<Code, Vec<NpcItemView>>,
}

impl NpcItemData {
    /// Every merchant listing for `code`, or `None` if no NPC trades it.
    pub fn get(&self, code: &Code) -> Option<&[NpcItemView]> {
        self.by_item.get(code).map(Vec::as_slice)
    }

    pub fn len(&self) -> usize {
        self.by_item.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_item.is_empty()
    }

    /// Build directly from `/npcs/items` rows (no network) — the network load
    /// path, tests, and callers that already hold the data. Rows are grouped by
    /// item code, preserving fetch order within each item.
    pub fn from_vec(items: Vec<NpcItemView>) -> Self {
        let mut by_item: HashMap<Code, Vec<NpcItemView>> = HashMap::new();
        for item in items {
            by_item.entry(item.code.clone()).or_default().push(item);
        }
        Self { by_item }
    }

    /// Load NPC-item data, preferring a fresh on-disk cache and falling back to a
    /// paginated `/npcs/items` fetch (which then refreshes the cache).
    pub fn load(driver: &HttpDriver) -> Result<Self> {
        let items = load_cached("npc_items.json", || driver.fetch_all_npc_items())
            .context("fetching /npcs/items")?;
        Ok(Self::from_vec(items))
    }
}

/// Load the overworld map, preferring a fresh on-disk tile cache and falling
/// back to a paginated network fetch (which then refreshes the cache). The map
/// is as static as the monster data, so cold launches shouldn't re-page /maps.
pub fn load_overworld_map(driver: &HttpDriver) -> Result<GameMap> {
    let tiles: Vec<MapTile> = load_cached("overworld_map.json", || driver.fetch_overworld_tiles())
        .context("fetching /maps")?;
    Ok(GameMap::from_tiles(tiles))
}

/// The shared TTL cache-or-fetch path: read `file` if younger than [`TTL`],
/// otherwise run `fetch` and refresh the cache (write failures are non-fatal —
/// a stale or unwritable cache just means refetching next launch).
fn load_cached<T, F>(file: &str, fetch: F) -> Result<Vec<T>>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
    F: FnOnce() -> Result<Vec<T>>,
{
    let path = cache_path(file);

    if let Some(items) = path.as_ref().and_then(read_fresh_cache) {
        return Ok(items);
    }

    let items = fetch().map_err(|e| anyhow::anyhow!("{e}"))?;

    if let Some(p) = &path {
        write_cache(p, &items); // best effort
    }

    Ok(items)
}

/// `$XDG_CACHE_HOME/artifacts-mmo/{file}`, falling back to `$HOME/.cache/...`.
/// Returns `None` if no home/cache dir can be determined, in which case we just
/// skip caching and always fetch.
fn cache_path(file: &str) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("artifacts-mmo").join(file))
}

/// Read and parse the cache iff it exists and is younger than the TTL.
fn read_fresh_cache<T: serde::de::DeserializeOwned>(path: &PathBuf) -> Option<Vec<T>> {
    let meta = std::fs::metadata(path).ok()?;
    let age = meta.modified().ok()?.elapsed().unwrap_or(Duration::MAX);
    if age > TTL {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    // Reject a stale-schema (or pre-versioning) cache: a parse failure here
    // (old format had no envelope) or a `version` mismatch both fall through to
    // `None` -> refetch, so a degraded `#[serde(default)]` parse is never
    // served across the TTL.
    let envelope: CacheEnvelope<Vec<T>> = serde_json::from_slice(&bytes).ok()?;
    if envelope.version != CACHE_SCHEMA_VERSION {
        return None;
    }
    Some(envelope.data)
}

/// Write `items` to the cache **atomically**: stage the bytes to a
/// uniquely-named temp file in the **same directory** as `path` (hence the same
/// filesystem, so `rename` is a POSIX-atomic replace rather than a
/// cross-device `EXDEV`), then `rename` it over `path`. A concurrent reader thus
/// observes either the previous complete file or the new complete file — never
/// a half-written one — so a racing `read_fresh_cache` can't hit a partial
/// `serde_json` failure. Write failures stay non-fatal (best effort): a failed
/// write just means refetching next launch.
fn write_cache<T: serde::Serialize>(path: &PathBuf, items: &[T]) {
    let _ = (|| -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let envelope = CacheEnvelope {
            version: CACHE_SCHEMA_VERSION,
            data: items,
        };
        let bytes = serde_json::to_vec(&envelope).map_err(std::io::Error::other)?;
        let tmp = temp_cache_path(path);
        // Stage to the sibling temp file, then atomically rename over the
        // target. On any failure, best-effort remove the orphaned temp file so
        // the cache directory doesn't accumulate stale `.tmp` files.
        std::fs::write(&tmp, &bytes)
            .and_then(|()| std::fs::rename(&tmp, path))
            .inspect_err(|_| {
                let _ = std::fs::remove_file(&tmp);
            })
    })();
}

/// A uniquely-named sibling of `path` within its parent directory (same
/// filesystem), used as the staging file for an atomic cache write. The
/// `pid`+`nanos` suffix keeps two racing writers from clobbering one temp
/// file; a stale leftover from a crashed writer is simply overwritten by the
/// next writer and renamed away, so it never blocks a read.
fn temp_cache_path(path: &Path) -> PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "cache".to_string());
    path.with_file_name(format!("{name}.tmp.{pid}.{nanos}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A uniquely-named scratch path under `$TMPDIR`/`/tmp`, removed on drop so
    /// parallel test runs don't collide and don't leak files.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(label: &str) -> Self {
            let pid = std::process::id();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            Self(std::env::temp_dir().join(format!("artifacts-cache-{label}-{pid}-{nanos}.json")))
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn round_trip_versioned_cache() {
        let s = Scratch::new("roundtrip");
        let items = vec![1u32, 2, 3];
        write_cache(&s.0, &items);
        let back: Option<Vec<u32>> = read_fresh_cache(&s.0);
        assert_eq!(back, Some(items));
    }

    #[test]
    fn stale_version_discarded_for_refetch() {
        // A cache written with a *different* schema version must be discarded
        // (-> refetch) rather than parsed with serde(default) holes.
        let s = Scratch::new("stalever");
        let envelope = CacheEnvelope {
            version: CACHE_SCHEMA_VERSION + 1,
            data: vec![1u32, 2, 3],
        };
        std::fs::write(&s.0, serde_json::to_vec(&envelope).unwrap()).unwrap();
        let back: Option<Vec<u32>> = read_fresh_cache(&s.0);
        assert_eq!(back, None, "stale-version cache must be discarded");
    }

    #[test]
    fn old_unversioned_cache_discarded_for_refetch() {
        // A pre-versioning cache file (raw Vec, no envelope) fails to parse as
        // CacheEnvelope -> discarded -> refetch. This is the one-time
        // migration: introducing versioning invalidates every old cache once.
        let s = Scratch::new("oldfmt");
        let raw: Vec<u32> = vec![1, 2, 3];
        std::fs::write(&s.0, serde_json::to_vec(&raw).unwrap()).unwrap();
        let back: Option<Vec<u32>> = read_fresh_cache(&s.0);
        assert_eq!(back, None, "un-versioned cache must be discarded");
    }
}
