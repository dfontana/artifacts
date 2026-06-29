/// CooldownSchema: parsed from every action response.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Cooldown {
    pub total_seconds: f64,
    pub remaining_seconds: f64,
    pub started_at: String,
    pub expiration: String,
    pub reason: String,
}

impl Cooldown {
    /// A zero-duration cooldown, used for no-op outcomes (e.g. a redundant move).
    pub fn none() -> Self {
        Self {
            total_seconds: 0.0,
            remaining_seconds: 0.0,
            started_at: String::new(),
            expiration: String::new(),
            reason: String::new(),
        }
    }
}

/// Predicted cooldown duration in seconds for each action type.
/// These are the CLIENT-SIDE formulas used by the estimate/simulate passes.
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

    /// Fight: 2s per turn.
    pub fn fight(turns: u32) -> f64 {
        2.0 * turns as f64
    }

    /// Rest: 1s per 5 HP, minimum 3s.
    pub fn rest(hp_to_restore: u32) -> f64 {
        f64::max(3.0, (hp_to_restore as f64 / 5.0).ceil())
    }

    /// Crafting: 5s per item.
    pub fn crafting(quantity: u32) -> f64 {
        5.0 * quantity as f64
    }

    /// Recycling: 3s per item.
    pub fn recycling(quantity: u32) -> f64 {
        3.0 * quantity as f64
    }

    /// Deposit/Withdraw/Give: 3s per distinct item type.
    pub fn deposit(distinct_types: u32) -> f64 {
        3.0 * distinct_types as f64
    }

    /// Default for unspecified actions.
    pub fn default_action() -> f64 {
        3.0
    }
}
