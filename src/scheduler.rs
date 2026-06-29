use crate::driver::{Driver, DriverResult};
use artifacts_core::{
    cooldown::Cooldown,
    error::GameError,
    machine::{Core, Progress},
    step::{Intent, Outcome, OutcomeKind, Step},
};
use tokio::sync::{mpsc, oneshot};

use crate::view::SharedView;

/// Message sent from a Character handle to the Scheduler.
pub struct Submit {
    pub intent: Intent,
    pub reply: oneshot::Sender<Result<Outcome, GameError>>,
}

/// The async scheduler owns the Driver, the Core, and the rate-limit bucket for
/// a single character. (Multi-character scheduling is intentionally out of scope.)
pub struct Scheduler {
    core: Core,
    driver: Box<dyn Driver>,
    rx: mpsc::Receiver<Submit>,
    view: SharedView,
}

impl Scheduler {
    pub fn new(driver: Box<dyn Driver>, rx: mpsc::Receiver<Submit>, view: SharedView) -> Self {
        Self {
            core: Core::new(),
            driver,
            rx,
            view,
        }
    }

    /// Run until the channel closes. Blocks the calling thread (run on a dedicated thread).
    pub fn run(mut self) {
        while let Some(submit) = self.rx.blocking_recv() {
            let outcome = self.process(submit.intent);
            let _ = submit.reply.send(outcome);
        }
    }

    fn process(&mut self, intent: Intent) -> Result<Outcome, GameError> {
        self.core.enqueue(intent);

        loop {
            // Use the driver's clock — critical for MockDriver (fake clock) correctness.
            // After a Sleep step, the driver has advanced its clock; next_step must see
            // the post-sleep time or it will return Sleep again immediately.
            let now = self.driver.current_time();
            let step = self.core.next_step(now);

            match step {
                Step::Done => {
                    return Err(GameError::Internal("queue unexpectedly empty".into()));
                }
                Step::Sleep { until, reason } => {
                    self.driver.execute(Step::Sleep { until, reason });
                    // After execute(), driver.current_time() >= until.
                }
                Step::FetchData { path } => {
                    let _ = self.driver.execute(Step::FetchData { path });
                }
                Step::Request { method, path, body } => {
                    let result = self.driver.execute(Step::Request { method, path, body });

                    match result {
                        DriverResult::Response { status, body } => {
                            let now_after = self.driver.current_time();
                            match self.core.handle_response(status, &body, now_after) {
                                Ok(Progress::Complete(outcome)) => {
                                    self.view.update(outcome.character.clone());
                                    return Ok(outcome);
                                }
                                Ok(Progress::Retry) => continue, // transient (499/486/429)
                                Ok(Progress::NoOp) => {
                                    // Benign no-op (490): no state change. Report success
                                    // with the current cached view and a zero cooldown.
                                    return Ok(Outcome {
                                        cooldown: Cooldown::none(),
                                        character: self.view.get(),
                                        kind: OutcomeKind::NoOp,
                                    });
                                }
                                Err(e) => return Err(e),
                            }
                        }
                        DriverResult::Error { message } => {
                            return Err(GameError::Network(message));
                        }
                        _ => {
                            return Err(GameError::Internal("unexpected driver result".into()));
                        }
                    }
                }
            }
        }
    }
}
