# Plan: a TUI for character state & workflows

A `ratatui` terminal UI that shows a single character's live state and lets you
plan and run workflows from one screen, with a truthful per-step progress
cursor. Launched as `artifacts tui <character>`.

This document is the agreed design from two requirements passes (a scoping pass
and an issue-by-issue gap-closing pass). It records **what** to build, **why**
each decision was made, and maps every decision onto concrete code in this repo.
Read `docs/ARCHITECTURE.md` first — this plan leans on the existing two-pass
(`plan` / `run`) design, the `SharedView` pattern, and the `MockDriver`.

---

## 1. Scope

**v1 is a full launcher**, not just a dashboard. It can:

- display one character's stats + inventory, refreshed live;
- list the workflows in `fennel/workflows/`;
- run the offline **`plan`** pass for a selected workflow and show feasibility,
  cost, and loop counts inline;
- execute a real **`run`** against the live API, showing a per-step progress
  cursor (which steps are done / active / pending / skipped) and a live cooldown
  timer;
- surface run failures (combat loss, network, infeasible plan) clearly.

**Single character.** The TUI targets one character named on the command line,
matching the existing CLI and the single-character scheduler ("Multi-character
scheduling is intentionally out of scope" — `scheduler.rs`). Multi-character is
backlog (§10).

---

## 2. Decisions

| Area | Decision | Why |
| --- | --- | --- |
| Scope | Full launcher: plan **and** live run | The point is to trigger workflows from the TUI, not just watch. |
| Characters | Single, named on CLI | Matches the single-character scheduler; avoids concurrent-thread complexity. |
| **#1 Node identity** | **Single shared Lua state at run launch** | One eval → one AST; `plan`-for-skeleton and `run` read the *same* tables, so ids align by identity. Dissolves cross-eval determinism risk entirely. |
| **#2 Skeleton** | **Flat list + indent + group metadata**; counts keyed by **node id** | Linear list maps to a ratatui list and the cursor sweep; loops shown once with `×N` (not expanded per-iteration); id-keyed counts avoid label collisions. |
| **#3 Cursor** | **Ordered id log → pure Rust reducer → 4 glyph states** | A single-slot cursor misses microsecond-apart fires (when-skip / loop boundary). An append-only id log + pure reducer is correct *and* unit-testable. |
| **#4 Driver ownership** | **Separate poll driver + fresh per-run driver** | `execute(&mut self)` needs exclusive ownership; HTTP clock is stateless; polling is suppressed during runs, so two instances never contend. |
| **#5 `setup_lua`** | **Add a 4th `Option<ProgressLog>` param** | Mirrors the existing `Option<Character>` + run-only-stub idiom; `planner`/CLI-run pass `None`. |
| **#6 Cancel** | **Idle→Running→Stopping run machine; cancel in Run pane (Interact)** | Hard-kill UX, but `Stopping` guards the next launch so two schedulers never drive one character. Cancel binding stays modal-consistent. |
| **#7 Tests** | **All 4 tiers, but few & multi-case** | Keep the suite high-value; the pure reducer is the primary gate. |
| Plan→run gating | One-key run, plan shown inline; infeasible needs override | Fast loop; `plan` is always safe (no I/O). |
| Stat/inventory source | `SharedView` while running, poll `GET /characters` while idle | `SharedView` is server-trued every action (§3.7), so no mid-run drift; no reconcile in v1. |
| Layout | Two-column + header + power bar | Most information-dense. |
| Visual style | Nerd-font icons + color | Per the brief; documented font prerequisite, with a `glyphs` module for a future ASCII fallback. |
| Stat density | Curated essentials in-pane; full block via zoom pop-over | Each pane is one widget rendered at compact or modal scale. |
| Interaction | Three modes: Normal / Interact / Focus | §5.2. |
| Extra panels in v1 | Cooldown countdown only | Cheap, time-derived; the rest are backlog. |

---

## 3. Architecture

```
artifacts tui <character>
        │ startup: load_live_context() → (poll_driver, view, map, monsters)
        ▼
  ┌───────────────────────── TUI thread (main) ─────────────────────────┐
  │ ratatui render loop + crossterm input poll (non-blocking, ~100ms)   │
  │  reads:  SharedView (stats/inv)   progress id-log (run cursor)       │
  │  idle:   poll_driver.fetch_character() every ~3s (suppressed in run) │
  │  run machine: Idle → Running → Stopping → Idle                       │
  └─────────────────────────────────────────────────────────────────────┘
        │ on 'r': spawn worker with a FRESH HttpDriver::from_env
        ▼
  ┌──────── run worker thread ─────────┐        ┌──── scheduler thread ────┐
  │ one Lua state (character+progress) │◀──────▶│ owns run_driver + Core   │
  │ eval wf ONCE → number-nodes →      │        │ updates SharedView       │
  │ plan(wf,seed)=skeleton → run(wf)   │        │ checks abort flag (#6)   │
  │ run fires host.progress(id) → log  │        └──────────────────────────┘
  └────────────────────────────────────┘
```

The TUI thread **never blocks**: it reads cheap shared cells (`SharedView`, the
id-log) and polls input with a timeout. The blocking `run` pass and scheduler
keep their existing threading; the TUI observes their shared state.

### 3.1 Run launch: one shared state (Issue #1)

At `r`, build **one** character-equipped Lua state and `eval_fennel` the workflow
**once** → a single AST table. Then on that same table:

1. `number-nodes(wf)` — a pre-order walk stamping a unique `:id` per node
   (visit-order ids).
2. `skeleton(wf)` — a structural walk returning the flat skeleton (§3.2).
3. `plan(wf, seed)` — seeded from the live view, to resolve loop counts and
   feasibility.
4. `run(wf)` — real execution; fires `host.progress(node.id)` per node.

Because steps 2–4 read the **same tables**, the ids the run reports are literally
the ids the skeleton recorded — alignment is by identity, with no determinism
argument. `plan` is safe to run in a character-equipped state: it calls only the
always-present `cost`/`sim` host fns, never run-only ones, and never mutates the
AST. The **browsing** plan panel (shown while selecting workflows, before `r`)
keeps using the cheap offline `planner::plan` — it needs only feasibility/cost
numbers, never id alignment, so the two are uncoupled.

This is a new combined path in `live.rs` (distinct from today's separate
`planner::plan` and `live::run_workflow` entry points).

### 3.2 Node ids + skeleton (Issue #2)

The skeleton is a **flat ordered `Vec<PlanStep>`**, each entry:

```
PlanStep { id, depth, kind, op, args, count: Option<u32>, loop_start_id: Option<NodeId> }
```

- Built by the Fennel `skeleton` walk (the AST lives in Lua; the walk is trivial
  there), marshaled to Rust. Rust formats the **display label** from `op`+`args`
  (e.g. `travel (2,0)`), keeping presentation in the TUI layer.
- Loops emit a **header** entry (`kind = loop`) carrying `count` and
  `loop_start_id` (the id of the loop body's first node — needed for iteration
  counting), followed by their body entries **once**, at `depth+1`. Never
  expanded per-iteration.
- `count` is joined from the plan, which records resolved iterations **keyed by
  node id** (`loop-counts[id] = {label, count}`) — *not* by label, so nested or
  reused labels can't collide.

### 3.3 The progress log + cursor reducer (Issue #3)

**`host.progress` fires at the top of `run-node`** (before the `match`), so
*every* node reports on entry — including `:when` and `:repeat-until` nodes,
regardless of predicate. That is what distinguishes "reached a `when` and skipped
it" from "never reached it," and makes the loop body's first node a reliable
iteration boundary even when it is a `when`.

**The cell is an append-only ordered log**, `Arc<Mutex<Vec<NodeId>>>`, not a
single slot. A single slot would miss microsecond-apart fires: when
`farm-chickens`'s `need-rest` predicate is false, the when-node id and the
sibling `fight` id fire back-to-back with **no blocking action between them**, so
a 100 ms poll of a latest-id slot would lose the when-id and break skip/boundary
detection. The run appends each id (a `usize` — trivially cheap, nothing like a
structured event channel); the TUI **drains new entries each frame** into a pure
reducer. The full log also gives the exact death point for the failure pop-over.

**The reducer is pure Rust** — `reduce(skeleton, id_log) -> Vec<RowState>` — which
is why the tricky logic is unit-testable (§7):

- **Iteration k/N**: count of times a loop's `loop_start_id` has fired since the
  loop was entered. The denominator is the plan's `count` (an estimate — if the
  run diverges the header shows `>N` gracefully, never panics).
