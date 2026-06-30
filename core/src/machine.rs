use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::{
    cooldown::Cooldown,
    error::{classify_error, parse_cooldown_remaining, GameError},
    state::{CharacterState, RateLimitState},
    step::{CharacterView, Intent, Method, Outcome, OutcomeKind, SleepReason, Step},
};

/// The result of feeding one HTTP response into the state machine.
#[derive(Debug)]
pub enum Progress {
    /// The action completed; here is the parsed outcome. Boxed because `Outcome`
    /// (a full `CharacterView` + fight details) dwarfs the other variants.
    Complete(Box<Outcome>),
    /// A transient condition (499 cooldown / 486 in-progress / 429 rate-limit)
    /// rescheduled the intent — call `next_step` again and retry.
    Retry,
    /// The action was a benign no-op (HTTP 490: already at destination).
    /// No state changed and no cooldown was incurred; the caller should report
    /// success using its own cached character view.
    NoOp,
}

/// Pure state machine. No I/O, no clock reads — callers supply `now: Instant`.
pub struct Core {
    pub queue: VecDeque<Intent>,
    pub character: CharacterState,
    pub buckets: RateLimitState,
    /// Current intent being processed (waiting for response).
    pending: Option<Intent>,
}

impl Core {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            character: CharacterState::new(),
            buckets: RateLimitState::action(),
            pending: None,
        }
    }

    pub fn enqueue(&mut self, intent: Intent) {
        self.queue.push_back(intent);
    }

    /// Put the in-flight intent back at the front of the queue so a transient
    /// failure (499/486/429) retries it after the reschedule.
    fn requeue_pending(&mut self) {
        if let Some(intent) = self.pending.take() {
            self.queue.push_front(intent);
        }
    }

    /// PURE: given current clock reading, decide the next step.
    pub fn next_step(&mut self, now: Instant) -> Step {
        // If still in cooldown, sleep until ready.
        if !self.character.is_ready(now) {
            return Step::Sleep {
                until: self.character.busy_until,
                reason: SleepReason::Cooldown,
            };
        }

        // Check rate limit.
        if let Some(reset_at) = self.buckets.next_available(now) {
            return Step::Sleep {
                until: reset_at,
                reason: SleepReason::RateLimit,
            };
        }

        // Dequeue the next intent.
        let Some(intent) = self.queue.pop_front() else {
            return Step::Done;
        };

        // Consume a token from the rate-limit bucket.
        // (We already confirmed availability above, so this must succeed.)
        self.buckets.try_consume(now);

        let step = build_request(&intent);
        self.pending = Some(intent);
        step
    }

    /// PURE: feed the HTTP response back in; update cooldown + buckets; return Outcome.
    /// Feed an HTTP response back in; update cooldown + buckets.
    /// - `Ok(Progress::Complete)` — action succeeded.
    /// - `Ok(Progress::Retry)`    — transient (499/486/429), rescheduled; call again.
    /// - `Ok(Progress::NoOp)`     — benign no-op (490 already at destination).
    /// - `Err(_)`                 — fatal game error.
    pub fn handle_response(
        &mut self,
        status: u16,
        body: &[u8],
        now: Instant,
    ) -> Result<Progress, GameError> {
        match status {
            200 | 201 => {
                let outcome = parse_action_response(body, self.pending.as_ref())?;
                self.character.set_busy_until(
                    now + Duration::from_secs_f64(outcome.cooldown.remaining_seconds),
                );
                self.pending = None;
                Ok(Progress::Complete(Box::new(outcome)))
            }
            490 => {
                // Already at destination: a move to the current tile. Not an error —
                // the character is where we wanted it. No cooldown, no retry.
                self.pending = None;
                Ok(Progress::NoOp)
            }
            499 => {
                // Character in cooldown — reschedule without surfacing as error.
                let remaining = parse_cooldown_remaining(body).unwrap_or(1.0);
                self.character
                    .set_busy_until(now + Duration::from_secs_f64(remaining));
                self.requeue_pending();
                Ok(Progress::Retry)
            }
            486 => {
                // Action already in progress — short retry.
                self.character
                    .set_busy_until(now + Duration::from_millis(500));
                self.requeue_pending();
                Ok(Progress::Retry)
            }
            429 => {
                // Rate limited — mark bucket exhausted and reschedule.
                // We don't know exact reset time from the body; back off 1 second.
                self.buckets.per_second.tokens = 0;
                self.buckets.per_second.resets_at = now + Duration::from_secs(1);
                self.requeue_pending();
                Ok(Progress::Retry)
            }
            other => {
                self.pending = None;
                match classify_error(other, body) {
                    Some(err) => Err(err),
                    None => Err(GameError::ServerError {
                        status: other,
                        message: String::from_utf8_lossy(body).into_owned(),
                    }),
                }
            }
        }
    }
}

impl Default for Core {
    fn default() -> Self {
        Self::new()
    }
}

/// A bodyless POST action (character name rides in the path, set by the driver).
fn post(path: &str) -> Step {
    Step::Request {
        method: Method::Post,
        path: path.into(),
        body: None,
    }
}

