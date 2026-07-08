# Dynamic Workflows — design

Status: **design, ready to implement** (spike deliverable from TODOS.md).
Companions: `docs/ARCHITECTURE.md` (current system), `plans/ACHIEVEMENTS.md`
(future spike — §7.5 defines the seam it will build on).

This document defines how the workflow system becomes *dynamic*: parameterized
workflows, workflows that compose other workflows, and goal-directed
*generators* ("craft me an iron sword" → a complete, plan-validated workflow).
It specifies what the system can and can't do, how the pieces compose beyond
the motivating examples, and a milestone-by-milestone build plan naming the
exact interfaces that change. Implementation details (function bodies, exact
struct fields) are left to the implementer; every *decision* is made here.

---

## 1. The core insight: three "rungs" that collapse into one protocol

The TODO frames three seemingly different asks:

1. **Parameterize** — `farm-copper` and `farm-chickens` are one skeleton with
   different targets; the target should be a CLI/TUI input.
2. **Compose** — workflows importing other workflows (higher-order workflows).
3. **Generate** — start from "craft X", work backwards through recipes and
   bank holdings to gather/fight/buy steps; TUI v2 picks an item and gets a
   workflow.

These are one mechanism at three intensities. Today a workflow file already
*evaluates* to an AST value (it is not executed on load). The single change
that unlocks everything: a workflow file evaluates to a **workflow module** —
a table with a declared parameter schema and a `build` function that returns
the AST:

```fennel
{:doc    "Farm a resource or monster until inventory is full, then bank."
 :params {:target {:type :resource :required true :doc "what to farm"}}
 :build  (fn [params ctx] <ast>)}
```

Then:

- **Rung 1** is calling `build` with params parsed from the CLI or a TUI form.
- **Rung 2** is one workflow `require`-ing another and splicing the AST that
  its `build` returns into its own `seq` — ASTs are plain data, so composition
  is function application plus table splicing. No new evaluation machinery.
- **Rung 3** is a workflow module whose `build` *computes*: the
  backwards-crafting planner is not a new subsystem, it is a library function
  (`fennel/lib/acquire.fnl`) that a `build` calls to construct its AST. The
  TUI's "pick an item to craft" is just the generic param form pointed at
  `workflows/craft.fnl`, whose one param has `:type :item`.

Everything downstream is unchanged **by construction**: `build` returns an
ordinary AST, so the existing `plan` pass (cost + feasibility + blockers) and
`run` pass, the TUI skeleton, progress ids, and loop counts all work on
generated workflows exactly as on hand-written ones. The plan pass becomes the
universal *validator* for generator output: a generator doesn't have to be
perfect, because a wrong plan is caught as a blocker before anything runs.

`ctx` (second `build` argument) is a read-only snapshot of the character's
seed state — the same table shape `predicate_state` builds, plus holdings
(§5.6). Generators need it (inventory capacity for trip-chunking, skills for
"can this character even mine this"), and passing it explicitly keeps `build`
a pure function of `(params, ctx, reference data)` — the same purity contract
predicates already obey, and the reason plan and run can't drift.

## 2. The workflow-module protocol

### 2.1 Shape

A `.fnl` file under the workflows root must evaluate to:

| key | type | required | meaning |
| --- | --- | --- | --- |
| `:build` | `(fn [params ctx] ast)` | yes | Pure constructor of the workflow AST. |
| `:params` | table of param specs | no (default `{}`) | Declared inputs; the *only* inputs `build` may read besides `ctx`. |
| `:doc` | string | no | One-liner for the TUI list and CLI errors. |

