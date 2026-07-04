/// CooldownSchema: parsed from every action response. Only the fields the
/// client reads are kept; serde ignores the rest (`started_at`, `expiration`,
/// `reason`) — the TUI's cooldown bar reads `CharacterView::cooldown_expiration`
/// instead.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Cooldown {
    pub total_seconds: f64,
    pub remaining_seconds: f64,
}

impl Cooldown {
    /// A zero-duration cooldown, used for no-op outcomes (e.g. a redundant move).
    pub fn none() -> Self {
        Self {
            total_seconds: 0.0,
            remaining_seconds: 0.0,
        }
    }
}

/// Predicted cooldown duration in seconds for each action type.
/// These are the CLIENT-SIDE formulas used by the plan pass.
/// The server's returned `expiration` is authoritative at run time.
pub mod formulas {
    /// Movement: 5s per tile (Manhattan distance).
    pub fn movement(tiles: u32) -> f64 {
        5.0 * tiles as f64
    }

    /// Gathering: 30s + floor(resource_level / 2).
    pub fn gathering(resource_level: u32) -> f64 {
        30.0 + (resource_level / 2) as f64
    }

    /// Fight: 2s per turn, reduced by haste (1 haste = 1% off). Verified live:
    /// a 29-turn fight at haste 0 returned a 58s cooldown (2×29).
    pub fn fight(turns: u32, haste: i32) -> f64 {
        let base = 2.0 * turns as f64;
        base * (1.0 - haste as f64 / 100.0)
    }

    /// Rest: 1s per 5 HP, minimum 3s.
    pub fn rest(hp_to_restore: u32) -> f64 {
        f64::max(3.0, (hp_to_restore as f64 / 5.0).ceil())
    }

    /// Deposit/Withdraw/Give: 3s per distinct item type.
    pub fn deposit(distinct_types: u32) -> f64 {
        3.0 * distinct_types as f64
    }
}

use jiff::Timestamp;
use std::time::Duration;

/// Parse an RFC3339 timestamp (the server's cooldown `expiration` format).
/// Returns `None` on anything unparseable — callers (e.g. the TUI cooldown
/// bar) then simply read "no cooldown" rather than erroring.
pub fn parse_rfc3339(s: &str) -> Option<Timestamp> {
    s.trim().parse().ok()
}

/// Remaining cooldown until `expiration`, clamped at zero — an expiration
/// already in the past has 0 remaining. Lives in `core` so the rule isn't
/// reimplemented in the presentation layer; the caller supplies `now` (this
/// crate stays sans-I/O).
pub fn remaining(expiration: Timestamp, now: Timestamp) -> Duration {
    let elapsed = expiration.duration_since(now);
    if elapsed.is_negative() {
        Duration::ZERO
    } else {
        elapsed.unsigned_abs()
    }
}

#[cfg(test)]
mod tests {
    use super::remaining;
    use jiff::Timestamp;
    use std::time::Duration;

    fn ts(epoch_secs: i64) -> Timestamp {
        Timestamp::from_second(epoch_secs).unwrap()
    }

    #[test]
    fn remaining_counts_down_then_clamps() {
        // Future expiration counts down linearly against `now`.
        assert_eq!(remaining(ts(100), ts(90)), Duration::from_secs(10));
        // Exactly at expiration: 0 remaining.
        assert_eq!(remaining(ts(100), ts(100)), Duration::ZERO);
        // Past expiration is clamped to 0 — never negative.
        assert_eq!(remaining(ts(100), ts(110)), Duration::ZERO);
        assert_eq!(remaining(ts(100), ts(200)), Duration::ZERO);
    }
}
