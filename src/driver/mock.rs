use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::{Driver, DriverResult};
use artifacts_core::step::Step;

/// A scripted (status, body) pair to return for a given request path.
#[derive(Debug, Clone)]
pub struct CannedResponse {
    pub path_contains: String,
    pub status: u16,
    pub body: Vec<u8>,
}

impl CannedResponse {
    pub fn new(path_contains: impl Into<String>, status: u16, body: Vec<u8>) -> Self {
        Self {
            path_contains: path_contains.into(),
            status,
            body,
        }
    }
}

/// MockDriver: fake clock + scripted responses, no network I/O.
pub struct MockDriver {
    /// Simulated current time (starts at `base`; advanced by sleeps).
    pub now: Instant,
    /// Queue of canned responses, consumed in order.
    pub responses: VecDeque<CannedResponse>,
    /// Total simulated time elapsed.
    pub elapsed: Duration,
}

impl MockDriver {
    pub fn new() -> Self {
        Self {
            now: Instant::now(),
            responses: VecDeque::new(),
            elapsed: Duration::ZERO,
        }
    }

    pub fn push_response(&mut self, r: CannedResponse) {
        self.responses.push_back(r);
    }

    pub fn push_responses(&mut self, rs: impl IntoIterator<Item = CannedResponse>) {
        for r in rs {
            self.responses.push_back(r);
        }
    }
}

impl Default for MockDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl Driver for MockDriver {
    fn current_time(&self) -> Instant {
        self.now
    }

    fn execute(&mut self, step: Step) -> DriverResult {
        match step {
            Step::Sleep { until, .. } => {
                if until > self.now {
                    let delta = until.duration_since(self.now);
                    self.now = until;
                    self.elapsed += delta;
                }
                DriverResult::Slept
            }
            Step::Request { path, .. } => {
                // Find matching canned response (first match by path substring).
                let idx = self
                    .responses
                    .iter()
                    .position(|r| path.contains(&r.path_contains));
                if let Some(i) = idx {
                    let r = self.responses.remove(i).unwrap();
                    DriverResult::Response {
                        status: r.status,
                        body: r.body,
                    }
                } else {
                    // Default: 200 with empty cooldown.
                    DriverResult::Response {
                        status: 200,
                        body: default_ok_body(),
                    }
                }
            }
            Step::FetchData { .. } => DriverResult::Data {
                body: b"{}".to_vec(),
            },
            Step::Done => DriverResult::Done,
        }
    }
}

fn default_ok_body() -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "data": {
            "cooldown": {
                "total_seconds": 0.0,
                "remaining_seconds": 0.0,
                "started_at": "2024-01-01T00:00:00Z",
                "expiration": "2024-01-01T00:00:00Z",
                "reason": "none"
            },
            "character": {
                "name": "kael",
                "x": 0,
                "y": 0,
                "hp": 100,
                "max_hp": 100,
                "level": 1,
                "inventory_max_items": 10,
                "inventory": []
            }
        }
    }))
    .unwrap()
}
