use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::{Driver, DriverResult};
use artifacts_core::step::{Method, Step};

/// A scripted (status, body) pair to return for a given request path.
#[derive(Debug, Clone)]
pub struct CannedResponse {
    pub path_contains: String,
    pub status: u16,
    pub body: Vec<u8>,
}

/// One request the driver was asked to execute, captured for after-the-fact
/// assertions. `body` is the raw POST payload (`None` for a bodyless action),
/// so a test can prove the exact wire format an intent produced, not just that
/// *some* request went out.
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    pub method: Method,
    pub path: String,
    pub body: Option<Vec<u8>>,
}

/// Shared handle to the driver's request log. `MockDriver::request_log` hands
/// one out before the driver is moved onto the scheduler thread, so a test can
/// read what was sent once the run finishes.
pub type RequestLog = Arc<Mutex<Vec<RecordedRequest>>>;

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
    /// Every request executed, in order (shared so tests can read it after the
    /// driver has moved onto the scheduler thread).
    requests: RequestLog,
}

impl MockDriver {
    pub fn new() -> Self {
        Self {
            now: Instant::now(),
            responses: VecDeque::new(),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// A handle to this driver's request log. Grab it before moving the driver
    /// onto the scheduler thread, then inspect the recorded requests after the
    /// run to assert on the exact path/body each intent sent.
    pub fn request_log(&self) -> RequestLog {
        self.requests.clone()
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
                    self.now = until;
                }
                DriverResult::Slept
            }
            Step::Request { method, path, body } => {
                // `path` is cloned because it's read again below to match a
                // canned response; `body` is not needed after this, so move it.
                self.requests
                    .lock()
                    .expect("mock request log poisoned")
                    .push(RecordedRequest {
                        method,
                        path: path.clone(),
                        body,
                    });
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