A bare-AST file (the current format) is **rejected with a loud, prescriptive
error** ("workflow must export {:build ...}; wrap your AST in
`{:build (fn [_ _] <ast>)}`"). One protocol, no dual-format support — the repo
has three workflows; migrate them in the same change (§8 M1).

### 2.2 Param specs

```fennel
:params {:target {:type :resource :required true :doc "resource to farm"}
         :qty    {:type :number   :default 1     :doc "how many to make"}}
```

Fields per param: `:type` (required), `:required` (default false), `:default`
(must be absent when `:required`), `:doc`, and `:options` (for `:enum` only).

Types and their coercion (from CLI strings) / TUI affordance:

| `:type` | coercion | TUI completion source |
| --- | --- | --- |
| `:string` | identity | free text |
| `:number` | `tonumber`, error if not numeric | free text |
| `:bool` | `"true"`/`"false"` only | toggle |
| `:enum` | membership in `:options` | the options list |
| `:item` | string; semantic validity is enforced downstream by loud host lookups (`host.recipe`, `host.item_sources`) | recipe outputs ∪ npc catalog ∪ drop tables |
| `:resource` | string, ditto (`host.find_tile`, `host.active_resource`) | `ResourceData` codes |
| `:monster` | string, ditto (`host.monster_stats`) | `MonsterData` codes |
| `:npc` | string, ditto (`host.find_tile`) | npc catalog codes |
| `:skill` | string | fixed skill list |

Deliberate division of labor: **shape** validation (unknown key, missing
required, bad number, bad enum) lives in `fennel/lib/params.fnl`; **semantic**
validation ("no such monster") stays where it already lives — the loud host
lookups that fire during `build`/`plan`. No second registry of what codes
exist.

### 2.3 One validator, in Fennel

`fennel/lib/params.fnl` (new) exports (Lua-safe underscore names, per the
established convention):

- `validate_schema (schema)` — asserts the schema itself is well-formed
  (every param has a known `:type`, `:enum` has `:options`, no
  `:required`+`:default` conflict). Run once at load.
- `coerce (schema raw)` — takes raw params (a table of strings from the CLI,
  or already-typed values from the TUI/a composing workflow), applies
  defaults, coerces by type, and errors listing the declared params on any
  unknown key / missing required / uncoercible value. Returns the typed
  params table handed to `build`.

Rust does **not** re-implement any of this: the Rust loader passes raw string
pairs through to this module. One implementation, both entry points.

### 2.4 The Rust loader — `src/workflow.rs` (new)

The single place the protocol is implemented on the Rust side; `planner`,
`live`, and the TUI all go through it (today each calls `eval_fennel`
directly — those call sites migrate here):

```rust
/// Eval `src`, validate it exports the module protocol, coerce `params`
/// through fennel.lib.params, call build(params, ctx). Returns the AST.
pub fn load(lua: &Lua, src: &str, name: &str,
            params: &[(String, String)], ctx: LuaTable) -> Result<LuaValue>

/// Eval `src` and marshal just its :params schema + :doc (no build call) —
/// the TUI param form and CLI usage errors read this.
pub fn schema(lua: &Lua, src: &str, name: &str) -> Result<WorkflowInfo>

pub struct WorkflowInfo { pub doc: Option<String>, pub params: Vec<ParamSpec> }
pub struct ParamSpec { pub name: String, pub ptype: ParamType,
                       pub required: bool, pub default: Option<String>,
                       pub doc: Option<String>, pub options: Vec<String> }
```

`ctx` is built by the caller: `planner::plan` passes the same seed state table
it already builds (`build_state`); `live::run_workflow` builds one from the
live view through `predicate_state` (+ holdings, §5.6). Same construction
path both sides — the existing anti-drift argument, extended to `build`.

### 2.5 Entry-point signature changes

- `planner::plan(..., params: &[(String, String)])` — one added argument.
- `live::run_workflow(..., params: &[(String, String)], ...)` — ditto (it
  evaluates the workflow in its own character-equipped Lua state, so params
  must travel to it as data, not as a pre-built AST).
- CLI: trailing `key=value` arguments after the character —
  `artifacts run fennel/workflows/farm.fnl nillinbot target=copper_rocks`.
  Same for `plan`. A parse failure or param error prints the workflow's
  declared params (via `workflow::schema`) — that *is* the per-workflow help.
- TUI `spawn_tui_run(..., params)` — forwarded into `run_workflow`. Until M6
  (param form), the TUI can only run workflows whose required params are all
  defaulted; selecting one with unmet required params shows the schema as a
  hint line instead of running.

## 3. Composition — higher-order workflows

### 3.1 Workflows become require-able

Sub-workflow use is ordinary `require`:

```fennel
(local farm (require :workflows.farm))
(local {: coerce} (require :fennel.lib.params))

{:build (fn [_ ctx]
          (seq
            ((. farm :build) (coerce (. farm :params) {:target :copper_rocks}) ctx)
            (action :travel-to [...])))}
```

Mechanism: `setup_lua` registers a **package searcher** that resolves the
`workflows.<name>` namespace by reading `<workflows_root>/<name>.fnl` from
disk and compiling it with the embedded Fennel. `workflows_root` is a new
`LuaSetupOptions` field defaulting to `fennel/workflows` (cwd-relative — the
path the TUI already scans). `package.loaded` memoizes, so diamond imports
are cheap and cycles surface as Lua's standard require-cycle error (fine;
document it). The `fennel.lib.*` modules stay `include_str!`-seeded exactly
as today.

