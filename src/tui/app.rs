//! App state: the mode/focus state, the Idle→Running→Stopping run machine, and
//! the worker + abort handles (`plans/TUI.md` §3.4, §5). The UI thread never
//! blocks on the run: it reads the cheap shared cells this module owns.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;
use artifacts_core::map::GameMap;

use crate::character::SharedView;
use crate::data::{MonsterData, ResourceData};
use crate::driver::http::HttpDriver;
use crate::planner::{self, PlanResult, PlanSeed};
use crate::tui::reducer::{reduce, RowState, RunPhase};
use crate::tui::skeleton::PlanStep;
use crate::tui::workflows::{self, Workflow};
use crate::tui::{new_progress_log, ProgressLog};

/// How often the idle poll refreshes the character snapshot.
const POLL_INTERVAL: Duration = Duration::from_secs(3);
/// Spinner animation cadence.
const SPINNER_INTERVAL: Duration = Duration::from_millis(120);

/// The dashboard panes. The tiling runtime owns which one holds focus and where
/// it sits; this enum just names them so key routing, the footer, and the zoom
/// overlay can ask "what kind of pane is focused?" (the header and footer are
/// fixed chrome, not tiles).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Stats,
    Inventory,
    Workflows,
    Run,
}

/// Open command-palette state (`p`): the fuzzy query typed so far and the
/// highlighted row in the filtered list. `None` when the palette is closed.
#[derive(Default)]
pub struct Palette {
    pub query: String,
    pub selected: usize,
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

/// Lock a session mutex, recovering from poisoning instead of propagating the
/// poison and crashing the TUI on every subsequent tick. A `Mutex` is poisoned
/// when the run worker panics while holding one of its guards; without this
/// recovery, every later `app.tick()` would `.lock().unwrap()` → panic → the
/// whole TUI dies instead of just failing the run. The panic itself is surfaced
/// separately in `reap_worker` (via `JoinHandle::join`); this helper only keeps
/// the UI rendering the (possibly stale) data. Note: a `catch_unwind` around the
/// worker does **not** prevent poisoning — a guard's `Drop` runs during
/// unwinding and poisons the mutex before any outer boundary. (CAND-5)
pub(crate) fn lock_or_recover<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The run panel's memoized derived state — the reduced rows, the
/// `(id-log length, phase)` key they were reduced from, and the per-row display
/// labels — bundled into one struct so they are built and cleared together.
/// Stored as `Option<RunRowCache>` on `App`: rebuilding is one assignment in
/// `refresh_run_rows`, and relaunch clears it with one `run_cache = None`.
/// Bundling removes the drift a 3-field manual cache could suffer, where a
/// reset that forgot to clear all three would show stale rows or a stale label
/// count. (CAND-9)
pub(crate) struct RunRowCache {
    /// `(id-log length, phase)` the rows were reduced from; when it moves,
    /// `reduce` must run again.
    pub(crate) key: (usize, RunPhase),
    /// Reduced per-row state for the published skeleton.
    pub(crate) rows: Vec<RowState>,
    /// Per-row display labels, formatted from the skeleton.
    pub(crate) labels: Vec<String>,
}

pub struct App {
    pub character: String,
    /// The one SharedView the header/stats/inventory read — updated by the idle
    /// poll when Idle and by the run scheduler when Running (§3.8).
    pub view: SharedView,
    pub map: Option<Arc<GameMap>>,
    pub monsters: Option<Arc<MonsterData>>,
    pub resources: Option<Arc<ResourceData>>,
    /// Set true only while `run_state == Idle`; the idle-poll thread reads it and
    /// fetches the character snapshot only when it is set (§3.4, §3.7).
    poll_idle_flag: Arc<AtomicBool>,
    /// Set on quit to tell the idle-poll thread to exit (clean shutdown).
    poll_stop: Arc<AtomicBool>,

    pub workflows: Vec<Workflow>,
    pub selected: usize,
    /// Browsing plans already computed this session, by workflow index, with the
    /// seed each was planned from. Planning re-bootstraps the Fennel compiler on
    /// the render thread, so selection changes reuse the cached result until the
    /// live seed actually moves (`p` bypasses the cache).
    plan_cache: HashMap<usize, (PlanSeed, Result<PlanResult, String>)>,

