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

## Task: Align `repeat-until` plan and run semantics
Context: `fennel/lib/interp.fnl` checks a loop predicate before the first iteration in the plan pass, but the run pass always executes the body once before checking. An already-satisfied `has_item` loop performs an unwanted action, and a full-inventory farm can attempt a gather even though its plan predicts zero gathers.
Desired outcome: Planning and live execution use identical precondition-loop semantics.
Acceptance criteria:
- An initially true predicate produces zero planned and zero executed body actions.
- Deterministic initially-false scenarios produce matching planned and executed iteration counts.
- A predicate that becomes true exactly on iteration `MAX-ITERS` succeeds in both passes; only a still-false predicate is reported as exhaustion.
- Broad live-entrypoint coverage includes an already-satisfied item goal and a full-inventory farm/hunt case.

## Task: Make inventory-consuming simulations reject shortages
Context: `inv-remove` in `fennel/lib/actions.fnl` clamps an absent/short item to zero while subtracting the requested amount from total inventory. Craft, deposit, recycle, use, delete, give, sell, GE fill, and task trade can therefore plan as feasible without the required items; craft/sell can even credit outputs or gold after fabricated consumption. `tests/all_intents.rs` currently blesses crafting daggers from an empty inventory.
Desired outcome: Every simulated consumption either consumes the full requested quantity or records a precise blocker, while preserving a consistent inventory map/count and withholding dependent outputs/rewards when prerequisites fail.
Acceptance criteria:
- Crafting without all inputs and depositing/selling/giving more than held are infeasible and name required versus available quantities.
- Unrelated inventory never disappears from `inventory-count` after a shortage.
- Exact-stock consume/produce scenarios retain correct final inventory and gold.
- Existing broad action tests are corrected (including the empty-inventory craft expectation in `tests/all_intents.rs`) rather than adding one helper-level test per action.

## Task: Model bank-consuming actions against bank holdings
Context: Bank data is now loaded and exposed to builds, but the interpreter state and `withdraw-item`/`withdraw-gold` simulations still fabricate inventory/gold without checking or decrementing bank holdings. Attaching a `bank` table only to some plan paths cannot enforce withdrawal feasibility, and repeated bank actions can spend the same holdings more than once.
Desired outcome: Bank is a first-class simulated state component for every plan path; withdrawals consume modeled holdings and block on shortages, while deposits update it where subsequent actions depend on bank state.
Acceptance criteria:
- Withdrawing more item/gold than the bank holds is infeasible and names required versus available amounts.
- Sequential withdrawals cannot reuse already-consumed holdings; deposits make only the deposited amount available.
- CLI plan, single run gate, campaign, and TUI use identical bank simulation.
- Broad bank workflows cover exact stock, shortage, and deposit-then-withdraw behavior.

## Task: Redesign acquisition chunking around actual drops
Context: `emit-chunked-loop` exits when total inventory is full, then deposits `load` units of the target as if every gained slot contained that item. Monsters/resources can produce byproducts, rare targets, or multi-quantity drops. The current planner hides over-deposits because inventory removal clamps shortages, and tests cover only one guaranteed single drop.
Desired outcome: Bank trips derive deposit quantities and projected holdings from the actual target/byproduct distribution; generated actions must never deposit more of an item than the immediately preceding model can hold.
Acceptance criteria:
- Broad gather and fight fixtures include a target plus deterministic/rare byproducts.
- Every emitted deposit quantity is available in modeled inventory at that point.
- The corresponding MockDriver live run completes across bank trips without an invalid deposit and converges on the requested target disposition.
- Probabilistic/variable gather drops are classified honestly instead of treated as deterministic solely because the action is `gather`.