Composing through `coerce` (not raw table literals) keeps a sub-workflow's
defaults/required checks working when invoked from Fennel, identically to
CLI/TUI invocation. A `use_workflow` sugar helper in `interp.fnl` —
`(use_workflow :farm {:target :copper_rocks} ctx)` doing
require+coerce+build in one call — is worth adding for exactly this pattern.

### 3.2 A `:group` node for legibility

Composed and generated workflows flatten into long step lists. One new
**structural** AST node keeps them legible:

```fennel
{:type :group :label "acquire 4× copper_ore" :steps [...]}
```

- `plan`/`run`: walk children; no cost, no action count of its own.
- `skeleton`: emit a `:group` row at `depth`, children at `depth + 1`,
  `guard-id` passed through unchanged.
- `number-nodes` already handles it (it visits any node with `:steps`).
- Constructor `(group label ...steps)` exported from `interp.fnl`; the TUI
  renders the row like a loop header without a count.

`use_workflow` wraps its result in a `:group` labelled with the workflow name
and params, so composed runs read as an outline for free.

### 3.3 The load-time-resolution contract (stated, not changed)

`host.find_tile` runs at *build time*, anchored to the seed origin (plan) or
live position at workflow start (run). Composition amplifies an existing
wrinkle: a sub-workflow built for "nearest bank" is resolved from the
*initial* position, not from wherever the character stands when that step is
reached mid-run. The fix that keeps `build` pure: `find_tile` gains an
optional explicit anchor (§5.7), and a *generator* — which knows the
itinerary it is emitting — threads the previous destination as the anchor for
the next lookup. That covers the real cases without making the AST lazy.
Fully late-bound arguments (args as functions of state) are specified in §9
as a deferred extension; nothing in M1–M6 requires them.

## 4. Generators — goal-directed workflow construction

### 4.1 `fennel/lib/acquire.fnl` (new): the acquisition library

The flagship generator, and the reusable core for future ones. Contract:

```fennel
(acquire.item code qty ctx opts) → ast   ;; ends with ≥ qty of `code` in inventory
```

Algorithm (deterministic, recursive, memoized per item within one call):

1. **Holdings first.** `have = ctx.inventory[code]`; then bank:
   `bank_have = ctx.bank[code]` → emit (travel to nearest bank, withdraw)
   for `min(need, bank_have)`. Remaining `need` continues.
2. **Pick a source** for the remainder via `host.item_sources(code)` (§5.5),
   in policy order (first eligible wins):
   - **craft** — a recipe exists and `ctx.skills[recipe.skill] >= recipe.level`:
     recurse on each input (`batches × input.quantity`), then travel to the
     `:workshop` tile for the recipe's skill and emit `(action :craft [code qty])`.
   - **gather** — a resource drops it and `ctx.skills[resource.skill] >=
     resource.level`: travel to the nearest such resource tile, then
     `repeat_until (has_item code target-qty) :gathers (action :gather)`.
   - **fight** — a monster drops it and the fight simulator predicts a win
     from full HP: the farm-chickens pattern (rest-gate via `is_winnable`,
     fight loop with `has_item` exit).
   - **buy** — an NPC sells it for **gold** and
     `price × need ≤ opts.gold-budget`: travel to that npc tile,
     `npc-buy` guarded by `gold_at_least` (the buy-from-merchant pattern).
     Non-gold currencies (item-priced listings) are never auto-chosen; the
     source is surfaced in the "no route" error so the author can hand-write it.
   - **no route** → `error` with the item and every rejected source + reason
     ("resource iron_rocks needs mining 10, have 4") — a build-time failure
     the CLI/TUI surface verbatim. Loud beats a lying plan.
