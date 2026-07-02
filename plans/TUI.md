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
| **Header/cooldown data** | **Add `xp`/`max_xp`/`gold`/`cooldown`/`cooldown_expiration` to `CharacterView`** | The live `CharacterSchema` already returns them; one field add gives the header *and* the cooldown bar a single source that flows through `SharedView` both idle and running (§3.8). |
| Layout | Two-column + header + power bar | Most information-dense. |
| Visual style | Nerd-font icons + color | Per the brief; documented font prerequisite, with a `glyphs` module for a future ASCII fallback. |
| Stat density | Curated essentials in-pane; full block via zoom pop-over | Each pane is one widget rendered at compact or modal scale. |
| Interaction | Three modes: Normal / Interact / Focus | §5.2. |
| Extra panels in v1 | Cooldown countdown only | Cheap; derived from `cooldown_expiration` vs wall clock (§3.8); the rest are backlog. |

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
4. **Marshal** the skeleton to an owned `Vec<PlanStep>`, join the id-keyed loop
   counts from step 3, and **publish it to the UI** through the handoff cell
   (§3.4) — *before* the blocking run starts.
5. `run(wf)` — real execution; fires `host.progress(node.id)` per node.

Because steps 2–5 read the **same tables**, the ids the run reports are literally
the ids the skeleton recorded — alignment is by identity, with no determinism
argument. `plan` is safe to run in a character-equipped state: it calls only the
always-present `cost`/`sim` host fns, never run-only ones, and never mutates the
AST. The **browsing** plan panel (shown while selecting workflows, before `r`)
keeps using the cheap offline `planner::plan` — it needs only feasibility/cost
numbers, never id alignment, so the two are uncoupled.

**Why steps 1–5 are one thread, and why the skeleton must be marshaled and
handed off.** `mlua` is built **without the `send` feature** (see the comment in
`planner.rs`: "mlua::Error isn't Send"), so the `Lua` state is `!Send` and cannot
cross threads. The whole combined path therefore runs on the single run-worker
thread — exactly as `run_workflow` runs its Lua on its calling thread today. The
UI thread can't reach into Lua to read the skeleton lazily, and step 5 blocks the
worker for the entire run, so the worker must **convert the skeleton to an owned,
`Send` Rust value and publish it (step 4) before it blocks**. The UI reads that
published value to render the Run pane; until it appears the pane shows
`preparing run…`. `PlanStep` is fully owned (no `mlua` types), so it crosses the
boundary cleanly.

This is a new combined path in `live.rs` (distinct from today's separate
`planner::plan` and `live::run_workflow` entry points). Its signature takes the
initial view (to seed `plan`), the map/monsters, and the `RunSession` handles
(§3.4); it runs on the worker thread the TUI spawns at `r`.

### 3.2 Node ids + skeleton (Issue #2)

The skeleton is a **flat ordered `Vec<PlanStep>`**, each entry:

```
PlanStep {
  id, depth, kind, op, args,
  count:         Option<u32>,     // loop rows only
  loop_start_id: Option<NodeId>,  // loop rows: id of the body's first node
  guard_id:      Option<NodeId>,  // rows under a :when: that when's id (the skip key)
}
```

- Built by the Fennel `skeleton` walk (the AST lives in Lua; the walk is trivial
  there), marshaled to Rust. Rust formats the **display label** from `op`+`args`
  (e.g. `travel (2,0)`), keeping presentation in the TUI layer.

- **Which nodes become rows** (`kind`):
  - `:action` → one `Action` row.
  - `:repeat-until` / `:repeat-n` → one `Loop` header row, then its body **once**
    at `depth+1` (never expanded per-iteration).
  - `:when` → one `When` guard header row, then its body at `depth+1`; **every**
    body row (and their descendants) carries `guard_id = <this when's id>`. This
    handles a `when` with any number of body steps, not just one.
  - `:seq` → **no row** (purely structural). Its `:id` still fires at run time
    but maps to no `PlanStep`; the reducer's id→row map simply has no entry for
    it (such ids are used only for sequencing/segmentation, never rendered).

- **`loop_start_id`** is the id of the loop body's first node — the iteration
  boundary the reducer counts (§3.3). For `farm-chickens` that first node is the
  `:when`, which is exactly why `host.progress` must fire on `when` nodes too.

- **`count` (the `k/N` denominator) by loop kind:**
  - `:repeat-n` → `count = node.n`, filled directly by the skeleton walk (static,
    no plan needed).
  - `:repeat-until` → `count` is left `None` by the walk and **joined from the
    plan** afterward (step 4 of §3.1), which records resolved iterations **keyed
    by node id** (`loop-counts[id] = {label, count}`) — *not* by label, so nested
    or reused labels can't collide. A loop the plan never resolved stays `None`
    and renders `k/?`.

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

