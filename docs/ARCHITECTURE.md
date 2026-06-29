# Architecture

This document explains how the pieces of the Artifacts MMO client fit together: where user-authored game logic lives, and how it maps down through the Rust code to either an offline prediction or a real API call. Read this before making a targeted edit — it tells you which layer owns a given concern.

## The one-paragraph version

Bot behaviour is authored in **Fennel** as a _workflow_ — a tree of data (an AST), not opaque code. Because a workflow is data, the same source can be walked by **two interpreters**: `plan` (predict time/actions/cost _and_ feasibility by walking the control flow against a seed model state — no I/O; seed it from a live character for a per-character prediction) and `run` (real execution). The Rust side is split so that purity is provable at compile time: a sans-I/O **`core`** crate holds all game semantics (cooldowns, rate-limit buckets, the request/response state machine, pathfinding) with no sockets or clocks, and an **`artifacts`** crate adds the I/O runtime, the Fennel host, and the CLI on top.

## Layer map

```mermaid
flowchart TD
    subgraph authoring["AUTHORING — Fennel (where game logic is written)"]
        direction TB
        wf["<b>fennel/workflows/*.fnl</b><br/>a bot workflow — builds an AST value, never runs"]
        act["<b>fennel/lib/actions.fnl</b><br/>action vocabulary: each action defined ONCE as {:cost :sim :run}"]
        pred["<b>fennel/lib/predicates.fnl</b><br/>loop/branch predicates over model state"]
        interp["<b>fennel/lib/interp.fnl</b><br/>the two interpreters + AST constructors"]
    end

    bridge["<b>HOST BRIDGE — src/lua.rs</b><br/>embeds the Fennel compiler, registers the <code>host</code> table the Fennel layer calls (cooldown_cost, path_hops, gather_yield, …);<br/>in the run pass only, wires host.gather/move/fight/… to a live Character"]

    planner["<b>PLANNER — src/planner.rs</b><br/>runs the plan pass, returns a plain Rust struct — no I/O"]
    runtime["<b>RUNTIME — src/live.rs, scheduler, character</b><br/>Character (blocking facade) → Scheduler (async, owns Driver+Core) → SharedView;<br/>Driver (src/driver) does real HTTP"]

    core["<b>SANS-I/O BRAIN — core/ (crate artifacts-core; no tokio/reqwest/mlua)</b><br/>step.rs · machine.rs · cooldown.rs · state.rs · error.rs · map.rs · combat.rs · ident.rs"]

    authoring -->|"Fennel compiled to Lua, run in an mlua state"| bridge
    bridge -->|"offline: plan"| planner
    bridge -->|"live: run"| runtime
    planner --> core
    runtime --> core
```

## Where game logic lives: the Fennel layer

All _user-intended_ game logic is authored in `fennel/`. Nothing here executes on load — a workflow file evaluates to a **data tree** that one of the interpreters later walks.

### `fennel/workflows/*.fnl` — what the bot should do

A workflow is built from AST constructors (`seq`, `action`, `repeat_until`, `repeat_n`, `when_pred`). Example (`farm-copper.fnl`): travel to the copper tile, gather until inventory is full, travel to the bank, deposit everything. Loading the file yields the tree; it performs no I/O and makes no decisions by itself.

### `fennel/lib/actions.fnl` — the action vocabulary

**The single most important invariant in the codebase.** Each action (`gather`, `travel-to`, `deposit-item`, `craft`, `rest`, `fight`, …) is defined **exactly once** as a record with three fields:

| Field   | Used by | Meaning                                                 |
| ------- | ------- | ------------------------------------------------------- |
| `:cost` | plan    | Pure prediction of the cooldown this action will incur. |
| `:sim`  | plan    | Pure advance of the model state (e.g. add item to inv). |
| `:run`  | run     | Real execution, via a `host.*` function.                |

`def-action` asserts all three fields exist at load time. The `plan` pass uses `:cost` and `:sim` together (it sums `:cost` while threading state through `:sim`); `run` uses `:run`. Because both interpreters read the same table, the prediction and the real run **cannot describe different actions** — if `:sim` and `:run` diverged, plans would silently lie. Add a new action here, with all three facets, rather than special-casing it in an interpreter.

### `fennel/lib/predicates.fnl` — loop and branch conditions