3. **Capacity chunking.** The generator tracks projected inventory count as
   it emits (it knows every deterministic quantity, and expected drop rates
   for loops). When a gather/fight loop's target would overflow
   `ctx.inventory-max-items`, it splits the loop into full-inventory trips —
   gather-until-full, bank the intermediates, travel back — before the final
   partial trip. The plan pass's overflow blocker remains the backstop.
4. **Ordering.** Materials are acquired in recipe-DAG post-order (leaves
   first), each wrapped in a labelled `:group`; travel between phases anchors
   `find_tile` on the previous phase's destination (§3.3, §5.7).

`opts` (all defaulted): `:policy` — the source ranking, overridable per call
(e.g. `[:buy :craft]` for a "money is no object" variant); `:gold-budget` —
cap on auto-chosen purchases (default: `ctx.gold`); `:keep-in-bank` — end by
depositing the target rather than holding it.

Everything the algorithm consults is reference data or `ctx` — no I/O, no
live host fns — so `acquire.item` is as pure as a predicate, testable against
fixtures, and identical in plan and run invocations.

### 4.2 `fennel/workflows/craft.fnl` (new): the flagship user-facing workflow

```fennel
{:doc "Acquire the materials for any craftable item and craft it."
 :params {:item {:type :item :required true} :qty {:type :number :default 1}}
 :build  (fn [p ctx] ((. (require :fennel.lib.acquire) :item) p.item p.qty ctx {}))}
```

That's the whole file: `artifacts run fennel/workflows/craft.fnl nillinbot
item=copper_dagger` plans (validated!) and runs the full tree — withdraw what
the bank holds, mine the rest, smelt at the mining workshop, craft at
weaponcrafting. The TUI v2 story is this file behind a param form.

### 4.3 Why this generalizes past crafting

The acquire library is one *strategy compiler*: goal → sources → sub-plans.
Other generators reuse its pieces rather than its shape:

- **Achievement runner** (`plans/ACHIEVEMENTS.md`): iterate `/achievements`
  criteria; each criterion type maps to an emitter — "gather N of X" →
  `acquire.item`, "kill N of Y" → the fight-loop emitter, "craft N" →
  `acquire` + craft. The deliverable there is a criterion→emitter table, not
  new machinery.
- **Gear-up**: for each equipment slot, pick the best craftable-at-current-
  skills item (reference data query), then `acquire` + `:equip`.
- **Restock / mule**: `withdraw` at bank + `give-item` to another character —
  parameterized workflow, no generator needed, enabled by M1 alone.
- **Task grinder** (once task state is on the view — punted with tasks
  generally): `task-new`, read the task, `acquire`-or-fight it, `task-complete`.

## 5. Groundwork: data and host-surface additions

Everything above needs world data we don't load yet, plus two genuine fixes to
the model surface. All of it follows existing patterns (TTL-cached reference
data, loud host lookups, `predicate_state` as the single state surface).

### 5.1 Resource drops + skill (fixes a real modeling bug)

`ResourceView` (`core/src/map.rs`) today carries only `code` + `level`, and
gather's `:sim` adds **the resource code** to the model inventory — a model
where mining copper yields `copper_rocks`-the-item, not `copper_ore`. Any
workflow that gathers then deposits/crafts a specific code is mispredicted
today; `has_item`-based loops (§5.4) would never terminate.

- Extend `ResourceView` with `skill: String` and `drops: Vec<Drop>` (the
  `/resources` payload already carries both; `DropRateSchema` is identical to
  the monster drop shape — hoist the existing drop struct from `combat.rs` to
  a shared home in core and reuse it).
- Bump `CACHE_SCHEMA_VERSION`.
- `host.active_resource` returns the enriched record (`skill`, `drops`).
- Gather `:sim` switches to `add-expected-drops` (the fight helper — primary
  drops are rate 1, so the common case stays deterministic-in-effect), and
  gains a skill gate: `ctx/st.skills[skill] < level` → `--pending-blocker`
  (same mechanism as an unwinnable fight). Gather keeps *hard* overflow
  blocking (its primary drop is deterministic); do not mark it
  `probabilistic-drops`.

### 5.2 Skills on the state surface

- `CharacterView` (`core/src/step.rs`): parse the eight `*_level` fields
  (mining, woodcutting, fishing, weaponcrafting, gearcrafting,
  jewelrycrafting, cooking, alchemy), `#[serde(default)]` like the combat
  stats.