**The reducer is pure Rust** — `reduce(skeleton, id_log, status) -> Vec<RowState>`
— which is why the tricky logic is unit-testable (§7). `status` is the
`RunStatus` (§3.4): `Running`, `Done`, or `Failed`. It is what makes the terminal
frame correct — without it the last fired id would render as *active* forever:

- **Iteration k/N**: count of times a loop's `loop_start_id` has fired since the
  loop was entered. The denominator is the plan's `count` (an estimate — if the
  run diverges the header shows `>N` gracefully, never panics; an unresolved
  `None` count renders `k/?`).
- **Skipped**: a row with `guard_id = Some(w)` whose own id did **not** appear in
  the current iteration's id-subsequence even though its guard `w` *did* fire
  there. (If `w` did not fire, the row is `pending`/not-reached, not skipped.)
  Keying off `guard_id` — rather than "between the when-id and the next node" —
  is what makes this well-defined for a `when` with multiple body steps and for
  nested guards.
- **Active vs done at the tail**: while `status == Running`, the latest fired id's
  row is `active`. On `Done`, every row that was reached is `done` and the run is
  complete. On `Failed`, the last fired id's row is marked the death step (its id
  is the last entry in the log) and drives the failure pop-over.
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
| when guard | `` (branch) + the predicate name when one is given | dim accent |

### 3.4 Threading & render loop

- **Main thread**: terminal setup (`crossterm` raw mode + alternate screen), then
  loop: poll input (~100 ms timeout) → handle key → recompute derived view-state
  (drain id-log → reducer) → `terminal.draw(...)`. No blocking calls.
- **Run worker**: spawned on `r`; runs the combined path (§3.1). The TUI and the
  worker share one bundle of handles, created by the TUI before the spawn and
  moved (cloned `Arc`s) into the worker:

  ```rust
  struct RunSession {
      view:     SharedView,                       // stats/inventory/header/cooldown
      progress: ProgressLog,                      // Arc<Mutex<Vec<NodeId>>> — the id-log
      skeleton: Arc<OnceLock<Vec<PlanStep>>>,     // handoff: worker sets once (step 4), UI reads
      status:   Arc<Mutex<RunStatus>>,            // Preparing → Running → Done | Failed(String)
      abort:    Arc<AtomicBool>,                  // cancel flag (§3.5, #6)
  }
  ```

  The UI reads all of these each frame (cheap, non-blocking); it also keeps the
  worker's `JoinHandle` to detect exit (the `Stopping → Idle` transition, #6).
  `RunStatus` is what lets the render reflect terminal frames correctly: it is
  passed into the reducer (§3.3) so the last fired id reads as *active* while
  `Running` but as *done* on `Done`, and the death step is marked on
  `Failed` (the pop-over text is the `Failed(String)` payload). The worker sets
  `skeleton` (step 4) and advances `status` (`Preparing` at spawn → `Running`
  before step 5 → `Done`/`Failed` on exit).
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

### 3.8 Header & cooldown data: one field add to `CharacterView`

The header (name, level, **xp bar**, **gold**) and the **cooldown bar** need
fields the current `CharacterView` (`core/src/step.rs`) does not carry — it has
`name/x/y/hp/max_hp/level/inventory_max_items/inventory` plus combat stats, but
**no `xp`, `max_xp`, `gold`, or cooldown**. (The `xp`/`gold` on `FightResult`
are per-fight rewards, not character totals.) The live `CharacterSchema` already
returns all of these, so the fix is a single core change:

```rust
// added to CharacterView, all #[serde(default)] like the combat stats already are
pub xp: u32,
pub max_xp: u32,
pub gold: u32,
pub cooldown: u32,             // whole seconds remaining at fetch time
pub cooldown_expiration: String, // RFC3339; "" when idle
```

Field names match the API 1:1 (the existing `level`/`hp`/`max_hp`/… fields prove
`CharacterView` deserializes with no serde renames); `#[serde(default)]` keeps
every mock/fixture that omits them parsing unchanged. **Confirm the exact keys
against the `CharacterSchema` in the OpenAPI spec the README links
(`https://api.artifactsmmo.com/openapi.json`) before wiring** — because of
`#[serde(default)]`, a mis-typed key deserializes silently to `0`/`""` instead of
erroring, which would read as "0 gold, no cooldown" rather than a loud failure. Because `fetch_character`
and every action response both deserialize the same `CharacterView`, these fields
arrive through **one** path — so they populate `SharedView` identically whether
the run-state is Idle (poll) or Running (each `outcome.character`).