Predicates (`is_full`, `hp_below`, `is_at`, `is_winnable`) read a _model state_ table. The key subtlety: the same predicate must work against both the pure model table (in plan) and the live character snapshot (in run). That is guaranteed by funnelling both through one builder — see "The state surface" below. They are defined once here and reused across workflows; the exported keys are deliberately underscore-cased (no `-`/`?`), because Fennel mangles a hyphen or `?` in a bare cross-file reference to a different symbol than the installed global, so a workflow referencing `inventory-full?` would silently fail to resolve.

### `fennel/lib/interp.fnl` — the two interpreters

Walks the workflow AST. The node types are `:seq`, `:action`, `:repeat-until`, `:repeat-n`, `:when`.

- **plan** — one offline walk that predicts both cost and feasibility from a seed model state. It accumulates `:seconds`, `:actions`, and `:bucket-cost` by calling each action's `:cost` and threading state through its `:sim`, while watching the evolving state for blockers (e.g. inventory carried past capacity, or a `repeat-until` that can't terminate) — any blocker flips `:feasible` to false and is recorded in `:blockers`. `repeat-until` loops are run against the model until the predicate flips, and the resolved iteration count is recorded under its `:label` (e.g. `gathers: 10`). Seeding from a live character (`PlanSeed::from_view`) makes all of this specific to where that character is right now. (An earlier split — `estimate` for cost, `simulate` as a deterministic wrapper that always said "feasible" — only duplicated this walk; it was collapsed into `plan`.) Combat is modelled: `:fight`'s `:cost`/`:sim` call a deterministic, crits-off simulator (`core::combat`) to predict turns, HP loss, and expected drops; a predicted loss is a hard blocker and probabilistic drop overflow is a soft `:warnings` entry.
- **run** — executes each action's `:run` against the real character, re-reading the live view (`host.view`) to evaluate `repeat-until` / `when` predicates between steps.

## How Fennel maps back to Rust

### The host bridge — `src/lua.rs`

`setup_lua` builds an mlua state: it loads the vendored Fennel compiler, evaluates the three lib files, and installs a `host` table of Rust functions the Fennel layer calls. Two distinct sets of host functions:

- **Always present (pure computation):** `cooldown_cost`, `path_hops`, `gather_yield`, `resource_level`, plus the combat trio `monster_stats`, `simulate_fight`, and `find_tile`. These back the `:cost`/`:sim` facets, so the plan pass needs no _character_. The computations are pure; the reference data some of them read (monster stats, map content) is fetched and cached — so a per-character plan _may_ touch the network to populate that cache, but the simulation itself does not.
- **Run-only (live):** `gather`, `move`, `fight`, `rest`, `deposit_item`, `deposit_all`, `view`. Registered only when a `Character` is supplied; otherwise replaced by a stub that errors loudly, so an accidental live call during planning fails fast instead of silently doing nothing. (`fight` also bails the workflow on a loss, since a loss respawns the character at 1 HP.)

`path_hops` uses the core A* pathfinder when a `GameMap` was loaded, and falls back to Manhattan distance otherwise — so a plan is still meaningful with no map.

### The state surface (why predicates don't drift)

`predicate_state` in `src/lua.rs` is the **single definition** of the hyphen-cased key surface predicates read (`st.x`, `st.hp`, `st.inventory-count`, …). Both the planner's seed state (`planner::build_state`) and the live `host.view` are built through this one helper, so a predicate sees the same shape whether it runs against the model or the real character. (Note the deliberate hyphen-case here: switching these keys to underscores would break `predicates.fnl`.)

### Offline path — `src/planner.rs`

