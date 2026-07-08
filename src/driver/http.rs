//! Real network driver: reqwest + tokio.
//!
//! The `Driver` trait is synchronous, but reqwest is async. This driver owns a
//! dedicated current-thread tokio runtime and `block_on`s each request. That is
//! sound because the scheduler runs on a plain `std::thread` (not a tokio worker),
//! so blocking it never starves the async executor.
//!
//! Base URL `https://api.artifactsmmo.com`, bearer token from `ARTIFACTS_SECRET`.

use std::time::Instant;

use anyhow::{anyhow, Result};
use artifacts_core::combat::MonsterView;
use artifacts_core::ident::CharacterName;
use artifacts_core::map::{GameMap, MapTile, ResourceView};
use artifacts_core::npc::NpcItemView;
use artifacts_core::page::Page;
use artifacts_core::recipe::RecipeView;
use artifacts_core::step::{CharacterView, Method, Step};

use super::{Driver, DriverResult};

pub const DEFAULT_BASE_URL: &str = "https://api.artifactsmmo.com";

pub struct HttpDriver {
    client: reqwest::Client,
    runtime: tokio::runtime::Runtime,
    base_url: String,
    token: String,
    character: CharacterName,
}

impl HttpDriver {
    /// Construct with an explicit token. `character` is the name used to build
    /// `/my/{character}/action/...` URLs.
    pub fn new(character: impl Into<CharacterName>, token: impl Into<String>) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let client = reqwest::Client::builder()
            .user_agent("artifacts-rs/0.1")
            .build()?;
        Ok(Self {
            client,
            runtime,
            base_url: DEFAULT_BASE_URL.to_string(),
            token: token.into(),
            character: character.into(),
        })
    }

    /// Construct reading the token from the environment.
    pub fn from_env(character: impl Into<CharacterName>) -> Result<Self> {
        Self::new(character, token_from_env()?)
    }

    /// Override the base URL (useful for pointing at a local mock server in tests).
    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        self.base_url = base.into();
        self
    }

    fn url_for(&self, path: &str) -> String {
        build_url(&self.base_url, self.character.as_str(), path)
    }

    /// Perform one HTTP request and return (status, body bytes).
    fn do_request(
        &self,
        method: &Method,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> Result<(u16, Vec<u8>)> {
        let url = self.url_for(path);
        let client = &self.client;
        let token = &self.token;

        self.runtime.block_on(async move {
            let mut req = match method {
                Method::Get => client.get(&url),
                Method::Post => client.post(&url),
            };
            req = req.bearer_auth(token);
            if let Some(bytes) = body {
                req = req
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(bytes);
            } else if matches!(method, Method::Post) {
                // The API expects a JSON content-type even for empty-body actions.
                req = req.header(reqwest::header::CONTENT_TYPE, "application/json");
            }

            let resp = req.send().await.map_err(|e| anyhow!(e))?;
            let status = resp.status().as_u16();
            let bytes = resp.bytes().await.map_err(|e| anyhow!(e))?;
            Ok((status, bytes.to_vec()))
        })
    }

    /// Fetch the current character snapshot via `GET /characters/{name}`.
    pub fn fetch_character(&self) -> Result<CharacterView> {
        #[derive(serde::Deserialize)]
        struct Resp {
            data: CharacterView,
        }
        let (status, body) = self.do_request(
            &Method::Get,
            &format!("characters/{}", self.character),
            None,
        )?;
        if status != 200 {
            return Err(anyhow!(
                "fetch_character: status {status}: {}",
                String::from_utf8_lossy(&body)
            ));
        }
        let resp: Resp = serde_json::from_slice(&body)
            .map_err(|e| anyhow!("fetch_character parse error: {e}"))?;
        Ok(resp.data)
    }

    /// Fetch every item of a paginated list endpoint (100 per page). `base_path`
    /// may already carry query params (`maps?layer=overworld`) or not
    /// (`monsters`); `what` names the call in error messages.
    fn fetch_paginated<T: serde::de::DeserializeOwned>(
        &self,
        base_path: &str,
        what: &str,
    ) -> Result<Vec<T>> {
        let sep = if base_path.contains('?') { '&' } else { '?' };
        let mut items: Vec<T> = Vec::new();
        let mut page = 1u32;
        loop {
            let path = format!("{base_path}{sep}size=100&page={page}");
            let (status, body) = self.do_request(&Method::Get, &path, None)?;
            if status != 200 {
                return Err(anyhow!(
                    "{what}: status {status}: {}",
                    String::from_utf8_lossy(&body)
                ));
            }
            let parsed: Page<T> =
                serde_json::from_slice(&body).map_err(|e| anyhow!("{what} parse error: {e}"))?;
            let last_page = parsed.is_last();
            items.extend(parsed.data);
            if last_page {
                break;
            }
            if page > 1000 {
                return Err(anyhow!(
                    "{what}: pagination cap (1000 pages) exceeded — is_last never true \
                     (possible size==0 or malformed response)"
                ));
            }
            page += 1;
        }
        Ok(items)
    }

    /// Fetch all overworld map tiles (paginated) — the raw, disk-cacheable form
    /// (`data::load_overworld_map` is the TTL-cached loader built on this).
    pub fn fetch_overworld_tiles(&self) -> Result<Vec<MapTile>> {
        self.fetch_paginated("maps?layer=overworld", "fetch_overworld_tiles")
    }

    /// Fetch all overworld maps (paginated) into a `GameMap` for A* pathfinding.
    pub fn fetch_overworld_map(&self) -> Result<GameMap> {
        Ok(GameMap::from_tiles(self.fetch_overworld_tiles()?))
    }

    /// Fetch all monster reference data (paginated) via `GET /monsters`.
    pub fn fetch_all_monsters(&self) -> Result<Vec<MonsterView>> {
        self.fetch_paginated("monsters", "fetch_all_monsters")
    }

    /// Fetch all resource reference data (paginated) via `GET /resources`.
    pub fn fetch_all_resources(&self) -> Result<Vec<ResourceView>> {
        self.fetch_paginated("resources", "fetch_all_resources")
    }

    /// Fetch all item reference data (paginated) via `GET /items`, reduced to
    /// `RecipeView` (code + optional craft recipe). The disk-cacheable form
    /// behind `data::RecipeData::load`; static like `/monsters`, so cold
    /// launches shouldn't re-page it.
    pub fn fetch_all_items(&self) -> Result<Vec<RecipeView>> {
        self.fetch_paginated("items", "fetch_all_items")
    }

    /// Fetch the whole NPC merchant catalog (paginated) via `GET /npcs/items` —
    /// one `NpcItemView` per (item, merchant) listing. The disk-cacheable form
    /// behind `data::NpcItemData::load`; static prices (unlike the live GE order
    /// book), so it caches like the other reference datasets.
    pub fn fetch_all_npc_items(&self) -> Result<Vec<NpcItemView>> {
        self.fetch_paginated("npcs/items", "fetch_all_npc_items")
    }
}