## Task: Restore a clear `acquire.item` result contract
Context: The design promises that acquisition ends with at least `qty` in inventory, while capacity chunking silently weakens that contract by banking full trips and leaving only a final partial load. For example, requesting 25 items with capacity 10 can report success while holding 5; `craft.fnl` describes `qty` as the amount to end up holding. `keep_in_bank` also does not make the default ambiguity acceptable.
Desired outcome: Make final disposition explicit and enforceable: inventory-targeted acquisition must fit and end in inventory, while bank-targeted/intermediate-material acquisition uses a distinct mode with a documented postcondition.
Acceptance criteria:
- A top-level request larger than inventory capacity cannot report success with only a partial quantity held.
- Inventory and bank target modes have distinct names/contracts and tests.
- Craft input chunking uses the explicit intermediate/bank mode rather than relying on an undocumented exception.

## Task: Validate quantity and count domains at workflow boundaries
Context: Fennel `:number` coercion accepts zero, negative, fractional, NaN, and infinite values. Acquisition quantities, gold budgets, merchant prices, and `repeat-n` counts can consequently yield empty/nonterminating plans or fail later during Rust `u32` conversion after planning succeeded.
Desired outcome: Reusable schema/build validation for finite integer domains, with positive quantities/prices and nonnegative repeat counts as appropriate.
Acceptance criteria:
- `qty=0`, negative, fractional, NaN, and infinity fail before AST execution with parameter-specific messages.
- Invalid budgets/prices/counts are rejected at the same boundary.
- Valid integer strings retain current coercion behavior.
- Cases are consolidated into the existing workflow-parameter scenario.

## Task: Make generated NPC purchases honor live affordability and action limits
Context: `acquire.fnl` compares a purchase only with configured budget, not projected/live gold, and emits an unconditional `npc-buy`. A budget above current gold can generate a known-unaffordable action. The NPC concepts documentation also describes a per-action quantity limit that the generator does not enforce, while the current OpenAPI schema is ambiguous.
Desired outcome: Purchase generation and execution use catalog price, remaining budget, current gold, and confirmed server quantity limits consistently, with a live guard against affordability drift.
Acceptance criteria:
- A budget greater than current gold does not produce an unconditional buy.
- Known local gold drift before dispatch suppresses the purchase; unavoidable concurrent/server rejection surfaces an actionable incomplete-goal error rather than being treated as completion.
- The supported server maximum is verified against authoritative behavior/spec; generated buys are chunked if required.
- Budget, inventory, cooldown, and projected gold remain correct across chunks.

## Task: Remove unchecked price hints from NPC action contracts
Context: `fennel/workflows/buy-from-merchant.fnl` still requires a user-supplied price even though `NpcItemData` exposes authoritative fixed catalog prices. More broadly, public `npc-buy` and `npc-sell` simulations accept price hints that live execution discards. Incorrect/zero/negative hints make plan predicates and gold deltas diverge from the server, while current tests manufacture responses from the same arbitrary constants.
Desired outcome: Retire the stale compatibility workflow and make both NPC buy/sell simulations resolve and validate the exact catalog listing, currency, and price used by live execution.
Acceptance criteria:
- No NPC workflow/action accepts an unchecked buy or sell price hint.
- Unknown NPC/item pairs, unsupported direction, and non-gold listings fail before dispatch.
- Buy and sell acceptance coverage derives expected prices from API-shaped NPC fixtures and verifies live/model gold parity.

## Task: Canonicalize schema validation and value coercion
Context: `workflow::load` invokes Fennel `validate_schema`, but `workflow::schema` manually marshals without validation and silently drops/buries malformed options/defaults. The TUI then duplicates value validation in Rust, already disagreeing on required empty strings and potentially on Lua `tonumber` syntax/non-finite values.
Desired outcome: One canonical schema validator/coercer defines CLI, composition, workflow scanning, and TUI behavior; introspection returns strictly validated metadata instead of reconstructing permissive defaults.
Acceptance criteria:
- Every schema rejected by `load` is rejected by `schema` with the same root reason.
- Invalid option/default types are errors, never omitted or converted to empty strings.
- A shared parity corpus covers absent/empty/whitespace strings, number formats, booleans, enums, defaults, and non-finite values through Fennel and the form.
- The form tracks presence separately from text if empty required strings remain valid.

