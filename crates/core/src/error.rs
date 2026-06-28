use thiserror::Error;

#[derive(Debug, Error)]
pub enum GameError {
    // Typed recoverable errors (workflow layer can match on these)
    #[error("inventory full (497)")]
    InventoryFull,
    #[error("missing required item (478)")]
    MissingItem,
    #[error("insufficient gold (492)")]
    InsufficientGold,

    // Fatal errors
    #[error("skill level too low (493)")]
    SkillLevelTooLow,
    #[error("not enough HP (483)")]
    NotEnoughHp,
    #[error("max utilities equipped (484)")]
    MaxUtilitiesEquipped,
    #[error("already equipped (485)")]
    AlreadyEquipped,
    #[error("slot empty or occupied (491)")]
    SlotEmptyOrOccupied,
    #[error("already at destination (490)")]
    AlreadyAtDestination,
    #[error("character not found (498)")]
    CharacterNotFound,
    #[error("not found (404)")]
    NotFound,
    #[error("invalid payload (422)")]
    InvalidPayload,
    #[error("bank full (462)")]
    BankFull,
    #[error("map: no path (595)")]
    NoPath,
    #[error("map: blocked (596)")]
    MapBlocked,
    #[error("map: not found (597)")]
    MapNotFound,
    #[error("map: content not found (598)")]
    MapContentNotFound,
    #[error("bank: insufficient gold (460)")]
    BankInsufficientGold,
    #[error("bank: transaction in progress (461)")]
    BankTransactionInProgress,

    #[error("unexpected server error: status={status}, message={message}")]
    ServerError { status: u16, message: String },

    #[error("failed to parse response: {0}")]
    ParseError(#[from] serde_json::Error),

    #[error("internal error: {0}")]
    Internal(String),
}

/// Response envelope from the server.
#[derive(Debug, serde::Deserialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Debug, serde::Deserialize)]
struct ErrorBody {
    code: u16,
    message: String,
}

/// Classify an HTTP error response body into a typed GameError.
/// Returns None for transient codes that should be retried (499, 486, 429).
pub fn classify_error(status: u16, body: &[u8]) -> Option<GameError> {
    let message = parse_error_message(body);

    match status {
        // Transient — caller handles reschedule
        499 | 486 | 429 => None,

        // Recoverable
        497 => Some(GameError::InventoryFull),
        478 => Some(GameError::MissingItem),
        492 => Some(GameError::InsufficientGold),

        // Fatal
        493 => Some(GameError::SkillLevelTooLow),
        483 => Some(GameError::NotEnoughHp),
        484 => Some(GameError::MaxUtilitiesEquipped),
        485 => Some(GameError::AlreadyEquipped),
        491 => Some(GameError::SlotEmptyOrOccupied),
        490 => Some(GameError::AlreadyAtDestination),
        498 => Some(GameError::CharacterNotFound),
        404 => Some(GameError::NotFound),
        422 => Some(GameError::InvalidPayload),
        462 => Some(GameError::BankFull),
        595 => Some(GameError::NoPath),
        596 => Some(GameError::MapBlocked),
        597 => Some(GameError::MapNotFound),
        598 => Some(GameError::MapContentNotFound),
        460 => Some(GameError::BankInsufficientGold),
        461 => Some(GameError::BankTransactionInProgress),

        _ => Some(GameError::ServerError { status, message }),
    }
}

fn parse_error_message(body: &[u8]) -> String {
    if let Ok(env) = serde_json::from_slice::<ErrorEnvelope>(body) {
        env.error.message
    } else {
        String::from_utf8_lossy(body).into_owned()
    }
}

/// Extract remaining_seconds from a 499 response body.
pub fn parse_cooldown_remaining(body: &[u8]) -> Option<f64> {
    #[derive(serde::Deserialize)]
    struct CooldownError {
        error: CooldownData,
    }
    #[derive(serde::Deserialize)]
    struct CooldownData {
        data: Option<CooldownRemaining>,
    }
    #[derive(serde::Deserialize)]
    struct CooldownRemaining {
        cooldown: Option<CooldownInner>,
    }
    #[derive(serde::Deserialize)]
    struct CooldownInner {
        remaining_seconds: f64,
    }

    let env: CooldownError = serde_json::from_slice(body).ok()?;
    env.error.data?.cooldown.map(|c| c.remaining_seconds)
}