`plan` sets up a Lua state with **no** character, builds a seed model state (`PlanSeed` — position, hp, inventory, the gather tile's level/resource), calls the Fennel `plan` pass, and marshals the Lua result back into a plain Rust struct (`PlanResult` — cost, `feasible`, and any `blockers`). This keeps `mlua` types out of callers like the CLI. `PlanSeed::default()` gives a generic best case; `PlanSeed::from_view` seeds the plan from a live character so the prediction reflects that character's current position, hp, and inventory.

### Live path — `src/live.rs` + the runtime modules

`live::run_workflow` wires the runtime together and runs a workflow's `run` pass against a real driver. The threading model matters:

- **`Character` (`src/character.rs`)** — the blocking facade the host fns hold. Each method (`move_to`, `gather`, …) sends an `Intent` to the scheduler over a channel and **blocks the script thread** waiting for the `Outcome`. This is what lets the Fennel `run` pass read as straight-line synchronous code.
- **`Scheduler` (`src/scheduler.rs`)** — runs on its own `std::thread`, owns the `Driver` and the `core::Core`. It loops: feed the intent to `Core`, ask `Core::next_step(now)` what to do, execute that `Step` on the driver, feed the response back via `Core::handle_response`. Transient codes (499/486/429) drive a `Retry`; a benign 490 is a no-op; success returns the `Outcome`.
- **`SharedView` (`src/view.rs`)** — an `Arc<RwLock<CharacterView>>` refreshed after every outcome. `host.view` reads it synchronously so predicates never block.
- **`Driver` (`src/driver/`)** — the I/O boundary trait. It owns the **authoritative clock**, so the scheduler reads `driver.current_time()` rather than `Instant::now()`. `mock` supplies a fake clock + canned responses for hermetic tests; `http` is reqwest + tokio against the live API.

### The sans-I/O brain — `core/`

`core` is a separate crate **on purpose**: its dependency tree is serde/thiserror only — no tokio, reqwest, or mlua — so the compiler _proves_ it does no I/O. It holds the parts of the game that are pure functions of (state, response, clock):

| Module | Responsibility |
| --- | --- |
| `step.rs` | The vocabulary: `Intent` (what to do), `Step` (what the driver does next), `Outcome`/`OutcomeKind`, `CharacterView`. |
| `machine.rs` | `Core` — `next_step(now)` decides sleep/request/done; `handle_response(status, body, now)` updates cooldown + buckets and classifies the result. **Pure: the caller supplies `now`.** |
| `cooldown.rs` | Per-action cooldown formulas (also exposed to Fennel via `host.cooldown_cost`). |
| `state.rs` | `CharacterState` (`busy_until`) and `RateLimitState` token buckets. |
| `error.rs` | Maps HTTP/response codes to retry/no-op/fatal classifications. |
| `map.rs` | Overworld A* pathfinding, the shared Manhattan distance, and nearest-content lookup (e.g. the nearest chicken/bank tile). |
| `combat.rs` | The deterministic (crits-off) fight simulator and the `MonsterView` reference-data model. Pure: `simulate(player, monster) -> FightPrediction`. |
| `ident.rs` | Opaque game-identity newtypes (`ContentType`, `Code`) so codes that should only ever be compared/looked up can't be confused with each other or string-manipulated. |

The only place `src/` reaches into `core` directly is the scheduler driving `Core`, plus `host.cooldown_cost`/`path_hops` re-exposing the pure formulas to Fennel.

## The CLI — `src/main.rs`

A thin dispatcher over the two paths above:

| Command | Path | Needs token | What it does |
| --- | --- | --- | --- |
| `artifacts plan <wf.fnl>` | offline | no | `planner::plan` against the default seed → prints feasibility/cost/loops. |
| `artifacts plan <wf.fnl> <character>` | offline + 2 fetches | yes | Fetches the character + map, seeds the plan from its live state, then `planner::plan`. |
| `artifacts run <wf.fnl> <character>` | live | yes | Fetches character + map, then `live::run_workflow`. |

## Adding things — where does it go?

- **A new bot behaviour** → a new file in `fennel/workflows/`, built from existing AST constructors. No Rust changes if it only uses existing actions.
- **A new action** (e.g. `withdraw-item`) → `fennel/lib/actions.fnl` with all three of `:cost`/`:sim`/`:run`, plus the backing `host.*` fn in `src/lua.rs` and a `Character` method if it needs the live runtime.
- **A new predicate** → `fennel/lib/predicates.fnl`; if it needs a new state field, add it to `predicate_state` in `src/lua.rs` so both the plan and run passes see it.
- **A new game rule** (cooldown formula, response code, pathfinding) → `core/`. Keep it pure; if you reach for a clock or a socket here, it belongs in `src/`.

## See also

- [`README.md`](../README.md) — quick start, build/test commands, and the authoritative external API references.
</content>

</invoke>