/// A POST action carrying a JSON body.
fn post_json(path: &str, body: serde_json::Value) -> Step {
    Step::Request {
        method: Method::Post,
        path: path.into(),
        body: Some(serde_json::to_vec(&body).unwrap()),
    }
}

fn build_request(intent: &Intent) -> Step {
    use serde_json::json;
    match intent {
        Intent::Move { x, y } => post_json("action/move", json!({"x": x, "y": y})),
        Intent::Gather => post("action/gathering"),
        Intent::Fight => post("action/fight"),
        Intent::Rest => post("action/rest"),
        Intent::Craft { code, quantity } => post_json(
            "action/crafting",
            json!({"code": code, "quantity": quantity}),
        ),
        Intent::Equip {
            code,
            slot,
            quantity,
        } => post_json(
            "action/equip",
            json!({"code": code, "slot": slot, "quantity": quantity}),
        ),
        Intent::Unequip { slot, quantity } => post_json(
            "action/unequip",
            json!({"slot": slot, "quantity": quantity}),
        ),
        Intent::DepositItem { code, quantity } => post_json(
            "action/bank/deposit/item",
            json!({"code": code, "quantity": quantity}),
        ),
        Intent::WithdrawItem { code, quantity } => post_json(
            "action/bank/withdraw/item",
            json!({"code": code, "quantity": quantity}),
        ),
        // Expanded into individual DepositItem intents above the core (see
        // Character::deposit_all); the bare intent should never reach the wire.
        Intent::DepositAll => post("action/bank/deposit/item"),
        Intent::UseItem { code, quantity } => {
            post_json("action/use", json!({"code": code, "quantity": quantity}))
        }
        Intent::Recycle { code, quantity } => post_json(
            "action/recycling",
            json!({"code": code, "quantity": quantity}),
        ),
    }
}