## Task: Prevent workflow builds from mutating planner state
Context: The protocol documents `ctx` as read-only, but it is an ordinary mutable Lua table. `planner::plan_named` passes that exact table to `build` and then `interp.plan`, so a workflow can forge capacity, inventory, skills, HP, or nested combat fields and defeat feasibility checks; campaign builds/plans from separate state and can disagree.
Desired outcome: Build receives a deeply immutable proxy, or at minimum a deep copy that cannot alias the plan seed.
Acceptance criteria:
- Top-level and nested `(tset ctx ...)` attempts either fail clearly or cannot affect the state passed to `interp.plan`.
- CLI plan, campaign, and TUI produce the same result for a mutating workflow.
- Coverage asserts the observable blocker/result through `planner::plan_named`, not table identity.

## Task: Make composed workflow resolution safe and location-independent
Context: The `workflows.<stem>` searcher defaults to cwd-relative `fennel/workflows`, even when the top-level workflow lives elsewhere, and joins unvalidated stems so separators/`..` can escape the configured root. Planner/live do not thread the top-level workflow directory into `LuaSetupOptions`.
Desired outcome: Resolve composition relative to an explicit canonical workflow root and reject path traversal/unsupported module shapes.
Acceptance criteria:
- A workflow in a temporary external directory can compose a sibling independent of process cwd.
- Absolute paths, separators, `..`, and canonical root escapes are rejected.
- Repository workflows continue resolving normally.
- Composition coverage uses a temporary root rather than depending on Cargo's cwd.

## Task: Restrict acquisition to navigable sources
Context: `SourceIndex` ranks global `/resources` and `/monsters` data while only the overworld map is loaded. It can choose underground/interior or otherwise unreachable content, then fail at `find_tile` without trying later candidates. Map lookup also ranks by Manhattan distance instead of reachable path cost and optimistic path fallback can turn an impossible route into a plausible plan. The existing map-event cache task remains separate but compounds this problem.
Desired outcome: Source eligibility and ranking use the same layer/access/path model as navigation, with explicit reasons for unreachable candidates and fallback to later viable sources.
Acceptance criteria:
- Either the complete layer/transition graph is loaded or nonloaded-layer sources are excluded before ranking.
- The nearest reachable source by path cost wins; disconnected/restricted/conditional destinations do not become Manhattan-cost plans.
- If the first ranked source is unavailable, later sources are attempted and the final no-route error lists reachability reasons.
- Coverage includes duplicate sources across reachable and unreachable layers/regions.

## Task: Validate and normalize recipe data before generation
Context: Recipe fields are permissive on the wire, but acquisition assumes positive output quantity, a usable skill/workshop, integral batches, nonempty inputs, and unique input codes. Missing/zero values can cause division by zero, fractional requests, or `find_tile(:workshop, nil)`; duplicate inputs are acquired independently but consumed cumulatively.
Desired outcome: Separate “craft metadata exists” from “recipe is usable,” normalize duplicate inputs, and expose item-specific validation errors before generation.
Acceptance criteria:
- Output/input quantities are positive and batch conversion matches server semantics.
- Duplicate input codes are summed deterministically or rejected.
- Missing operational fields produce a clear unusable-recipe reason rather than a runtime Lua/Rust error.
- Valid recipes retain current generation and simulation behavior.

## Task: Preserve reference-data availability in `SourceIndex`
Context: `SourceIndex::build` errors only when all four datasets are absent. If recipes load but monsters do not, a monster-only item is reported as having no known source rather than “monster data unavailable,” making partial bootstrap failure indistinguishable from authoritative absence.
Desired outcome: Track per-category availability or require a complete source bundle wherever authoritative source selection is promised.
Acceptance criteria:
- Missing categories produce an explicit incomplete-data diagnosis.
- A loaded category with no matching row remains distinguishable from an unloaded category.
- Planner/TUI/CLI surface the same diagnosis.

