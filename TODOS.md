# Spike: Achievement workflows
Context: There are achievements in this game. Can we generate workflows that can work through achievement lists?
Goal: Automation system around achievements, which is fully scripted in fennel layer
Deliverable: `plans/ACHIEVEMENTS.md`

# TUI: Workflow Editing
Context: Once we have a TUI, one thing we'll want to be able to do is add, edit, or delete workflows while the TUI is active. Ideally we can launch $EDITOR for this, and the file names are appropriate enough to trigger built in language server providers. We'll also want to ensure the LSPs can access any host information / autocomplete the built in stubs and helpers we provide
Goal: Manage workflows within the TUI using preferred editor
Deliverable: PR I can review on github

# Spike, TUI: v2 stretch goals
Context: The following items didn't make it into the v1 UI. We should pick apart some items to address in a v2
- Display what tiles have monsters or resources on the map, identify what they drop.
- Display what crafters are on the map, what they can craft
- Display what merchanges are on the map, what they will buy/sell.
- Show the character's current task + its rewards (from `/tasks/list`). Task
  reference data was deliberately NOT cached for sim accuracy (the assigned task
  and its rewards are random server state, not fixable with reference data), so
  it's a display-only concern that belongs here — see the "Recipe / GE / task
  reference data" entry below for the reasoning.
- XP / drops / gold **ticker** — a rate/delta feed (XP-per-hour, a drops log,
  gold gained this run) by diffing successive `SharedView` snapshots. The static
  header **values** (xp bar, gold) and the cooldown bar are in v1 (§3.8); only
  the time-series *deltas* are backlog.
- Mini overworld map from the fetched `GameMap`.
- Bank contents panel (needs a new `GET /my/bank/items` driver method).
- Mid-run reconcile / drift alarm.
- **Abortable cooldown sleep** for near-instant cancel.
- Graceful "stop at boundary" (drain outcomes) instead of hard kill.
Goal: Identify more TUI ergonomics or features worth adding and how we might do so
Deliverable: `plans/TUI_v2.md`

