pub mod http;
pub mod mock;

use artifacts_core::step::Step;
use std::time::Instant;

/// The I/O boundary trait. Implementations provide clock + network.
/// The driver owns the authoritative clock — the scheduler must call
/// `current_time()` rather than `Instant::now()` so that MockDriver's
/// fake clock and HttpDriver's real clock both work correctly.
pub trait Driver: Send + 'static {
    /// Execute one `Step` and return the raw HTTP result (status + body).
    /// For `Sleep` steps, the driver waits (real or fake) before returning.
    /// For `Done`, returns immediately.
    fn execute(&mut self, step: Step) -> DriverResult;

    /// Current time according to this driver's clock.
    /// MockDriver returns its fake monotonic clock; HttpDriver returns Instant::now().
    fn current_time(&self) -> Instant;
}

#[derive(Debug)]
pub enum DriverResult {
    /// HTTP response arrived.
    Response { status: u16, body: Vec<u8> },
    /// Slept until the requested instant.
    Slept,
    /// Data fetch result.
    Data { body: Vec<u8> },
    /// Transport-level failure (connection refused, timeout, DNS, TLS).
    /// Distinct from an HTTP error response, which rides in `Response`.
    Error { message: String },
    /// No more steps.
    Done,
}