/// Read the bearer token from the environment: `ARTIFACTS_SECRETS`
fn token_from_env() -> Result<String> {
    std::env::var("ARTIFACTS_SECRET").map_err(|_| anyhow!("ARTIFACTS_SECRET is not set"))
}

/// Build the full request URL.
///
/// Action paths (`action/...`) are character-scoped: `{base}/my/{character}/{path}`.
/// Everything else is treated as a top-level resource path: `{base}/{path}`.
pub fn build_url(base: &str, character: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    if path.starts_with("action/") {
        format!("{base}/my/{character}/{path}")
    } else {
        format!("{base}/{path}")
    }
}

impl Driver for HttpDriver {
    fn current_time(&self) -> Instant {
        Instant::now()
    }

    fn execute(&mut self, step: Step) -> DriverResult {
        match step {
            Step::Sleep { until, .. } => {
                let now = Instant::now();
                if until > now {
                    let dur = until - now;
                    // `tokio::time::sleep` reads the runtime clock when constructed,
                    // so it MUST be created inside the runtime context (within the
                    // async block), not passed as a pre-built future to block_on.
                    self.runtime.block_on(async move {
                        tokio::time::sleep(dur).await;
                    });
                }
                DriverResult::Slept
            }
            Step::Request { method, path, body } => match self.do_request(&method, &path, body) {
                Ok((status, body)) => DriverResult::Response { status, body },
                Err(message) => DriverResult::Error {
                    message: message.to_string(),
                },
            },
            Step::Done => DriverResult::Done,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_url_levels() {
        assert_eq!(
            build_url("https://api.artifactsmmo.com", "kael", "action/move"),
            "https://api.artifactsmmo.com/my/kael/action/move"
        );
        assert_eq!(
            build_url("https://api.artifactsmmo.com", "kael", "characters/kael"),
            "https://api.artifactsmmo.com/characters/kael"
        );
        assert_eq!(
            build_url(
                "https://api.artifactsmmo.com",
                "kael",
                "maps?layer=overworld"
            ),
            "https://api.artifactsmmo.com/maps?layer=overworld"
        );
        assert_eq!(
            build_url("https://api.artifactsmmo.com/", "kael", "/action/gathering"),
            "https://api.artifactsmmo.com/my/kael/action/gathering"
        );
    }
}