# Map cache staleness vs. ephemeral event content
Context: Surfaced while trying to run a live NPC-buy workflow. The overworld map uses the same 24h TTL disk cache as monster data (`TTL` in `src/data.rs`), on the assumption the map is static. It isn't: the map carries **ephemeral event content**. `fish_merchant` (a gold merchant selling `gudgeon @ 10`) was present when we first observed it, then left the world as a timed event before we could buy — but a cache written during its visit keeps reporting it (and, conversely, a cache written while it's absent misses it). Result: `host.find_tile` resolves phantom merchants or misses live ones, so a planned/queued purchase workflow silently targets a tile that no longer has content. Refetching every launch is too costly (5 paged `/maps` calls). Note this is distinct from the `(x,y)` collision dedup already fixed in `GameMap::insert` — that fix can't help when the content simply isn't in the current feed.
Goal: A cheap/intelligent freshness signal to bust *just the map* cache when event content changes — e.g. poll an events endpoint / event log (https://docs.artifactsmmo.com/concepts/events/), key the map cache on the active-events set, or give map content a much shorter TTL than static reference data.
Deliverable: PR I can review on github

# Cleanups

Triaged 2026-07-14 from the llu::utr review findings. Priority 1 is
simplicity (simple code, no over-engineering, reasonable assumptions);
priority 2 is correctness (bugs aren't allowed for the sake of simplicity).
Everything else comes way after those two — hardening, diagnostics polish,
and defenses against our own trusted local files were dropped (list at the
bottom). Deliverable for every task: PR I can review on github.

## Priority 1 — Simplicity

### Task: One schema validator for load, schema, and the TUI form
Context: `workflow::load` validates via Fennel `validate_schema`, but
`workflow::schema` marshals the params table manually with no validation
(silently dropping malformed options/defaults), and the TUI form re-validates
values in Rust — three implementations that already disagree (e.g. required
empty strings).
Scope: `workflow::schema` calls the same `validate_schema` as `load`;
the TUI form delegates value checking/coercion to the one Fennel coercer.
Delete the Rust duplicates. Extend the existing workflow-params scenario —
no new parity-corpus machinery.

### Task: One exclusive TUI overlay state
Context: Tooltip, palette, form, and error popover are independent fields; a
tooltip can render over an active form while input goes to the hidden modal,
and Ctrl-C is checked after modal handlers so it can be swallowed or inserted
as a literal `c`.
Scope: Fold the overlays into a single enum (at most one active; opening one
closes the others) and route Ctrl-C before modal-local input. This is a net
state-machine simplification, not new machinery.

## Priority 2 — Correctness

### Task: Simulated consumption must reject shortages (inventory and bank)
Context: `inv-remove` in `fennel/lib/actions.fnl` silently clamps a short
item at zero, so craft/deposit/recycle/use/delete/give/sell/GE-fill/task-trade
plan as feasible without the required items — `tests/all_intents.rs` currently
blesses crafting daggers from an empty inventory. The bank sims have the same
hole: `withdraw-item`/`withdraw-gold` fabricate goods without checking or
decrementing modeled bank holdings, so repeated withdrawals double-spend.
Scope: Apply the `--pending-blocker` pattern that `gold-spend` (directly below
`inv-remove`) already uses: a shortage records a precise blocker naming
required vs available, dependent outputs/rewards are withheld, and bank
holdings are a modeled state component that withdrawals consume and deposits
credit. Fix the tests that bless the old behavior.

### Task: Acquire chunking deposits only what the model holds, and the qty contract is honest
Context: `emit-chunked-loop` deposits `load` units of the target as if every
gained slot contained it — byproducts and multi-quantity drops break that
(currently hidden by the inv-remove clamp above). Chunking also silently
weakens the documented "ends with ≥ qty in inventory" contract: requesting 25
with capacity 10 can report success holding 5, the rest banked.
Scope: Derive deposit quantities from what the model actually gained; a
top-level request that cannot fit in inventory is an explicit error or an
explicit bank-target mode with its own documented postcondition — never
silent partial success. Craft-input chunking uses the explicit bank mode.

### Task: NPC buy/sell use catalog prices and gate on gold
Context: `fennel/workflows/buy-from-merchant.fnl` requires a user-supplied
price hint even though `NpcItemData` carries the authoritative fixed prices,
and `npc-buy`/`npc-sell` sims accept hints that live execution discards —
wrong hints make plan gold deltas diverge from the server. `acquire.fnl` also
compares purchases only against the configured budget, not projected/current
gold, so a budget above current gold emits a known-unaffordable buy.
Scope: Resolve price from the catalog (fail before dispatch on unknown
NPC/item pair, unsupported direction, or non-gold listing), delete the price
params and the stale compatibility workflow, and cap generated purchases by
min(budget, projected gold).

### Task: Validate numeric domains at the boundaries
Context: `:number` coercion accepts zero, negative, fractional, NaN, and
infinite values for acquisition quantities, budgets, prices, and `repeat-n`
counts — yielding empty or nonterminating plans, or late u32 conversion
failures after planning succeeded. Recipe data is trusted the same way:
zero output quantity divides by zero, a missing workshop reaches
`find_tile(:workshop, nil)`, duplicate input codes are acquired independently
but consumed cumulatively.
Scope: Reject non-finite / out-of-domain values where positive integers are
required — at param coercion for user input, at generation time for recipe
fields — with parameter/item-specific messages. Sum duplicate recipe inputs.
Extend the existing workflow-parameter scenario; no validation framework.

### Task: Campaign loop counts executed chunks and fails on no progress
Context: `run_until_done` spends its iteration cap on build probes as well as
executed chunks, so a campaign needing exactly N chunks executes chunk N then
reports exhaustion without the final build that would observe nil (the current
test codifies this). Separately, a feasible zero-action AST burns all 100
default iterations doing nothing.
Scope: `--max-iterations N` caps executed chunks, with one final build-only
completion probe allowed; a non-nil build producing zero actions fails
immediately with a "campaign made no progress" error. Skip fancy
context-equivalence detection — the zero-action check covers the observed
failure mode.

### Task: Acquisition falls back past sources that are not on the map
Context: `SourceIndex` ranks global `/resources` and `/monsters` data while
only the overworld map is loaded, so it can pick underground/interior content,
then fail at `find_tile` without trying later candidates.
Scope: When the top-ranked source has no tile on the loaded map, try the next
candidate; the final no-source error lists what was tried and why. Full
layer/transition-graph and path-cost ranking is out of scope (over-engineering
for one overworld map).

### Task: Fix the three TUI run-lifecycle races
Context (all confirmed in code): (a) `tick` refreshes run rows before
`reap_worker`, so a terminal status published between them leaves the final
row spinning forever — after Idle the refresh guard skips recompute; (b) the
idle-poll flag flips only at the end of `tick`, so a poll racing a launch can
publish an idle snapshot into the `SharedView` the scheduler now owns; (c)
`run_worker` converts any error to Done whenever the abort flag is set, so a
genuine failure racing a cancellation disappears, and completed vs cancelled
are indistinguishable.
Scope: Reap takes one final row snapshot before settling to Idle; launch
flips the poll flag before spawning (restoring it on failed spawn); outcomes
are typed Completed/Cancelled/Failed and only a typed cancellation is
suppressed. Quit during a run routes through the same cancellation path.

### Task: TUI plan cache invalidates the launched workflow and refreshes bank
Context: `reap_worker` removes `plan_cache[selected]` — whichever workflow is
selected at reap time, not the one that ran (src/tui/app.rs:430). Bank is
fetched once at startup, so bank-aware plans go stale after any bank-mutating
run.
Scope: Remember which workflow launched and invalidate that entry; refetch
bank after a run completes (visible failure if the refetch fails). The full
versioned-live-context / run-provenance design is out of scope.

### Task: Surface workflow scan failures instead of an empty list
Context: `App::new` does `workflows::scan(...).unwrap_or_default()`, so
directory-read or Lua setup failures render as "no workflows found" while the
CLI reports the real error.
Scope: Distinguish a legitimately empty directory (intentional empty-state
message) from a scan error (show the error and path in the panel).

### Task: Make execution fixtures strict
Context: `tests/common::char_json` omits API-required skill fields, letting a
nominally level-zero character receive canned successful gathers, and
`MockDriver` fabricates success for unscripted requests — missing or duplicate
actions pass unless a test happens to inspect logs.
Scope: Complete the shared character fixture (scenarios set relevant levels
explicitly) and make unscripted requests fail by default with an explicit
opt-out for tests that want leniency.

## Way after (kept for the record, not scheduled)

- Bound network requests and transient retries: synchronous `req.send()` has
  no deadline and 429/486/499 retries have no cap, so a hung request blocks a
  chunk forever and TUI cancellation can't interrupt it. A request timeout
  plus a simple retry cap is enough; typed timeout taxonomies are not needed.
- Move selection-time workflow planning off the render thread if generated
  acquisitions ever make browsing feel sluggish (debounce/token machinery is
  not worth it until then).

Dropped from the original 29 review findings, with reasons:
- Immutable `ctx` proxy for builds — defends against our own workflow files;
  they are trusted local code.
- Composition path-traversal rejection / cwd-independence — same trust model,
  and the whole app (including the TUI scan) already assumes the repo cwd.
- Per-category `SourceIndex` availability diagnostics — error-message polish.
- Threading projected state between composed generators — plans remain
  correct (interp simulates the whole AST); only routing is suboptimal, and
  the fix duplicates the simulator.
- Duplicate CLI argument rejection + richer parameter help — last-value-wins
  is conventional CLI behavior.
- Grapheme/CJK/tiny-terminal form editing — the `clamp(24, width)` panic was
  fixed directly (src/tui/widgets/form.rs); the rest is polish for terminals
  this tool doesn't target.
- Cache namespace per base URL + page-size tuning — one base URL in practice;
  perf polish.
- Restore formatting cleanliness — done; `cargo fmt --all -- --check` passes.
