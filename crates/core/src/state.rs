use std::time::{Duration, Instant};

/// Per-character cooldown state.
#[derive(Debug, Clone)]
pub struct CharacterState {
    /// Monotonic instant after which the character can act again.
    pub busy_until: Instant,
}

impl CharacterState {
    pub fn new() -> Self {
        Self {
            busy_until: Instant::now(),
        }
    }

    pub fn is_ready(&self, now: Instant) -> bool {
        now >= self.busy_until
    }

    pub fn set_cooldown(&mut self, remaining_secs: f64) {
        self.busy_until = Instant::now() + Duration::from_secs_f64(remaining_secs);
    }

    pub fn set_busy_until(&mut self, until: Instant) {
        self.busy_until = until;
    }
}

impl Default for CharacterState {
    fn default() -> Self {
        Self::new()
    }
}

/// Token bucket for one rate-limit window.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    pub tokens: u32,
    pub capacity: u32,
    pub resets_at: Instant,
    pub window: Duration,
}

impl TokenBucket {
    pub fn new(capacity: u32, window: Duration) -> Self {
        Self {
            tokens: capacity,
            capacity,
            resets_at: Instant::now() + window,
            window,
        }
    }

    pub fn try_consume(&mut self, now: Instant) -> bool {
        if now >= self.resets_at {
            self.tokens = self.capacity;
            self.resets_at = now + self.window;
        }
        if self.tokens > 0 {
            self.tokens -= 1;
            true
        } else {
            false
        }
    }

    pub fn is_available(&self, now: Instant) -> bool {
        now >= self.resets_at || self.tokens > 0
    }
}

/// Shared rate-limit state for one named bucket.
/// The `action` bucket is shared across all characters; the scheduler owns it.
/// Per-character buckets (none in v1) would live in CharacterState.
#[derive(Debug, Clone)]
pub struct RateLimitState {
    /// Per-second bucket.
    pub per_second: TokenBucket,
    /// Per-minute bucket.
    pub per_minute: TokenBucket,
}

impl RateLimitState {
    /// Construct for the `action` bucket: 10/s, 100/min.
    pub fn action() -> Self {
        Self {
            per_second: TokenBucket::new(10, Duration::from_secs(1)),
            per_minute: TokenBucket::new(100, Duration::from_secs(60)),
        }
    }

    /// Returns the earliest Instant at which a token will be available, or None if available now.
    pub fn next_available(&self, now: Instant) -> Option<Instant> {
        let mut blocked_until: Option<Instant> = None;

        for bucket in [&self.per_second, &self.per_minute] {
            if !bucket.is_available(now) {
                let candidate = bucket.resets_at;
                blocked_until = Some(match blocked_until {
                    Some(prev) => prev.max(candidate),
                    None => candidate,
                });
            }
        }

        blocked_until
    }

    pub fn try_consume(&mut self, now: Instant) -> bool {
        // Peek first — don't consume one bucket if the other is empty.
        if self.next_available(now).is_some() {
            return false;
        }
        self.per_second.try_consume(now) && self.per_minute.try_consume(now)
    }
}