## Task: Thread projected state through composed generators
Context: `daily.fnl` builds each sub-workflow immediately from the same original `ctx`. A second generated workflow therefore chooses sources relative to the starting position/inventory rather than the first workflow's projected endpoint/effects, even though `acquire` itself carefully threads an itinerary internally.
Desired outcome: Define a composition mechanism that threads projected context or explicit endpoint/disposition metadata between generated children without duplicating the full simulator.
Acceptance criteria:
- With duplicate source tiles, the second child selects the source nearest the first child's projected endpoint.
- Combined travel cost matches the composed itinerary rather than two independent start-state plans.
- Existing simple composition remains deterministic and gains a broad duplicate-tile scenario.

## Task: Fix campaign iteration-boundary semantics
Context: `run_until_done` uses the iteration cap for build probes as well as executed chunks. A campaign requiring exactly `N` successful chunks executes chunk N, then reports exhaustion without the final build needed to observe nil; the current test codifies this surprising failure.
Desired outcome: Define `--max-iterations N` as a cap on executed chunks and allow one final build-only completion probe.
Acceptance criteria:
- Exactly N chunks followed by nil succeeds with a cap of N.
- An N+1st chunk is detected but not executed.
- Exhaustion reports completed chunks and the rejected next chunk clearly.
- The existing broad campaign scenario observably fails if `build` is called more than once for one fetched iteration; it does not infer build count from fetch count.

## Task: Detect campaigns that make no progress
Context: A non-nil zero-action AST or an unchanged live context that regenerates equivalent work can burn the full default 100 campaign iterations, repeatedly fetching or sending no-op/duplicate actions. The current implementation relies only on the coarse iteration cap.
Desired outcome: Fail promptly with a “campaign made no progress” diagnosis while preserving legitimate repeated chunks whose observable context changes.
Acceptance criteria:
- A feasible non-nil zero-action AST fails before any action request.
- Equivalent generated work against unchanged character/bank context fails promptly.
- Legitimately changing campaigns continue and eventually observe nil.

## Task: Bound network requests and transient retries
Context: New campaign/TUI execution can still block indefinitely inside synchronous `req.send()`/body reads, and scheduler retries for persistent 429/486/499 responses have no retry or elapsed-time budget. TUI cancellation cannot interrupt an in-flight request, and plain campaigns can hang forever inside one chunk.
Desired outcome: Documented connect/request deadlines, bounded per-intent retry/backoff, and typed timeout/cancellation outcomes.
Acceptance criteria:
- Every network request terminates within a documented upper bound.
- Finite transient sequences still succeed; persistent transient responses stop at an exact tested request/time budget with status history.
- Cancellation interrupts retry sleeps promptly and settles an in-flight request within the request bound.
- Timeout and user cancellation remain distinguishable to CLI/TUI users.

## Task: Reject ambiguous duplicate CLI arguments and improve help
Context: Duplicate `key=value` parameters and singleton flags silently use the last value. A malformed parameter token fails before loading schema, so the error omits the workflow's declared-parameter help despite the design promise. Existing tests focus narrowly on private `--max-iterations` parser branches.
Desired outcome: Deterministic command-level argument validation with canonical per-workflow help on parameter failures.
Acceptance criteria:
- Duplicate parameter names and duplicate singleton flags fail before network setup and name the duplicate.
- Malformed `key=value` input includes the workflow's declared parameter listing.
- Flags and params can be interleaved only according to one documented grammar.
- Coverage is a table-driven command/parser scenario rather than one test per private branch.