fn parse_action_response(body: &[u8], intent: Option<&Intent>) -> Result<Outcome, GameError> {
    use crate::step::DropItem;

    // All action responses have a common envelope shape.
    #[derive(serde::Deserialize)]
    struct Envelope {
        data: Data,
    }

    // The live API is not uniform: most actions return a single `character`,
    // but `action/fight` returns a `characters` array (and tucks xp/gold/drops
    // inside `fight.characters[]`, not at the top of `fight`). Accept both.
    #[derive(serde::Deserialize)]
    struct Data {
        cooldown: Cooldown,
        #[serde(default)]
        character: Option<CharacterView>,
        #[serde(default)]
        characters: Vec<CharacterView>,
        #[serde(default)]
        fight: Option<FightData>,
        #[serde(default)]
        details: Option<DetailsData>,
    }

    #[derive(serde::Deserialize, Default)]
    struct FightData {
        turns: u32,
        result: String,
        /// Per-character fight outcome (xp/gold/drops live here on the live API).
        #[serde(default)]
        characters: Vec<FightCharacter>,
    }

    #[derive(serde::Deserialize, Default)]
    struct FightCharacter {
        #[serde(default)]
        xp: u32,
        #[serde(default)]
        gold: u32,
        #[serde(default)]
        drops: Vec<DropItem>,
    }

    #[derive(serde::Deserialize, Default)]
    struct DetailsData {
        #[serde(default)]
        items: Vec<DropItem>,
        #[serde(default)]
        hp_restored: Option<u32>,
        #[serde(default)]
        xp: Option<u32>,
    }

    let env: Envelope = serde_json::from_slice(body)?;
    let mut data = env.data;

    let kind = match intent {
        Some(Intent::Move { .. }) => OutcomeKind::Move,
        Some(Intent::Gather) => OutcomeKind::Gather {
            items: data.details.unwrap_or_default().items,
        },
        Some(Intent::Fight) => {
            let f = data.fight.take().unwrap_or_default();
            use crate::step::{FightOutcome, FightResult};
            // xp/gold/drops are reported per character; this client drives one.
            let outcome = f.characters.into_iter().next().unwrap_or_default();
            OutcomeKind::Fight(FightResult {
                turns: f.turns,
                result: if f.result == "win" {
                    FightOutcome::Win
                } else {
                    FightOutcome::Lose
                },
                xp: outcome.xp,
                gold: outcome.gold,
                drops: outcome.drops,
            })
        }
        Some(Intent::Rest) => OutcomeKind::Rest {
            hp_restored: data.details.unwrap_or_default().hp_restored.unwrap_or(0),
        },
        Some(Intent::Craft { .. }) => OutcomeKind::Craft {
            items: data.details.unwrap_or_default().items,
        },
        Some(Intent::DepositItem { .. }) => OutcomeKind::Deposit {
            items: data.details.unwrap_or_default().items,
        },
        Some(Intent::WithdrawItem { .. }) => OutcomeKind::Withdraw {
            items: data.details.unwrap_or_default().items,
        },
        Some(Intent::Equip { .. }) => OutcomeKind::Equip,
        Some(Intent::Unequip { .. }) => OutcomeKind::Unequip,
        Some(Intent::UseItem { .. }) => OutcomeKind::UseItem,
        Some(Intent::Recycle { .. }) => OutcomeKind::Recycle {
            items: data.details.unwrap_or_default().items,
        },
        Some(Intent::DepositAll) | None => OutcomeKind::DepositAll { items: vec![] },
    };

    // Resolve the character snapshot from whichever field the action populated.
    let character = data
        .character
        .or_else(|| data.characters.into_iter().next())
        .ok_or_else(|| GameError::Internal("action response had no character".into()))?;

    Ok(Outcome {
        cooldown: data.cooldown,
        character,
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn make_200_response(cooldown_remaining: f64) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "data": {
                "cooldown": {
                    "total_seconds": cooldown_remaining,
                    "remaining_seconds": cooldown_remaining,
                    "started_at": "2024-01-01T00:00:00Z",
                    "expiration": "2024-01-01T00:01:00Z",
                    "reason": "gathering"
                },
                "character": {
                    "name": "kael",
                    "x": 2,
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

    fn make_499_response(remaining: f64) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "error": {
                "code": 499,
                "message": "Character in cooldown.",
                "data": {
                    "cooldown": {
                        "remaining_seconds": remaining
                    }
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn test_499_reschedules_not_errors() {
        let mut core = Core::new();
        core.enqueue(Intent::Gather);

        let now = Instant::now();

        // Get the request step.
        let step = core.next_step(now);
        assert!(
            matches!(step, Step::Request { .. }),
            "expected Request step"
        );

        // Simulate a 499 response.
        let body = make_499_response(5.0);
        let result = core.handle_response(499, &body, now);
        assert!(
            matches!(result, Ok(Progress::Retry)),
            "499 must reschedule (Progress::Retry), got: {:?}",
            result
        );

        // Core should now sleep.
        let now2 = now + Duration::from_millis(100);
        let step2 = core.next_step(now2);
        assert!(
            matches!(
                step2,
                Step::Sleep {
                    reason: SleepReason::Cooldown,
                    ..
                }
            ),
            "expected Sleep(Cooldown) after 499, got: {:?}",
            step2
        );
    }

    #[test]
    fn test_486_reschedules() {
        let mut core = Core::new();
        core.enqueue(Intent::Gather);

        let now = Instant::now();
        let _step = core.next_step(now);

        let result = core.handle_response(486, b"{}", now);
        assert!(
            matches!(result, Ok(Progress::Retry)),
            "486 retries, got: {result:?}"
        );

        // Re-queued — should produce another Request after cooldown.
        let step2 = core.next_step(now + Duration::from_secs(1));
        assert!(matches!(step2, Step::Request { .. } | Step::Sleep { .. }));
    }

    #[test]
    fn test_200_advances_cooldown() {
        let mut core = Core::new();
        core.enqueue(Intent::Gather);

        let now = Instant::now();
        let _step = core.next_step(now);

        let body = make_200_response(30.0);
        let result = core.handle_response(200, &body, now);
        let outcome = match result {
            Ok(Progress::Complete(o)) => o,
            other => panic!("expected Complete, got: {other:?}"),
        };
        assert!((outcome.cooldown.remaining_seconds - 30.0).abs() < 0.01);

        // Immediately after, still in cooldown.
        let step = core.next_step(now + Duration::from_millis(100));
        assert!(matches!(
            step,
            Step::Sleep {
                reason: SleepReason::Cooldown,
                ..
            }
        ));

        // After cooldown, queue is empty → Done.
        let step = core.next_step(now + Duration::from_secs(31));
        assert!(matches!(step, Step::Done));
    }

    #[test]
    fn test_fatal_error_propagates() {
        let mut core = Core::new();
        core.enqueue(Intent::Gather);

        let now = Instant::now();
        let _step = core.next_step(now);

        let body = serde_json::to_vec(&serde_json::json!({
            "error": {"code": 493, "message": "Skill level too low."}
        }))
        .unwrap();
        let result = core.handle_response(493, &body, now);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), GameError::SkillLevelTooLow));
    }

    #[test]
    fn test_490_is_benign_noop() {
        let mut core = Core::new();
        core.enqueue(Intent::Move { x: 2, y: 0 });

        let now = Instant::now();
        let step = core.next_step(now);
        assert!(matches!(step, Step::Request { .. }));

        // Already at destination: must NOT be a fatal error, and must NOT retry.
        let body = serde_json::to_vec(&serde_json::json!({
            "error": {"code": 490, "message": "Character already at destination."}
        }))
        .unwrap();
        let result = core.handle_response(490, &body, now);
        assert!(
            matches!(result, Ok(Progress::NoOp)),
            "490 must be a benign no-op, got: {result:?}"
        );

        // Intent consumed (not re-enqueued); no cooldown set → next step is Done.
        let step = core.next_step(now + Duration::from_millis(10));
        assert!(
            matches!(step, Step::Done),
            "queue should be empty after 490 no-op"
        );
    }
}