**The cooldown bar is then derived, not stored separately.** `busy_until` lives
inside `Core` on the scheduler thread and the scheduler discards `Outcome.cooldown`
(`scheduler.rs` keeps only `outcome.character`), so neither is reachable from the
TUI — but `cooldown_expiration` now rides in `SharedView`. The header parses it
once per frame and renders `max(0, expiration − wall_clock_now)` as the bar; an
empty/past timestamp renders empty. This is display-only, so using the wall clock
(not the driver clock) is fine. No new shared cell, no scheduler change.

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
│                           │      when need-rest?            │
│                           │         rest                    │
│                           │      ⠋ fight                       │
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
  feature; derived from `cooldown_expiration` vs the wall clock — §3.8, *not*
  `busy_until`, which is unreachable from the TUI).
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
| `core/src/step.rs` | Add `xp`/`max_xp`/`gold`/`cooldown`/`cooldown_expiration` to `CharacterView` (all `#[serde(default)]`) so the header + cooldown bar have a source that flows through `SharedView` idle and running (§3.8). No other core change. |
| `src/main.rs` | New `tui` subcommand: `artifacts tui <character>`; reuse `load_live_context`, **keep** the returned driver, hand off to the TUI module. |
| `src/tui/` (new) | `app.rs` (run-state machine, mode/focus state, worker + abort handles), `ui.rs` (layout + `render(area, scale)` per pane), `widgets/` (header, stats, inventory, workflows, plan, run), `reducer.rs` (pure `reduce(skeleton, id_log) -> Vec<RowState>`), `skeleton.rs` (PlanStep + label formatting), `glyphs.rs`, `theme.rs`, `event.rs`, `workflows.rs` (scan `fennel/workflows/`). |
| `fennel/lib/interp.fnl` | Add `number-nodes` (pre-order id stamp) and `skeleton` (flat structural walk) fns; `run-node` calls `(host.progress node.id)` at the top. **The id-keyed loop counts are recorded here, not in `planner.rs`** — `plan-node` already resolves a loop's iteration count, so it *also* writes `(when node.id (tset acc.loop-counts node.id {:label node.label :count iters}))`. The `(when node.id …)` guard is load-bearing: the browsing `plan` path never runs `number-nodes`, so `node.id` is `nil` there and an unguarded `(tset … nil …)` would throw. `repeat-n` records `node.n`; `repeat-until` records the resolved `iters`. The existing `assumptions[label]` map is left untouched. |
| `src/lua.rs` | New `host.progress(id)` appending to `Arc<Mutex<Vec<NodeId>>>` when a log is given; **always registered** (a no-op stub when the log is absent — `run-node` calls it unconditionally, so leaving it unregistered would be a nil-call). Add the 4th `progress: Option<ProgressLog>` param to `setup_lua`. |
| `src/planner.rs` | The id-keyed counts are *recorded* in Fennel (above) and *read* by the combined path (below); browsing `planner::plan` is unchanged (it never stamps ids, so `acc.loop-counts` is empty and ignored). One small change: make the seed builder (`build_state`) reusable (`pub(crate)`) so the combined path can seed `plan` on the **shared** state instead of `planner::plan`'s own `None`-character state. |
| `src/live.rs` | New **combined path** on the run-worker thread: build one state (`setup_lua(Some(character), map, monsters, Some(log))`), eval once, then `number-nodes` → `skeleton` → `plan(seed)` (seed via `PlanSeed::from_view` + the shared `build_state`) → **read `loop-counts` from the plan result and join it onto the marshaled `Vec<PlanStep>`** → **publish the skeleton and flip `status` to `Running`** (§3.4) → `run`. Takes the `RunSession` handles; on exit sets `status` to `Done`/`Failed`. Keep the existing `run_workflow` for CLI `run` (passing `progress: None`, and a never-set abort flag to `Scheduler::new`). |
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
   `host.progress` (always registered) at `run-node` top, the id-keyed
   `loop-counts` recording (`(when node.id …)`-guarded), and the `setup_lua`
   param. Prove ids fire and align under the combined path (Tier 2 scaffolding).
   *Riskiest unknown — first.*
2. **Skeleton + reducer in pure Rust.** `PlanStep` (incl. `guard_id`), the
   id-keyed `loop-counts` join, the `reduce(skeleton, id_log, status)` state
   machine (done/active/pending/skipped via `guard_id`, loop `k/N`, terminal
   frames from `RunStatus`). Tier 1 test is the gate. No UI yet.
3. **Static TUI shell.** The `CharacterView` field add (§3.8); layout, panes
   (`render(area, scale)`), modes, power bar, glyphs/theme, reading `SharedView`
   + a fetched character (header xp/gold/cooldown render here). No runs. Tier 3
   snapshot.
4. **Wire the run.** Combined path on a worker thread, the `RunSession` handoff
   (publish skeleton, drive `status`), driver-ownership split, abort flag +
   Stopping guard, live cursor, cooldown timer, failure pop-over. Tier 2 + Tier 4.
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

- XP / drops / gold **ticker** — a rate/delta feed (XP-per-hour, a drops log,
  gold gained this run) by diffing successive `SharedView` snapshots. The static
  header **values** (xp bar, gold) and the cooldown bar are in v1 (§3.8); only
  the time-series *deltas* are backlog.
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