    /// Zoom overlay toggle (`z`): render the focused pane as a centered modal
    /// on top of the tiled body. Focus itself lives in the tiling runtime.
    pub zoom: bool,
    /// Open command palette (`p`), or `None` when closed.
    pub palette: Option<Palette>,
    pub inventory_scroll: usize,

    pub run_state: RunState,
    pub session: Option<RunSession>,
    run_handle: Option<JoinHandle<Result<()>>>,
    /// The run panel's memoized derived state — rows, their invalidate key, and
    /// the per-row labels — built and cleared as one `RunRowCache` so the parts
    /// can never drift. `None` before the skeleton is published and on relaunch.
    /// (CAND-9)
    pub(crate) run_cache: Option<RunRowCache>,

    /// A transient hint / prompt shown in the power bar.
    pub status_msg: Option<String>,
    /// A blocking failure pop-over (dismissed with Esc).
    pub error_popover: Option<String>,
    /// After a first `r` on an infeasible plan, capital `R` overrides.
    pub infeasible_prompt: bool,

    pub spinner: usize,
    last_spin: Instant,
    pub should_quit: bool,
}

impl App {
    pub fn new(
        character: String,
        view: SharedView,
        map: Option<Arc<GameMap>>,
        monsters: Option<Arc<MonsterData>>,
        resources: Option<Arc<ResourceData>>,
        poll_driver: HttpDriver,
    ) -> Self {
        let workflows = workflows::scan(workflows::DEFAULT_DIR).unwrap_or_default();
        // Idle polling runs OFF the render thread (§3.4: the main loop has no
        // blocking calls). The dedicated thread owns `poll_driver`, fetches only
        // while Idle, and publishes into the `SharedView` the render thread reads
        // each frame. Starts Idle, so the flag starts set.
        let poll_idle_flag = Arc::new(AtomicBool::new(true));
        let poll_stop = Arc::new(AtomicBool::new(false));
        std::thread::spawn({
            let view = view.clone();
            let idle = poll_idle_flag.clone();
            let stop = poll_stop.clone();
            move || poll_loop(poll_driver, view, idle, stop)
        });
        let mut app = Self {
            character,
            view,
            map,
            monsters,
            resources,
            poll_idle_flag,
            poll_stop,
            workflows,
            selected: 0,
            plan_cache: HashMap::new(),
            zoom: false,
            palette: None,
            inventory_scroll: 0,
            run_state: RunState::Idle,
            session: None,
            run_handle: None,
            run_cache: None,
            status_msg: None,
            error_popover: None,
            infeasible_prompt: false,
            spinner: 0,
            last_spin: Instant::now(),
            should_quit: false,
        };
        app.refresh_plan();
        app
    }

    /// Signal the idle-poll thread to exit. Called from `tui::run` on teardown.
    /// Non-blocking by design: we set the stop flag and let the thread notice it
    /// on its next wake (≤ ~100 ms); we deliberately do **not** join, so a slow or
    /// hung in-flight fetch can never make quit hang (the very failure Fix #1
    /// removes from the render thread).
    pub fn shutdown(&self) {
        self.poll_stop.store(true, Ordering::SeqCst);
    }

    pub fn selected_workflow(&self) -> Option<&Workflow> {
        self.workflows.get(self.selected)
    }

    /// Move the workflow selection by `delta` (wrapping) and refresh the browsing
    /// plan for the new selection. Resets the inventory scroll so a fresh pane
    /// starts at the top.
    pub fn select(&mut self, delta: isize) {
        if self.workflows.is_empty() {
            return;
        }
        let n = self.workflows.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(n) as usize;
        self.inventory_scroll = 0;
        self.refresh_plan();
    }

    /// Scroll the inventory list by `delta` rows (saturating at 0).
    pub fn scroll_inventory(&mut self, delta: isize) {
        if delta < 0 {
            self.inventory_scroll = self.inventory_scroll.saturating_sub((-delta) as usize);
        } else {
            self.inventory_scroll = self.inventory_scroll.saturating_add(delta as usize);
        }
    }