## Task: Version TUI plans by complete live context and run provenance
Context: TUI plan cache entries are keyed only by selected index and `PlanSeed`; bank is fetched once, idle polling updates the character without refreshing the displayed plan, and reaping invalidates whichever workflow is currently selected rather than the one launched. Bank-changing runs can therefore leave stale bank-aware plans, and switching selection during a run invalidates the wrong entry.
Desired outcome: A versioned live context (character plus bank) and immutable run provenance drive plan cache keys, invalidation, and replanning.
Acceptance criteria:
- Idle character changes invalidate/recompute the displayed selected plan.
- Bank is refreshed after any potentially bank-mutating run and before the next bank-aware plan/run; refresh failure is visible.
- Reaping invalidates the launched workflow regardless of current selection.
- Cache identity includes workflow/source, params, seed, and bank generation; a selected plan never disappears merely because another run completed.
- One App-level scenario proves form opening, submitted-param replanning, blocked-value preservation, and force launch with exactly those values.

## Task: Capture a terminal TUI run snapshot after worker completion
Context: `tick` refreshes run rows before checking `JoinHandle::is_finished`. If terminal status is published between those operations, `reap_worker` changes to Idle and later row refreshes are skipped, leaving the final row permanently active/spinning.
Desired outcome: Joining/reaping always takes one final consistent status/progress snapshot before entering Idle.
Acceptance criteria:
- Idle cannot coexist with a cached run phase of Running/Stopping.
- A synchronized test forces completion between ordinary refresh and reap and observes terminal rows.
- Panic/failure terminal states still surface their messages and final progress.

## Task: Make idle-poller ownership atomic with TUI run launch
Context: The idle flag is updated only on the next tick after a run enters Running. A poll completing between the launch event and that tick can publish an idle snapshot into the `SharedView` now owned by the scheduler.
Desired outcome: Transfer view ownership atomically as part of the launch transition and discard stale poll results.
Acceptance criteria:
- Idle polling is disabled before a worker can start; failed spawn restores it.
- A poll started while idle cannot publish after run ownership changes.
- A synchronized test proves scheduler updates cannot be clobbered at launch.

## Task: Make TUI cancellation and quit explicit lifecycle outcomes
Context: Quit keys set `should_quit` immediately without routing an active run through `Stopping`, while `run_worker` converts any error to Done whenever the abort flag happens to be set. Genuine network/Lua/plan failures racing with cancellation can disappear, and completed/cancelled are indistinguishable.
Desired outcome: Typed Completed, Cancelled, and Failed outcomes; normal quit requests cancellation/reap before terminal teardown, with a separately explicit force-quit if needed.
Acceptance criteria:
- Quit during Running sets the same cancellation signal as stop and no new action is submitted afterward.
- Only a typed cancellation is suppressed; unrelated errors remain visible even if abort was requested.
- Run UI distinguishes completed from cancelled and passes cooldown/request race tests.
- Terminal restoration still occurs on worker failure or panic.

## Task: Centralize TUI overlay state and global key routing
Context: Tooltip, palette, form, and error are independent states. A tooltip can render over an active form/palette while input goes to the hidden modal. Ctrl-C is checked after modal handlers, so it may be swallowed or inserted as `c`; other Ctrl/Alt character events can also enter text.
Desired outcome: One exclusive overlay/modal state and true global shortcuts handled before modal-local input.
Acceptance criteria:
- At most one input-capturing overlay is active/rendered.
- Opening form, palette, or error closes/suspends tooltip.
- Ctrl-C requests quit from every mode/overlay and modified characters are inserted only when explicitly supported.
- Event-entrypoint coverage exercises overlay transitions and routing.