- **Skipped**: a `when`-body row whose id did not appear between its parent
  when-id and the next node, on the current iteration.
- Loop-body rows are shown **once**, reflecting the **current iteration's** state
  (e.g. the `rest` row toggles skipped/active as the loop turns), while the
  header counts up.

**Row states render as glyphs** (centralized in a `glyphs` module; words only in
the zoomed detail view):

| State | Glyph | Color |
| --- | --- | --- |
| done | `` | green |
| active | `⠋⠙⠹…` (animated braille) | accent |
| pending | `` | dim grey |
| skipped | `` | muted yellow |
| loop header | `` + `k/N` | accent |

### 3.4 Threading & render loop

- **Main thread**: terminal setup (`crossterm` raw mode + alternate screen), then
  loop: poll input (~100 ms timeout) → handle key → recompute derived view-state
  (drain id-log → reducer) → `terminal.draw(...)`. No blocking calls.
- **Run worker**: spawned on `r`; runs the combined path (§3.1). The TUI keeps
  clones of `SharedView` + the id-log to read while it runs, plus the worker's
  `JoinHandle`/done-flag and the abort flag (§3.5, #6).
- **Idle polling**: a ~3 s timer issues `poll_driver.fetch_character()` when
  run-state is `Idle`; **suppressed** during a run (`SharedView` is already
  server-trued).

### 3.5 Driver ownership (Issue #4)

`load_live_context` already returns an `HttpDriver` (today discarded in `main` as
`_driver`). The TUI **keeps it** (`poll_driver`) for the initial fetch + idle
polls. Each run builds a **fresh** `HttpDriver::from_env` (`run_driver`, cheap)
moved into the combined path. They never run concurrently (polling is suppressed
during runs), so there is no API/rate-limit contention — and since
`execute(&mut self)` demands exclusive ownership and the HTTP clock is
stateless, two instances are the natural, safe shape.

### 3.6 `setup_lua` change (Issue #5)

`setup_lua(character: Option<Character>, map, monsters, progress: Option<ProgressLog>)`.
Register a real `host.progress` (appends to the log) when `progress` is `Some`, a
no-op stub when `None`. Only the **`run`** pass calls it; the `number-nodes`,
`skeleton`, and `plan` walks are pure Fennel needing no host. Callers: TUI passes
`Some`; `planner.rs` and CLI `artifacts run` pass `None` (behavior unchanged).

### 3.7 Why `SharedView` can't silently drift

`scheduler.rs` does `self.view.update(outcome.character.clone())` on every
completed action, and `outcome.character` is parsed straight from the API
response body — Artifacts' action endpoints return the **full updated character
schema**. So `SharedView` is replaced wholesale with server truth after each
action; it is not a local prediction. The only non-update is the benign `490`
no-op. Hence: read it directly while running, no mid-run reconcile in v1.

---

## 4. UI design

### 4.1 Layout

```
┌──────────────────────────────────────────────────────────────┐
│  kael   Lv12   xp ▓▓▓▓▓░░ 3.4k/5k    1,240   cd ▓▓▓░░ 1.4s │  header
├───────────────────────────┬──────────────────────────────────┤
│ STATS                     │ WORKFLOWS                         │
│  120/120   ⚔ 45   ⛨ 20  │ ▸ farm-copper                     │
│  (2,0)   crit 5%  hst 3   │   farm-chickens                   │
├───────────────────────────┼──────────────────────────────────┤
│ INVENTORY        12/100   │ PLAN  farm-copper  feasible ~142s │
│   copper   x12            │ RUN                               │
│   ash      x4             │   travel (target)               │
│   …                       │   fights 7/12                   │
│                           │      rest (when needed)         │
│                           │   ⠋ fight                         │
│                           │   travel (bank)                 │
│                           │   deposit-all                   │
├───────────────────────────┴──────────────────────────────────┤
│ NORMAL  ←→↑↓ focus   ⏎ interact   z zoom   q quit            │  power bar
└──────────────────────────────────────────────────────────────┘
```

Panes: **header**, **stats**, **inventory**, **workflows**, **plan**, **run**,
**power bar**. All panes have borders; the focused pane's border is colored
(Normal mode). While a run is active, the power bar hints that **stop** lives in
the Run pane (the binding itself is Run-pane/Interact — #6).

### 4.2 Widget model: reusable, re-scalable panes

Each pane is a self-contained widget rendered at **two scales**: *compact* (its
grid cell) and *modal* (a centered pop-over — the zoom/Focus view). The full
per-element combat stat block is the stats widget at modal scale, not a separate
screen. Design each pane as `render(area, scale)` from the start.

### 4.3 Visual style

- **Nerd-font icons** (documented prerequisite), centralized in `glyphs` so a
  future ASCII-fallback theme is one module swap.
- **Color**: focused-pane border accent, feasibility green/red, cooldown bar,
  active-step accent, failures red. One small `theme` module.

### 4.4 Panels

- **Header** — name, level, xp bar, gold, live cooldown bar (the one extra v1
  feature; time-derived from `busy_until`).
- **Stats** — compact: hp, position, primary atk/def, crit, haste. Modal: full
  per-element attack/dmg/resist, initiative.
- **Inventory** — occupied slots (code + quantity), `used/max` header; modal
  scrolls the full list.
- **Workflows** — selectable list scanned from `fennel/workflows/*.fnl`.
- **Plan** — `PlanResult` for the selected workflow (browsing: offline planner).
- **Run** — the flat skeleton with per-row glyph states driven by the reducer.

---

## 5. Interaction model

### 5.1 Run lifecycle

1. Focus **Workflows**, Interact, select a workflow.
2. Selection (or `p`) runs the offline `planner::plan` → fills the **Plan** panel.
   Always safe (no I/O).
3. Press **`r`** to launch (plan already shown inline). If the plan is
   **infeasible** (e.g. predicted combat loss), the first `r` does not run; the
   power bar prompts `infeasible — press R to override`; capital `R` forces it.
   Launch is allowed only from run-state `Idle` (#6).
4. During the run: the **Run** panel shows the live cursor (reducer over the
   id-log); **Stats**/**Inventory** update from `SharedView`; the cooldown bar
   ticks.
5. On completion all rows go done. On **failure** (combat loss bails the run via
   `host.fight`; or network error), a **blocking pop-over** shows the error and
   the step it died on (last id in the log), dismissed with `Esc`.
6. **Cancel** — focus the Run pane, Interact, `x` (#6).

### 5.2 Modes

- **Normal** — arrow keys move focus between panes (active border colored); `⏎`
  enters Interact; `z` zooms to Focus; `q` quits.
- **Interact** — operate the focused pane: Workflows `↑/↓` select, `p` plan,
  `r`/`R` run; Run pane `x` stop; Inventory/Stats `↑/↓` scroll. `q`/`Esc` →
  Normal.
- **Focus** — the focused pane as a modal pop-over (zoom); pane interactions
  still work; `q`/`Esc` closes.

The power bar always lists the bindings valid in the current mode/run-state.

### 5.3 Cancellation + run-state machine (Issue #6)

Run-state: **Idle → Running → Stopping → Idle**.

- Add an `Arc<AtomicBool>` abort flag the scheduler checks at the top of its step
  loop in `process()`.
- `x` (Run pane, Interact) while Running: set abort, flip to **Stopping**, detach
  input from the run. The scheduler bails the current intent and exits, dropping
  the reply sender → the worker's `blocking_recv` (`character.rs:23`) errors →
  the Lua run pass unwinds → the worker thread ends. Latency: ≤ 1 in-flight
  action (a cooldown sleep is observed after it returns; interrupting the sleep
  is backlog).
- The TUI watches the worker's `JoinHandle`/done-flag; when the orphan has
  exited, **Stopping → Idle**. A new run can launch **only** from Idle — this is
  the guard that prevents two schedulers driving the same character.
- This is faithful "hard kill" (no graceful outcome drain; server-side mid-action
  residue accepted); `Stopping` exists solely as the launch guard.

---

## 6. Concrete code changes

| Where | Change |
| --- | --- |
| `Cargo.toml` (artifacts crate) | Add `ratatui` + `crossterm`. |
| `src/main.rs` | New `tui` subcommand: `artifacts tui <character>`; reuse `load_live_context`, **keep** the returned driver, hand off to the TUI module. |
| `src/tui/` (new) | `app.rs` (run-state machine, mode/focus state, worker + abort handles), `ui.rs` (layout + `render(area, scale)` per pane), `widgets/` (header, stats, inventory, workflows, plan, run), `reducer.rs` (pure `reduce(skeleton, id_log) -> Vec<RowState>`), `skeleton.rs` (PlanStep + label formatting), `glyphs.rs`, `theme.rs`, `event.rs`, `workflows.rs` (scan `fennel/workflows/`). |
| `fennel/lib/interp.fnl` | Add `number-nodes` (pre-order id stamp) and `skeleton` (flat structural walk) fns; `run-node` calls `(host.progress node.id)` at the top. |
| `src/lua.rs` | New run-only `host.progress(id)` appending to `Arc<Mutex<Vec<NodeId>>>`; no-op stub when absent. Add the 4th `progress: Option<ProgressLog>` param to `setup_lua`. |
| `src/planner.rs` | Record resolved loop iterations **keyed by node id** (`loop-counts[id] = {label, count}`) alongside the existing label map; expose to the skeleton join. (Browsing `plan` otherwise unchanged.) |
| `src/live.rs` | New **combined path**: build one state, eval once, `number-nodes` → `skeleton` → `plan(seed)` → `run`, exposing `SharedView`, the id-log, and the abort flag to the caller; run on a worker thread. Keep the existing `run_workflow` for CLI `run` (passing `progress: None`). |
| `src/scheduler.rs` | Accept the abort flag; check it at the top of the `process()` step loop and bail/exit when set. |

---

## 7. Testing strategy (Issue #7)

All four tiers, but **few and multi-case** — high-value tests that exercise many
cases each, not a sprawl of tiny ones. The pure reducer is the primary gate.

- **Tier 1 — reducer unit (pure, the gate).** *One* table-driven test sweeping:
  linear progression; loop `iter k/N`; **when-skip**; back-to-back fires in one
  drain; **plan/reality divergence** (run exceeds predicted count → `>N`, no
  panic). No Lua, no network.
- **Tier 2 — hermetic alignment (MockDriver, offline).** *One* test running real
  Fennel through the combined path against `MockDriver` for both `farm-copper`
  (linear + loop) and `farm-chickens` (the `when_pred` branch), asserting every
  fired id exists in the skeleton and the sequence is coherent. Guards Option A's
  identity alignment and `host.progress` wiring.
- **Tier 3 — `TestBackend` snapshots.** Snapshot only the **highest-value**
  widget(s) — the run panel for a known `RowState` vector — to catch layout
  regressions without a terminal.
- **Tier 4 — live smoke (opt-in, gated by the env token).** *One* end-to-end run
  of `farm-copper` + `farm-chickens` through the combined path, behind the
  existing live-test gate.

---

## 8. Implementation roadmap

Sequenced to retire the riskiest unknown first; each step is independently
verifiable.

1. **Fennel + numbering + host.progress.** `number-nodes`, `skeleton`,
   `host.progress` at `run-node` top; the `setup_lua` param. Prove ids fire and
   align under the combined path (Tier 2 scaffolding). *Riskiest unknown — first.*
2. **Skeleton + reducer in pure Rust.** `PlanStep`, the id-keyed `loop-counts`
   join, the `reduce` state machine (done/active/pending/skipped, loop `k/N`).
   Tier 1 test is the gate. No UI yet.
3. **Static TUI shell.** Layout, panes (`render(area, scale)`), modes, power bar,
   glyphs/theme, reading `SharedView` + a fetched character. No runs. Tier 3
   snapshot.
4. **Wire the run.** Combined path on a worker thread, driver-ownership split,
   abort flag + Stopping guard, live cursor, cooldown timer, failure pop-over.
   Tier 2 + Tier 4.
5. **Polish.** Zoom pop-overs, infeasible override, nerd-font/theme refinements.

Steps 1–2 are the whole ballgame; if they're solid the rest is conventional
ratatui work.

---

## 9. Risks

- **`core::machine::Progress` name clash** — `scheduler.rs` already imports a
  `Progress` from `core::machine`. Name the TUI cursor types distinctly
  (`ProgressLog`, `RowState`, `RunCursor`) to avoid confusion.
- **Stopping latency** — a cancel during a long cooldown sleep takes ≤ 1 action;
  surface "stopping…" so it isn't read as a hang. Abortable sleep is backlog.
- **Nerd-font dependency** — documented; `glyphs` keeps an ASCII fallback a
  one-module change.
- **Fennel layer changes** stay invisible to workflow authors (ids auto-stamped,
  `host.progress` a no-op without a log).
- **Test-suite growth** — honor the "few, multi-case" rule (§7); the existing
  suite is already trending large.

---

## 10. Out of scope for v1 (backlog)

- XP / drops / gold ticker (diff successive `SharedView` snapshots).
- Mini overworld map from the fetched `GameMap`.
- Bank contents panel (needs a new `GET /my/bank/items` driver method).
- Mid-run reconcile / drift alarm.
- **Abortable cooldown sleep** for near-instant cancel.
- Graceful "stop at boundary" (drain outcomes) instead of hard kill.
- Multi-character roster with concurrent runs (one scheduler per character).
- ASCII-only theme toggle.
- Tie-in with the `PROGRESS_BARS` spike — if it lands a structured event channel,
  the run panel could upgrade from a cursor to a rich live action log without
  changing the rest of the TUI.
```