    /// The browsing plan for the selected workflow, read straight from the cache
    /// (the single source of truth) — no redundant mirrored `plan` field that
    /// could desync from it (e.g. after `reap_worker` invalidates the entry).
    pub fn plan(&self) -> Option<&Result<PlanResult, String>> {
        self.plan_cache.get(&self.selected).map(|(_, r)| r)
    }

    /// Show the browsing plan for the selected workflow (offline planner, always
    /// safe — no I/O), reusing the cached result when the seed hasn't changed.
    pub fn refresh_plan(&mut self) {
        self.refresh_plan_impl(false);
    }

    /// `p` — explicitly recompute the plan, bypassing the cache.
    pub fn force_refresh_plan(&mut self) {
        self.refresh_plan_impl(true);
    }

    fn refresh_plan_impl(&mut self, force: bool) {
        self.infeasible_prompt = false;
        let Some(wf) = self.workflows.get(self.selected) else {
            return;
        };
        let seed = PlanSeed::from_view(&self.view.get());
        if !force {
            if let Some((cached_seed, _)) = self.plan_cache.get(&self.selected) {
                if *cached_seed == seed {
                    return;
                }
            }
        }
        let result = planner::plan(
            &wf.src,
            self.map.clone(),
            self.monsters.clone(),
            self.resources.clone(),
            &seed,
        )
        .map_err(|e| e.to_string());
        self.plan_cache.insert(self.selected, (seed, result));
    }

    // ─── periodic work (called every render loop) ─────────────────────────────

    pub fn tick(&mut self) {
        // Spinner animation, time-driven so it advances without input.
        if self.last_spin.elapsed() >= SPINNER_INTERVAL {
            self.spinner = self.spinner.wrapping_add(1);
            self.last_spin = Instant::now();
        }
        self.reap_worker();
        self.refresh_run_rows();
        // Tell the idle-poll thread whether it may fetch: only while Idle. During
        // a run the scheduler server-trues the view every action (§3.7), so
        // polling must be suppressed to avoid clobbering server truth.
        self.poll_idle_flag
            .store(self.run_state == RunState::Idle, Ordering::SeqCst);
    }