## Task: Make the parameter form robust on small and Unicode terminals
Context: Width calculation uses `.clamp(24, terminal_width)`, which panics below 24 columns. Tall forms are clipped without focus-following vertical scroll; long/wide values lack horizontal scroll; cursor editing uses Unicode scalar indexes rather than grapheme boundaries/display columns. The unstable ratatui line-count feature still does not solve these cases.
Desired outcome: A total, scrollable form renderer/editor for every `Rect`, using grapheme editing and terminal display widths; remove the unstable dependency if the revised layout no longer needs it.
Acceptance criteria:
- Rendering at `0×0`, `10×4`, `23×8`, and normal sizes never panics and shows a useful too-small fallback where necessary.
- Focused fields/errors stay visible in forms taller than the viewport, with above/below indicators.
- Combining marks, CJK, and multi-codepoint emoji edit as graphemes; horizontal scrolling keeps the caret visible.
- Tests use `TestBackend` and exercise both navigation and rendering.

## Task: Move TUI workflow planning off the render thread
Context: Every selection change synchronously creates Lua state and builds/plans a workflow on the UI thread. Generated acquisitions can stall input, redraw, and spinner animation while users hold navigation keys.
Desired outcome: Debounced asynchronous planning with generation/provenance tokens so stale results cannot overwrite the current selection/context.
Acceptance criteria:
- Selection redraws immediately and shows a Pending state.
- Rapid A→B→C navigation cannot publish A/B results into C.
- Only results matching workflow, params, live-context generation, and source version enter the cache.

## Task: Surface workflow scan failures instead of an empty list
Context: `App::new` converts any global `workflows::scan` error to an empty vector. Directory read, Lua setup, or unreadable-entry failures therefore look like “no workflows found,” while CLI sees direct errors; source/schema snapshots also remain stale for the session.
Desired outcome: Distinguish a legitimately missing/empty directory from global/per-file failures, and define a reload path compatible with future workflow editing.
Acceptance criteria:
- Missing/empty directory has an intentional empty-state message.
- Global failures are actionable and include the path; one unreadable workflow does not hide readable workflows.
- Reload refreshes source, schema, completion metadata, params, and affected plan-cache entries consistently.

## Task: Harden reference-data loading policies
Context: Cache filenames do not include `HttpDriver` base URL, so mock/staging data can be trusted by production for 24 hours. Static endpoints are paged serially at 100 despite supporting much larger pages, while bank must remain at its smaller live endpoint limit. New NPC/source datasets increase cold-start cost and the impact of policy mistakes.
Desired outcome: Typed endpoint-specific pagination/cache policy with environment-safe cache identity.
Acceptance criteria:
- Non-default base URLs use a separate normalized/hash cache namespace or disable persistent caching.
- Static map/monster/resource/item/NPC endpoints request the largest supported page size and still paginate larger totals; bank keeps its authoritative limit.
- Tests prove a mock cache cannot be consumed by the production base URL.

## Task: Make execution fixtures strict and API-shaped
Context: `tests/common::char_json` omits API-required skill fields, allowing a nominally level-zero character to receive canned successful level-one gathers. `MockDriver` fabricates success for unscripted requests, so missing/duplicate actions can pass unless every test inspects logs. Generated acquisition has extensive plan/skeleton coverage but no full flagship live craft sequence.
Desired outcome: Broad entrypoint tests fail on unexpected actions and use fixtures matching authoritative response shapes.
Acceptance criteria:
- Shared character JSON includes every required skill field and scenarios explicitly set relevant levels.
- Execution tests default to strict unscripted-request failure (with opt-out only when intentional).
- A generated one-level craft runs through `live::run_workflow` with bank withdrawal/gather/craft requests and final inventory assertions.
- The live-integration procedure records at least one current-server generated craft proof without making it a default test.

## Task: Restore formatting cleanliness
Context: `cargo fmt --all -- --check` fails in `src/planner.rs` and `src/tui/ui.rs` on the reviewed endpoint, even though Clippy and all automated tests pass.
Desired outcome: The full range is rustfmt-clean without unrelated formatting churn.
Acceptance criteria:
- `cargo fmt --all -- --check` passes.
- `cargo clippy --all-targets --all-features -- -D warnings` passes.
- `cargo test --all-features` passes.
