//! Real network driver: reqwest + tokio.
//!
//! The `Driver` trait is synchronous, but reqwest is async. This driver owns a
//! dedicated current-thread tokio runtime and `block_on`s each request. That is
//! sound because the scheduler runs on a plain `std::thread` (not a tokio worker),
//! so blocking it never starves the async executor.
//!
//! Base URL `https://api.artifactsmmo.com`, bearer token from `ARTIFACTS_TOKEN`.

use std::time::Instant;

use artifacts_core::map::{GameMap, MapTile, MapsPage};
use artifacts_core::step::{CharacterView, Method, Step};

use super::{Driver, DriverResult};

pub const DEFAULT_BASE_URL: &str = "https://api.artifactsmmo.com";

pub struct HttpDriver {
    client: reqwest::Client,
    runtime: tokio::runtime::Runtime,
    base_url: String,
    token: String,
    character: String,
}

impl HttpDriver {
    /// Construct with an explicit token. `character` is the name used to build
    /// `/my/{character}/action/...` URLs.
    pub fn new(character: impl Into<String>, token: impl Into<String>) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("failed to build tokio runtime: {e}"))?;
        let client = reqwest::Client::builder()
            .user_agent("artifacts-rs/0.1")
            .build()
            .map_err(|e| format!("failed to build reqwest client: {e}"))?;
        Ok(Self {
            client,
            runtime,
            base_url: DEFAULT_BASE_URL.to_string(),
            token: token.into(),
            character: character.into(),
        })
    }

    /// Construct reading the token from the environment. Checks `ARTIFACTS_TOKEN`
    /// first, then `ARTIFACTS_SECRET` (the name commonly used in `.envrc` setups).
    pub fn from_env(character: impl Into<String>) -> Result<Self, String> {
        let token = std::env::var("ARTIFACTS_TOKEN")
            .or_else(|_| std::env::var("ARTIFACTS_SECRET"))
            .map_err(|_| "neither ARTIFACTS_TOKEN nor ARTIFACTS_SECRET is set".to_string())?;
        Self::new(character, token)
    }

    /// Override the base URL (useful for pointing at a local mock server in tests).
    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        self.base_url = base.into();
        self
    }

    fn url_for(&self, path: &str) -> String {
        build_url(&self.base_url, &self.character, path)
    }

    /// Perform one HTTP request and return (status, body bytes).
    fn do_request(
        &self,
        method: &Method,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> Result<(u16, Vec<u8>), String> {
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

            let resp = req.send().await.map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
            Ok::<_, String>((status, bytes.to_vec()))
        })
    }

    // ─── bootstrap helpers (not part of the Driver trait) ─────────────────────

    /// Fetch the current character snapshot via `GET /characters/{name}`.
    pub fn fetch_character(&self) -> Result<CharacterView, String> {
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
            return Err(format!(
                "fetch_character: status {status}: {}",
                String::from_utf8_lossy(&body)
            ));
        }
        let resp: Resp = serde_json::from_slice(&body)
            .map_err(|e| format!("fetch_character parse error: {e}"))?;
        Ok(resp.data)
    }

    /// Fetch all overworld maps (paginated) into a `GameMap` for A* pathfinding.
    pub fn fetch_overworld_map(&self) -> Result<GameMap, String> {
        let mut tiles: Vec<MapTile> = Vec::new();
        let mut page = 1u32;
        loop {
            let path = format!("maps?layer=overworld&size=100&page={page}");
            let (status, body) = self.do_request(&Method::Get, &path, None)?;
            if status != 200 {
                return Err(format!(
                    "fetch_overworld_map: status {status}: {}",
                    String::from_utf8_lossy(&body)
                ));
            }
            let parsed: MapsPage = serde_json::from_slice(&body)
                .map_err(|e| format!("fetch_overworld_map parse error: {e}"))?;
            let last_page = parsed.page * parsed.size >= parsed.total;
            tiles.extend(parsed.data);
            if last_page || page > 1000 {
                break;
            }
            page += 1;
        }
        Ok(GameMap::from_tiles(tiles))
    }
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
                Err(message) => DriverResult::Error { message },
            },
            Step::FetchData { path } => match self.do_request(&Method::Get, &path, None) {
                Ok((_, body)) => DriverResult::Data { body },
                Err(message) => DriverResult::Error { message },
            },
            Step::Done => DriverResult::Done,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_paths_are_character_scoped() {
        assert_eq!(
            build_url("https://api.artifactsmmo.com", "kael", "action/move"),
            "https://api.artifactsmmo.com/my/kael/action/move"
        );
        assert_eq!(
            build_url(
                "https://api.artifactsmmo.com",
                "kael",
                "action/bank/deposit/item"
            ),
            "https://api.artifactsmmo.com/my/kael/action/bank/deposit/item"
        );
    }

    #[test]
    fn data_paths_are_top_level() {
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
    }

    #[test]
    fn trailing_and_leading_slashes_are_normalised() {
        assert_eq!(
            build_url("https://api.artifactsmmo.com/", "kael", "/action/gathering"),
            "https://api.artifactsmmo.com/my/kael/action/gathering"
        );
    }

    /// Live smoke test against the real API. Hits the network, so it is ignored
    /// by default. Run with a real token and character:
    ///   ARTIFACTS_TOKEN=... ARTIFACTS_CHARACTER=kael \
    ///     cargo test -p artifacts-driver --features http -- --ignored live_fetch
    #[test]
    #[ignore = "hits the live network; requires ARTIFACTS_TOKEN + ARTIFACTS_CHARACTER"]
    fn live_fetch() {
        let character = std::env::var("ARTIFACTS_CHARACTER")
            .expect("set ARTIFACTS_CHARACTER for the live test");
        let driver = HttpDriver::from_env(&character).expect("build driver");

        let view = driver.fetch_character().expect("fetch character");
        assert_eq!(view.name, character);
        eprintln!(
            "character at ({}, {}), hp {}/{}",
            view.x, view.y, view.hp, view.max_hp
        );

        let map = driver.fetch_overworld_map().expect("fetch map");
        assert!(map.tile_count() > 0, "expected a non-empty overworld map");
        eprintln!("loaded {} overworld tiles", map.tile_count());
    }
}