- `predicate_state` gains a `skills` sub-table (`st.skills.mining`, …);
  `PlanSeed` gains the corresponding field; `state-eq` compares `:skills` by
  identity (no `:sim` mutates it — same treatment as `:combat`).
- Craft `:sim` gains the same skill gate as gather (recipe `skill`/`level`
  are already loaded).
- New predicate `skill_at_least (skill lvl st)`.

### 5.3 Inventory joins the *live* predicate surface (second real fix)

`STATE-KEYS` lists `:inventory` and the plan seed carries it, but `host.view`
does **not** include it — an inventory-contents predicate works in plan and
crashes in run today. Fix: `predicate_state` takes the inventory map as an
explicit parameter (built from `CharacterView.inventory` on the live side,
from `PlanSeed` — which gains per-item contents, not just the count — on the
plan side); `build_state`'s manual `st.set("inventory", …)` disappears. Both
passes now get the full surface from the one helper, closing the gap the
architecture doc's anti-drift argument already claims.

### 5.4 New predicate: `has_item`

`(has_item code qty st)` — `st.inventory[code] >= qty`. The generator's loop
exit ("gather until 30 copper_ore"), and generally useful. Depends on §5.1
(gather must add real drop codes) and §5.3 (live surface).

### 5.5 NPC item catalog + the item-sources index

- `/npcs/items` is **static reference data** (fixed prices, unlike the GE
  order book — the "GE uses author hints" decision stands and is unaffected):
  new `NpcItemData` in `src/data.rs`, cached as `npc_items.json` with the
  same TTL; view type (item `code`, `npc`, `currency`, `buy_price`,
  `sell_price`) in a new small core module; driver gains
  `fetch_all_npc_items`.
- **`SourceIndex`** (Rust, built once at `setup_lua` from the four datasets):
  inverted maps item-code → sources. Exposed as
  `host.item_sources(code) → {:craftable bool
                              :resources [{:code :skill :level :rate}]
                              :monsters  [{:code :rate}]
                              :npcs      [{:npc :currency :buy_price}]}`
  (recipe details stay behind the existing `host.recipe`). Loud error only
  when *no* dataset is loaded; an item with no sources returns empty lists —
  "unsourceable" is the generator's decision to report, with context.

### 5.6 Bank holdings

- Driver: `fetch_bank_items()` (paginated `/my/bank/items`; account-wide).
- **Not** TTL-cached — live account state, same class as the GE/tasks
  decision. Fetched once per invocation in `load_live_context` (1–2 pages)
  and passed through `LuaSetupOptions` as `bank: Option<Arc<BankData>>`.
- Surfaced two ways: `ctx.bank` ({code → qty}) for generators, and
  `host.bank()` for predicates that want a start-of-run snapshot.
- **Deliberately deferred** (M4b, independent): putting bank on the *mutable*
  model state with deposit/withdraw `:sim`s moving quantities (which would
  make `withdraw-item`'s "trusts the workflow" honesty note obsolete and give
  `state-eq` a bank clause). The generator doesn't need it — it checks
  holdings itself at build time — so it ships only if/when a hand-written
  workflow is bitten by the trust gap.

### 5.7 `find_tile` explicit anchor

`host.find_tile(kind, code, {:x :y}?)` — optional third argument overrides
the anchor (live position / seed origin). Generators thread itinerary
positions through it (§3.3, §4.1-4). Backwards compatible; a few lines in
`src/lua.rs`.

