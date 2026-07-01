//! App state: the mode/focus state, the Idle→Running→Stopping run machine, and
//! the worker + abort handles (`plans/TUI.md` §3.4, §5). The UI thread never
//! blocks on the run: it reads the cheap shared cells this module owns.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;
use artifacts_core::map::GameMap;

use crate::data::MonsterData;
use crate::driver::http::HttpDriver;
use crate::planner::{self, PlanResult, PlanSeed};
use crate::tui::reducer::RunPhase;
use crate::tui::skeleton::PlanStep;
use crate::tui::workflows::{self, Workflow};
use crate::tui::{new_progress_log, ProgressLog};
use crate::view::SharedView;

/// How often the idle poll refreshes the character snapshot.
const POLL_INTERVAL: Duration = Duration::from_secs(3);
/// Spinner animation cadence.
const SPINNER_INTERVAL: Duration = Duration::from_millis(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Interact,
    Focus,
}

/// The focusable panes (the header and power bar are chrome, not focusable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Stats,
    Inventory,
    Workflows,
    Run,
}

impl Pane {
    /// Left→right, top→bottom focus order for the arrow keys.
    pub const ORDER: [Pane; 4] = [Pane::Stats, Pane::Inventory, Pane::Workflows, Pane::Run];
}

/// The launch guard machine (§5.3). A new run can start **only** from `Idle`,
/// which is what prevents two schedulers driving one character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Idle,
    Running,
    Stopping,
}

/// The worker's published run status, read by the render each frame so terminal
/// frames are correct (§3.4). The window before the skeleton is handed off shows
/// `preparing run…`, driven by the skeleton's absence rather than a status.
#[derive(Debug, Clone)]
pub enum RunStatus {
    Running,
    Done,
    Failed(String),
}

impl RunStatus {
    pub fn phase(&self) -> RunPhase {
        match self {
            RunStatus::Running => RunPhase::Running,
            RunStatus::Done => RunPhase::Done,
            RunStatus::Failed(_) => RunPhase::Failed,
        }
    }
}

/// The bundle of handles shared between the UI thread and the run worker (§3.4).
/// The UI reads all of these each frame (cheap, non-blocking); the worker sets
/// `skeleton` once (the handoff) and advances `status`.
#[derive(Clone)]
pub struct RunSession {
    pub view: SharedView,
    pub progress: ProgressLog,
    pub skeleton: Arc<OnceLock<Vec<PlanStep>>>,
    pub status: Arc<Mutex<RunStatus>>,
    pub abort: Arc<AtomicBool>,
}

impl RunSession {
    pub fn new(view: SharedView) -> Self {
        Self {
            view,
            progress: new_progress_log(),
            skeleton: Arc::new(OnceLock::new()),
            status: Arc::new(Mutex::new(RunStatus::Running)),
            abort: Arc::new(AtomicBool::new(false)),
        }
    }
}

pub struct App {
    pub character: String,
    /// The one SharedView the header/stats/inventory read — updated by the idle
    /// poll when Idle and by the run scheduler when Running (§3.8).
    pub view: SharedView,
    pub map: Option<Arc<GameMap>>,
    pub monsters: Option<Arc<MonsterData>>,
    /// Kept from `load_live_context` for the initial fetch + idle polls (§3.5).
    poll_driver: HttpDriver,

    pub workflows: Vec<Workflow>,
    pub selected: usize,
    /// The browsing plan for the selected workflow (offline planner).
    pub plan: Option<Result<PlanResult, String>>,

    pub mode: Mode,
    pub focus: Pane,
    pub inventory_scroll: usize,

    pub run_state: RunState,
    pub session: Option<RunSession>,
    run_handle: Option<JoinHandle<Result<()>>>,

    /// A transient hint / prompt shown in the power bar.
    pub status_msg: Option<String>,
    /// A blocking failure pop-over (dismissed with Esc).
    pub error_popover: Option<String>,
    /// After a first `r` on an infeasible plan, capital `R` overrides.
    pub infeasible_prompt: bool,

    pub spinner: usize,
    last_poll: Instant,
    last_spin: Instant,
    pub should_quit: bool,
}

impl App {
    pub fn new(
        character: String,
        view: SharedView,
        map: Option<Arc<GameMap>>,
        monsters: Option<Arc<MonsterData>>,
        poll_driver: HttpDriver,
    ) -> Self {
        let workflows = workflows::scan(workflows::DEFAULT_DIR).unwrap_or_default();
        let mut app = Self {
            character,
            view,
            map,
            monsters,
            poll_driver,
            workflows,
            selected: 0,
            plan: None,
            mode: Mode::Normal,
            focus: Pane::Workflows,
            inventory_scroll: 0,
            run_state: RunState::Idle,
            session: None,
            run_handle: None,
            status_msg: None,
            error_popover: None,
            infeasible_prompt: false,
            spinner: 0,
            last_poll: Instant::now(),
            last_spin: Instant::now(),
            should_quit: false,
        };
        app.refresh_plan();
        app
    }

