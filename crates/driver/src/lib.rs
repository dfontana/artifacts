pub mod mock;

use std::time::Instant;
use artifacts_core::step::Step;

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
    /// No more steps.
    Done,
}