## 6. Capabilities and non-capabilities

**Can, after this design ships:**
- One skeleton, many targets, from CLI (`k=v`), TUI form, or a composing workflow.
- Workflows importing workflows, with grouped, legible skeletons and plans.
- "Craft/obtain any item" end-to-end: bank withdrawal, recursive crafting,
  gathering, fighting, gold-bounded NPC buying — chosen by an explicit,
  overridable policy, validated by the plan pass before running.
- Honest feasibility for skill requirements, real gather yields,
  inventory-contents loop exits, capacity-aware trip chunking.
- New generator *kinds* (achievements, gear-up, task grinding) as Fennel
  libraries reusing acquire's emitters — no Rust changes beyond data they
  might need.

**Can't (explicit non-goals; each is a stated punt, not an accident):**
- **No mid-run regeneration.** `build` runs once at load; the run executes a
  fixed AST whose predicates adapt within it. Drift response = stop,
  regenerate, rerun (the TUI v2 "drift alarm" backlog item is the hook).
- **No XP/leveling modeling.** "Gather until mining 10" would stall-bail in
  the plan (xp isn't on the model, and xp-per-action isn't clean reference
  data). Leveling workflows use bounded `repeat_n` budgets for now.
- **No cross-character orchestration.** `give-item`/`give-gold` exist as
  actions; there is no multi-character scheduler and none is designed here.
- **No GE automation in generators.** GE stays author-hint-driven (live order
  book); generators use static NPC prices only and *mention* GE listings in
  no-route errors at most.
- **No optimization.** Source choice is a greedy declared policy, not
  cost-minimizing search. The plan pass prices the result; if we ever want
  "cheapest of N candidate plans", generate N and compare `PlanResult`s —
  the interfaces here already permit that without change.
- **Ephemeral event content** (the stale-map TODO) is orthogonal and
  unaddressed: a generator can target a phantom event merchant exactly as a
  hand-written workflow can, until that TODO lands.

## 7. Usage gallery (what each entry point looks like)

1. **CLI, parameterized:** `artifacts plan fennel/workflows/farm.fnl nillinbot
   target=iron_rocks` → per-character feasibility ("needs mining 10, have 4"
   as a blocker) without running. Swap `plan`→`run` to execute.
2. **CLI, generated:** `artifacts run fennel/workflows/craft.fnl nillinbot
   item=iron_sword qty=1` → withdraw 3 iron_ore from bank, mine 3 more
   (chunked if needed), smelt at mining workshop, craft at weaponcrafting,
   done. `plan` first prints the whole predicted itinerary with loop counts.
3. **TUI:** select `craft` in the workflow list → param form (item completion
   from recipe outputs) → skeleton renders the generated groups → live cursor
   as today. (M6.)
4. **Fennel, composed:** a `daily.fnl` that groups `farm` (params:
   ash_tree), then `craft` (params: cooked_gudgeon ×10), then a bank deposit —
   twelve lines, all `use_workflow` calls.
5. **Achievements** (future spike): criterion table → acquire/fight emitters;
   this design's deliverable to that one is `acquire.item`, `use_workflow`,
   and `host.item_sources`.

## 8. Build plan — milestones, each independently shippable

Dependency shape: M1 → M2 → (M3 → M4 → M5) → M6; M2 and M3 are independent of
each other. Every milestone: update `docs/ARCHITECTURE.md` alongside, tests
per the entrypoint-driven standard (drive `planner::plan` / `workflow::load` /
CLI-shaped fixtures, not per-function micro-tests), `/code-complete` before PR.

### M1 — Protocol & parameters
- New `fennel/lib/params.fnl` (`validate_schema`, `coerce`) — §2.3.
- New `src/workflow.rs` (`load`, `schema`, `ParamSpec`) — §2.4; `setup_lua`
  seeds params.fnl as a module like the other three libs.
- `planner::plan` + `live::run_workflow` + CLI `k=v` parsing + TUI
  `spawn_tui_run` signature changes — §2.5. CLI param errors print the schema.