    /// Keep the run panel's derived state fresh: precompute the row labels once
    /// per published skeleton, and re-run the reducer only when the id-log or
    /// phase actually moved. `tick` runs before each draw, so the render path
    /// reads these without locking or recomputing.
    fn refresh_run_rows(&mut self) {
        // No active run → nothing to recompute. After `reap_worker` settles the
        // machine back to `Idle`, the published status/progress are final and
        // unchanging, yet without this guard every tick (~100ms, forever) would
        // re-lock both mutexes and recompute an unchanged key — wasteful and it
        // keeps the guards hot on the render thread. The persisted `run_cache`
        // (cleared only on the next `launch_run`) keeps the final rows on screen.
        // (CAND-7)
        if self.run_state == RunState::Idle {
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        let Some(skeleton) = session.skeleton.get() else {
            return;
        };
        // Recover from poison (a worker panic while holding a guard) instead
        // of crashing the render thread; the panic itself is surfaced in
        // `reap_worker`. (CAND-5)
        //
        // Hold BOTH guards across the key computation so the worker can't flip
        // `status` and `progress` between two separate reads and pair a stale
        // `phase` with a newer `log.len()` for one frame. Order is `status` then
        // `progress`, matching `reap_worker`/`run_worker` (which never hold both).
        let status_guard = lock_or_recover(&session.status);
        let log = lock_or_recover(&session.progress);
        let phase = status_guard.phase();
        let key = (log.len(), phase);
        // Rebuild rows + labels as ONE `RunRowCache` assignment when the key
        // moved (or the cache was cleared on relaunch / is pre-publish). Storing
        // rows, key, and labels in a single struct means they can never drift
        // relative to each other — the 3-field manual cache could, and a future
        // reset that forgot to clear all three would show stale rows or a stale
        // label count. Labels are immutable after publish but are reformatted
        // here alongside rows on key moves (cheap, and only when the id-log
        // actually moved — `reduce` already runs then).
        if self.run_cache.as_ref().is_none_or(|c| c.key != key) {
            self.run_cache = Some(RunRowCache {
                key,
                rows: reduce(skeleton, &log, phase),
                labels: crate::tui::skeleton::skeleton_labels(skeleton),
            });
        }
    }

    /// Detect the worker exiting and settle the run machine back to `Idle`
    /// (`Stopping → Idle` is the launch guard, §5.3).
    fn reap_worker(&mut self) {
        let finished = self.run_handle.as_ref().is_some_and(|h| h.is_finished());
        if !finished {
            return;
        }
        // `join` returns `Err` iff the worker panicked (a Lua panic, an
        // `unreachable!` in a host fn, an OOM mid-run, …) — the one terminal
        // outcome the worker can't publish itself. Recover any poison it left
        // behind instead of letting it crash every later `tick`. (CAND-5)
        let panicked = self
            .run_handle
            .take()
            .map(|h| h.join().is_err())
            .unwrap_or(false);
        // The worker publishes its own terminal status; surface a failure, or —
        // if it panicked before publishing — a stale-data notice.
        if let Some(session) = &self.session {
            let status = lock_or_recover(&session.status);
            if let RunStatus::Failed(msg) = &*status {
                self.error_popover = Some(msg.clone());
            } else if panicked {
                self.error_popover = Some("run worker panicked; run data may be stale".into());
            }
        }
        self.status_msg = None;
        self.run_state = RunState::Idle;
        // The run mutated the character (inventory/position/hp), so the cached
        // browsing plan for the selected workflow may now be stale. Drop the
        // cache entry so the next `launch_run` re-derives feasibility against
        // the live post-run state before gating (CAND-4).
        self.plan_cache.remove(&self.selected);
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
        // Re-derive the browsing plan against the current character state before
        // gating: a completed run invalidates the selected cache entry, and the
        // seed may have moved during the run. Without this the gate would read a
        // stale feasibility verdict (CAND-4).
        self.refresh_plan();
        // Gate on the browsing plan's feasibility unless overriding.
        let feasible = matches!(self.plan(), Some(Ok(p)) if p.feasible);
        if !feasible && !force {
            self.infeasible_prompt = true;
            self.status_msg = Some("infeasible — press R to override".into());
            return;
        }
        self.infeasible_prompt = false;

        let session = RunSession::new(self.view.clone());
        let initial = (*self.view.get()).clone();
        let handle = crate::tui::run_worker::spawn_tui_run(
            &self.character,
            wf.src,
            initial,
            self.map.clone(),
            self.monsters.clone(),
            self.resources.clone(),
            session.clone(),
        );
        match handle {
            Ok(handle) => {
                self.session = Some(session);
                self.run_handle = Some(handle);
                self.run_state = RunState::Running;
                self.status_msg = Some(format!("running {}…", wf.name));
                // A fresh session invalidates the previous run's derived state.
                // One assignment clears rows + key + labels together (CAND-9).
                self.run_cache = None;
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

/// The idle-poll thread body (§3.4). Owns `poll_driver` and, only while the app
/// is Idle, fetches the character snapshot every `POLL_INTERVAL` and publishes it
/// into the shared view the render thread reads each frame. Running entirely off
/// the render thread is what keeps a slow/hung request from ever freezing the UI.
/// Exits promptly once `stop` is set (quit).
fn poll_loop(driver: HttpDriver, view: SharedView, idle: Arc<AtomicBool>, stop: Arc<AtomicBool>) {
    // Wake often so `stop` is noticed promptly on quit; actually fetch only every
    // POLL_INTERVAL while Idle. First fetch lands ~POLL_INTERVAL after start.
    const WAKE: Duration = Duration::from_millis(100);
    let mut last_poll = Instant::now();
    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(WAKE);
        if stop.load(Ordering::SeqCst) {
            break;
        }
        if !idle.load(Ordering::SeqCst) || last_poll.elapsed() < POLL_INTERVAL {
            continue;
        }
        last_poll = Instant::now();
        // Blocking network fetch — safe here because this is a dedicated thread.
        if let Ok(v) = driver.fetch_character() {
            // Re-check Idle: a run may have launched during the fetch, in which
            // case the scheduler owns the view (§3.7) — don't clobber server truth.
            if idle.load(Ordering::SeqCst) && !stop.load(Ordering::SeqCst) {
                view.update(v);
            }
        }
        // Poll failures are dropped: best-effort, never block or touch the UI (§3.4).
    }
}