    pub fn selected_workflow(&self) -> Option<&Workflow> {
        self.workflows.get(self.selected)
    }

    /// Recompute the browsing plan for the selected workflow via the offline
    /// planner (always safe — no I/O). Seeded from the current live view.
    pub fn refresh_plan(&mut self) {
        self.infeasible_prompt = false;
        let Some(wf) = self.workflows.get(self.selected) else {
            self.plan = None;
            return;
        };
        let seed = PlanSeed::from_view(&self.view.get());
        let result = planner::plan(&wf.src, self.map.clone(), self.monsters.clone(), &seed)
            .map_err(|e| e.to_string());
        self.plan = Some(result);
    }

    // ─── periodic work (called every render loop) ─────────────────────────────

    pub fn tick(&mut self) {
        // Spinner animation, time-driven so it advances without input.
        if self.last_spin.elapsed() >= SPINNER_INTERVAL {
            self.spinner = self.spinner.wrapping_add(1);
            self.last_spin = Instant::now();
        }
        self.reap_worker();
        self.poll_idle();
    }

    /// Detect the worker exiting and settle the run machine back to `Idle`
    /// (`Stopping → Idle` is the launch guard, §5.3).
    fn reap_worker(&mut self) {
        let finished = self.run_handle.as_ref().is_some_and(|h| h.is_finished());
        if !finished {
            return;
        }
        if let Some(handle) = self.run_handle.take() {
            let _ = handle.join();
        }
        // The worker publishes its own terminal status; surface a failure.
        if let Some(session) = &self.session {
            if let RunStatus::Failed(msg) = &*session.status.lock().unwrap() {
                self.error_popover = Some(msg.clone());
            }
        }
        self.status_msg = None;
        self.run_state = RunState::Idle;
    }

    /// Refresh the character snapshot while idle; suppressed during a run because
    /// the scheduler already server-trues `SharedView` every action (§3.7).
    fn poll_idle(&mut self) {
        if self.run_state != RunState::Idle {
            return;
        }
        if self.last_poll.elapsed() < POLL_INTERVAL {
            return;
        }
        self.last_poll = Instant::now();
        match self.poll_driver.fetch_character() {
            Ok(v) => self.view.update(v),
            Err(e) => self.status_msg = Some(format!("poll failed: {e}")),
        }
    }

    // ─── run lifecycle ────────────────────────────────────────────────────────

    /// Launch a run of the selected workflow. `force` overrides an infeasible
    /// plan (capital `R`). Allowed only from `Idle` (§5.3).
    pub fn launch_run(&mut self, force: bool) {
        if self.run_state != RunState::Idle {
            return;
        }
        let Some(wf) = self.workflows.get(self.selected).cloned() else {
            self.status_msg = Some("no workflow selected".into());
            return;
        };
        // Gate on the browsing plan's feasibility unless overriding.
        let feasible = matches!(&self.plan, Some(Ok(p)) if p.feasible);
        if !feasible && !force {
            self.infeasible_prompt = true;
            self.status_msg = Some("infeasible — press R to override".into());
            return;
        }
        self.infeasible_prompt = false;

        let session = RunSession::new(self.view.clone());
        let initial = self.view.get();
        let handle = crate::tui::run_worker::spawn_tui_run(
            &self.character,
            wf.src,
            initial,
            self.map.clone(),
            self.monsters.clone(),
            session.clone(),
        );
        match handle {
            Ok(handle) => {
                self.session = Some(session);
                self.run_handle = Some(handle);
                self.run_state = RunState::Running;
                self.status_msg = Some(format!("running {}…", wf.name));
            }
            Err(e) => self.status_msg = Some(format!("launch failed: {e}")),
        }
    }

    /// Request cancellation of the active run (`x` in the Run pane). Hard kill:
    /// set the abort flag and flip to `Stopping`; the worker unwinds and
    /// `reap_worker` returns us to `Idle` (§5.3).
    pub fn stop_run(&mut self) {
        if self.run_state != RunState::Running {
            return;
        }
        if let Some(session) = &self.session {
            session.abort.store(true, Ordering::SeqCst);
        }
        self.run_state = RunState::Stopping;
        self.status_msg = Some("stopping…".into());
    }
}