- Migrate all three workflows to the protocol: `farm-copper` becomes
  `farm.fnl` (`:target` of type `:resource`), `farm-chickens` becomes
  `hunt.fnl` (`:target` of type `:monster` — a separate file, not an enum
  switch: the fight loop's rest gate makes the bodies genuinely different);
  `buy-from-merchant` keeps `:item`/`:price` params until M3 makes price
  derivable from the NPC catalog.
- Tests: plan a parameterized workflow through the public entry points;
  coercion/validation error cases via `workflow::load`.

### M2 — Composition
- `workflows.<name>` package searcher + `LuaSetupOptions.workflows_root` — §3.1.
- `:group` node in `interp.fnl` (plan/run/skeleton) + `group` constructor +
  TUI row rendering; `use_workflow` helper wrapping require+coerce+build+group.
- Example composed workflow committed as living documentation.
- Tests: a composed workflow plans/runs through existing entry points;
  skeleton shows grouped depths.

### M3 — World data & model-surface fixes
- `ResourceView` drops+skill, cache version bump, gather `:sim` fix + skill
  gate — §5.1. **This changes existing plan outputs; call it out in the PR.**
- Skills: `CharacterView` fields, `predicate_state`/`PlanSeed`/`state-eq`,
  craft gate, `skill_at_least` — §5.2.
- Inventory on the live surface via `predicate_state`; `has_item` — §5.3–5.4.
- `find_tile` anchor arg — §5.7.
- `NpcItemData` + driver fetch + cache — §5.5 (data only; index lands in M5).
- Tests: gather-then-craft workflow plans with true item codes; a
  skill-gated gather produces a blocker; `has_item` loop terminates in plan
  and evaluates on a live-shaped view (MockDriver).

### M4 — Holdings
- `fetch_bank_items`, `BankData`, `LuaSetupOptions.bank`, `ctx.bank`,
  `host.bank()` — §5.6. `load_live_context` fetches it.
- M4b (optional, separate PR, only on demonstrated need): bank on the mutable
  model state + deposit/withdraw sims + `state-eq` clause.

### M5 — The acquire generator
- `SourceIndex` in Rust + `host.item_sources` — §5.5.
- `fennel/lib/acquire.fnl` — §4.1 (policy, chunking, no-route errors).
- `fennel/workflows/craft.fnl` — §4.2.
- Tests: fixture reference data (small recipe DAG, one resource, one monster,
  one npc) → `craft.fnl` plans feasible with expected groups/loops; no-route
  and over-budget error shapes; a live integration run of a one-level craft
  (per `live-integration-test` skill) as the end-to-end proof.

### M6 — TUI param form
- Marshal `WorkflowInfo` for the selected workflow; form widget (text input +
  type-aware completion per §2.2 table; validation errors inline); wire
  submitted params through `spawn_tui_run`.
- The workflow list shows `:doc` and a param hint; running a
  required-params workflow opens the form instead of running immediately.
- Tests: reducer-level (form state), plus the `tui-iteration` skill for a
  scripted end-to-end drive of `craft` from the form.

## 9. Deferred extension (specified now, built when needed): late-bound args

If a hand-written workflow ever needs an action argument computed from
*evolving* state ("deposit exactly what this loop gathered"), the mechanism
is: an action arg that is a function is called with the current state (model
state in plan, `host.view` in run) at each evaluation, by the interpreters,
before invoking `:cost`/`:sim`/`:run`; the skeleton renders such args as `?`.
Purity contract identical to predicates. This is deliberately **not** in
M1–M6: generators compute concrete values at build time (§4.1), `deposit-all`
covers the common case, and adding laziness to the AST costs introspectability
— so it waits for a concrete workflow that can't be written without it.

## 10. Open questions (all safe to defer past M6)

- Should `plan` output price the acquire policy's NPC purchases as a gold
  delta line? (Nice; purely additive to `PlanResult`.)
- Multi-character `daily.fnl` orchestration — needs a scheduler design of its
  own; nothing here blocks it.
- Achievement criterion coverage — belongs to `plans/ACHIEVEMENTS.md`; §4.3
  lists the seams it consumes.
