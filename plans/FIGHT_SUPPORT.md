# Spike: Fight Support

**Status:** design spike — no code in this change. Output of the "Spike: Fight Support" TODO.

**Goal:** identify everything that has to land for combat to be a first-class, scriptable workflow primitive — alongside `gather` and `travel-to` — covering the four jobs the TODO calls out: _identify_ fight options, _plan_ which fights are feasible (and at what win chance), _execute_ the fight, and _claim rewards_ — plus the cross-cutting concern of using inventory/bank items and equipment to push the win chance up.

This document is a map of the gap, not an implementation. It says what each layer ([authoring → host bridge → planner/runtime → core](../docs/ARCHITECTURE.md)) is missing, what the combat model actually is, and the order to build it in. Read [`docs/ARCHITECTURE.md`](../docs/ARCHITECTURE.md) first — the rules there (one action defined once as `{:cost :sim :run}`; one state surface via `predicate_state`; game rules go in pure `core/`) are the constraints this plan has to satisfy.

## TL;DR — the shape of the work

`fight` already exists end-to-end as a _live_ action: `Intent::Fight` → `POST action/fight` ([`core/src/machine.rs:181`](../core/src/machine.rs)), the response is parsed into `OutcomeKind::Fight(FightResult)` ([`core/src/machine.rs:269`](../core/src/machine.rs)), `host.fight` is wired ([`src/lua.rs:222`](../src/lua.rs)), and the `:fight` action's `:run` calls it ([`fennel/lib/actions.fnl:114`](../fennel/lib/actions.fnl)). So you _can_ already script "punch the thing in front of me."

What's missing is everything that makes fighting **plannable and safe**:

1. **No combat model.** `:fight`'s `:cost` is a flat 5-turn guess and its `:sim` is the identity function — the plan pass can't tell a winnable fight from a fatal one, can't decrement HP, and can't add drops to the model inventory. Combat is the first action whose outcome is _state-dependent_ (it depends on the character's and the monster's stats), so it's the first action that needs real game data in the plan pass.
2. **No monster/item data.** There is no monster data model, no way to find monster tiles on the map, and `CharacterView` carries none of the combat stats (attack/resistance per element, equipped gear) that decide a fight.
3. **No feasibility/recommendation output.** The plan pass can flag inventory overflow today; it can't yet say "you'd lose this fight" or "equip X and rest first and you'd win at 92%."
4. **No pre-/post-fight choreography.** Equip/Unequip intents exist in `core` but have no Fennel actions or host fns; there's no rest-to-heal loop, and no "make inventory room for drops before engaging" step.

The bulk of the effort is **#1 and #2** — porting Artifacts' combat resolution into pure `core/` and feeding it monster + character + item stats. Everything else is plumbing that follows the existing patterns.

## Background: how Artifacts combat actually resolves

These mechanics are **confirmed** against the live docs ([Combat & Stats](https://docs.artifactsmmo.com/concepts/stats_and_fights)) and the live API (`GET /monsters/chicken`, character `nillinbot`) — see [Confirmed mechanics](#confirmed-mechanics-checked-against-docs--live-api) for the full check and the data shapes. Encoded here as the model the simulator must reproduce; the source for the formula is that docs page.

A fight is **turn-based and deterministic except for critical strikes**. Player and monster each have `hp`, `initiative`, a `critical_strike` percent, four **attack** stats (`attack_fire/earth/water/air`) and four **resistance** stats (`res_fire/earth/water/air`); the player additionally has percent **damage** boosts (`dmg_*`, from gear).

Per element, damage resolves as (quoting [Combat & Stats](https://docs.artifactsmmo.com/concepts/stats_and_fights)):

```
Elemental attack = Round(Base elemental attack × (1 + Total damage bonus / 100))
Final damage     = Round(Elemental attack × (1 - Resistance / 100))
Critical damage  = Round(Final damage × 1.5)          ; on a critical strike
```

where `Total damage bonus = global damage + that element's dmg%` (1 point = 1% extra), each point of resistance reduces by 1%, and `1 critical_strike = 1% crit chance`. The four elements are computed independently and summed. **Turn order is decided by `initiative`** (highest acts first; tie → higher HP; tie → random) — _not_ always the player. Fights cap at **100 turns**; reaching the cap is a **loss**, and on any loss the character **respawns at its spawn tile with 1 HP** — so a misjudged fight doesn't merely waste time, it teleports the character and leaves it nearly dead (which is why live loss-bail below matters). Drops, XP, and gold are awarded only on a win and already come back in the `fight` response (`turns`, `result`, `xp`, `gold`, `drops` — see [`core/src/machine.rs:240`](../core/src/machine.rs)).

**The only stochastic input to the outcome is the critical strike.** We commit to a single approach:

- **Deterministic simulation, crits off.** If the player wins this, the fight is _guaranteed_ winnable — crits only ever help the player here, so a crit-off win is a true lower bound. It's cheap, reproducible, and a safe gate for "should the bot engage." This is _the_ feasibility signal.

A probabilistic **Monte Carlo win-%** (sampling crits to grade the marginal fights the deterministic pass rejects) is **explicitly out of scope** for this work. The deterministic gate is the contract; pursue Monte Carlo only if marginal fights later prove worth chasing.

This matters for the design: because the resolution is a pure function of two stat blocks, **the combat simulator belongs in `core/` as a pure function**, exposed to the plan pass through a `host.*` formula like `cooldown_cost` and `path_hops`. The simulator itself is pure and unit-testable against fixtures; the stat _data_ it consumes is fetched and cached (see [data loading](#planner-and-runtime)).

## What each layer needs

### `core/` — the pure combat brain (the load-bearing piece)

New module, e.g. `core/src/combat.rs`:

- `struct CombatStats { hp, initiative, attack: [i32;4], res: [i32;4], dmg: [i32;4], critical_strike }` — one shape for both player and monster. `initiative` decides turn order, so it's part of the block, not an afterthought.
- `struct MonsterView { code, level, hp, attack/res per element, initiative, ... }` and a map-content notion of **where** monsters are (see map below).
- `fn simulate(player: &CombatStats, monster: &CombatStats) -> FightPrediction` returning `{ result: Win|Lose, turns, player_hp_remaining, ... }`. Deterministic with crits off — the agreed feasibility signal, no RNG path (Monte Carlo is out of scope).
- `cooldown::formulas::fight(turns)` is `2s × turns` today, but the live formula also factors **haste** (`cooldown = turns×2 − (haste×0.01)×(turns×2)`, 5s floor). Fold haste in here so the predicted cost matches the server, and feed it the simulator's _real_ turn count instead of the flat 5.

Extend `core/src/map.rs` (or a sibling) so map tiles can carry **content** (`{type: "monster", code: "chicken"}`), enabling "find the nearest tile with monster X." Today `map.rs` only does pathfinding over coordinates.

Extend `CharacterView` ([`core/src/step.rs:110`](../core/src/step.rs)) with the combat stats and equipped slots, so a live character can be turned into `CombatStats`. The `Slot` enum already enumerates every equipment slot ([`core/src/step.rs:34`](../core/src/step.rs)).

Keep all of this serde/thiserror-only — it's the whole reason `core` is a separate crate.

### Host bridge — `src/lua.rs`

New host fns (registered always, like `cooldown_cost`), backing the `:cost`/`:sim` facets. The _computation_ they wrap is pure; the stat data they read comes from a TTL-cached dataset (see [data loading](#planner-and-runtime)) — the plan pass is allowed to touch the network to populate that cache:

- `host.monster_stats(code) -> {hp, initiative, attack_*, res_*, ...}` — monster stat block for the simulator, read from the cached `/monsters` dataset.
- `host.item_stats(code) -> {slot, attack_*, res_*, dmg_*, ...}` — read from the cached `/items` dataset, so equipment planning can compute the stat delta of swapping gear.
- `host.simulate_fight(player_stats, monster_stats) -> {result, turns, hp_remaining}` — thin wrapper over `core::combat::simulate`, the predicate/sim pass calls this.
- `host.monster_tile(code) -> {x, y, level}` — nearest monster tile, for `travel-to` targeting (mirrors how the gather tile is modelled).

The model-state surface (`predicate_state`, [`src/lua.rs:79`](../src/lua.rs)) gains the combat fields (`hp`, `max-hp` already exist; add `attack-*`/`res-*`/equipped gear) so combat predicates read the same shape in plan and run — the same drift-prevention discipline the doc already enforces.

Run-only host fns to add (the live counterparts already half-exist):

- `host.equip(code, slot)` / `host.unequip(slot)` — `Intent::Equip`/`Intent::Unequip` and the `action/equip`/`action/unequip` requests already exist in `core` ([`core/src/machine.rs:187`](../core/src/machine.rs)); they just need `Character` methods (`character.rs` already has `equip`) surfaced as host fns.
- `host.withdraw_item(code, qty)` — `Intent::WithdrawItem` exists in `core`; needed to pull a better weapon/consumable out of the bank before a fight.

### Authoring — `fennel/lib/`

**Rewrite `:fight` in `actions.fnl`** so all three facets are real (the file's central invariant — [`fennel/lib/actions.fnl:1`](../fennel/lib/actions.fnl)):

- `:cost` — `host.simulate_fight` → use the predicted `turns` in `cooldown_cost :fight {:turns ...}` instead of the flat 5.
- `:sim` — apply the prediction to the model state: subtract predicted HP loss and, on a predicted win, add the _expected_ drops to the model inventory via the existing `inv-add` helper. Drops are **probabilistic** (each entry has a `rate` = 1-in-N chance plus min/max quantity, scaled by the character's `prospecting`), so the expected yield is fractional — treat inventory overflow from drops as a **risk warning**, not the hard blocker that a deterministic overflow (e.g. a fixed gather count) is. A predicted loss, by contrast, _is_ a hard blocker (see below).
- `:run` — unchanged (`host.fight`).

**New actions:** `:equip` / `:unequip` (wrap the new host fns), `:withdraw-item`. Each with the full `{:cost :sim :run}` trio.

**New predicates** in `predicates.fnl` (read model state; work in plan and run alike):

- `winnable? [monster-code st]` — `host.simulate_fight` says Win. The feasibility gate.
- `hp-full? [st]` and the existing `hp-below?` — drive the rest-before-fight loop.

**New blocker** in the plan pass (`interp.fnl`): when `:fight`'s prediction is a loss, call `acc-add-blocker` ("would lose fight vs <monster> with current stats") — exactly the mechanism inventory-overflow already uses ([`fennel/lib/interp.fnl:75`](../fennel/lib/interp.fnl)). This is how planning "recommends what's needed to get success rate up": the blocker names the failing fight, and (stretch) the planner can try equipment permutations from inv+bank and report the cheapest loadout that flips it to winnable.

A reference workflow `fennel/workflows/farm-chickens.fnl` (the combat analogue of `farm-copper.fnl`): rest to full → ensure inventory room → travel to monster tile → `repeat-until inventory-full?` (rest if `hp-below?`, then fight) → bank drops.

### Planner and runtime

- `PlanSeed` ([`src/planner.rs`](../src/planner.rs)) and `PlanResult` grow combat fields: seed the player's combat stats + a target monster from the live character (`PlanSeed::from_view`), and surface the per-fight win prediction in the result so the CLI can print "fight feasible: yes/no, ~N turns."
- **Data loading — a TTL'd disk cache, not vendored data.** Monster and item stats (`/monsters`, `/items`) are static-ish reference data that changes rarely, so fetch them and **cache to disk outside version control with a ~1-day TTL** (e.g. under the OS cache dir or `target/`, git-ignored); refetch when the cache is missing or stale. Do **not** vendor a snapshot into the repo — it would go stale silently and bloat the tree. The plan pass is permitted to hit the network to (re)populate this cache; `host.monster_stats`/`host.item_stats` read from it. This keeps the simulator pure while keeping the dataset current with near-zero ongoing network cost.
- Runtime is mostly there: the scheduler already executes `Intent::Fight` and parses the `FightResult`. The gap is **choreography**, which is authored in Fennel (rest/equip/withdraw around the fight), not new runtime code. One safety addition is worth making in the live loop: treat a `FightOutcome::Lose` as a hard stop/bail rather than looping — a loss respawns the character at its spawn tile with **1 HP**, so blindly retrying a misjudged fight death-spirals (1-HP loss → instant re-loss).

## The four jobs, mapped

| TODO job | Where it lands |
| --- | --- |
| **Identify fight options** | `map.rs` tile content + `host.monster_tile` / `host.monster_stats`; "what can I fight near me / at all." |
| **Plan feasibility & win chance** | `core::combat::simulate` (deterministic crit-off gate — Monte Carlo out of scope) via `host.simulate_fight`; `winnable?` predicate; loss → `acc-add-blocker`. |
| **Recommend how to raise win %** | plan pass tries gear from inventory+bank (`host.item_stats` deltas) and reports the loadout/rest that flips a blocked fight to winnable. |
| **Execute** | already works: `:fight` `:run` → `host.fight` → `Intent::Fight`; add equip/withdraw/rest choreography actions + live loss-bail. |
| **Claim rewards** | drops return in the fight response automatically; the work is _making room_ — `:fight` `:sim` adds drops so the plan catches overflow, and the workflow deposits/withdraws around capacity. |

## Build order (suggested)

1. **`core::combat::simulate` + fixtures.** Pure, deterministic (crits off), unit-tested against known monster/character matchups. Nothing else can be trusted until this matches the live server — validate by comparing `simulate` against real `fight` responses for a few monsters (the live-test harness already exists; `nillinbot` vs `chicken` is a ready first fixture).
2. **Monster/item data + host fns** (`monster_stats`, `item_stats`, `simulate_fight`) behind the TTL disk cache.
3. **Real `:fight` `:cost`/`:sim`** (haste-aware cost, expected-drop sim) + the loss blocker + `winnable?` predicate. Now `plan` tells the truth about combat.
4. **Equip/withdraw/rest actions** + a `farm-chickens.fnl` workflow + live loss-bail.
5. **(stretch) Equipment recommendation** — try gear permutations from inventory+bank and report the cheapest loadout that flips a blocked fight to winnable.

Steps 1–3 deliver the core value (you can _plan_ a fight honestly); 4 makes a real farming loop; 5 is optimization. (Monte Carlo win-% is out of scope entirely — see [the combat model](#background-how-artifacts-combat-actually-resolves).)

## Confirmed mechanics (checked against docs + live API)

The questions a spike would normally leave open were resolved now, against [Combat & Stats](https://docs.artifactsmmo.com/concepts/stats_and_fights) and the live API using the `nillinbot` test character. Findings (these are the rules the implementation must match):

- **Damage formula** — verified, quoted in [the combat model](#background-how-artifacts-combat-actually-resolves): `Elemental attack = Round(base × (1 + total_dmg%/100))`, `Final = Round(attack × (1 − res/100))`, crit `= Round(Final × 1.5)`. `1 dmg = +1%`, `1 res = −1%`, `1 critical_strike = 1% crit chance`. Elements are independent and summed.
- **Turn order** — by `initiative` (highest first; tie → higher HP; tie → random), **not** player-first. Confirmed live: `chicken` has `initiative: 50`; characters carry their own. The simulator must read `initiative` from both sides.
- **Turn cap** — **100 turns**; reaching it is a **loss**, and a loss respawns the character at its spawn tile with **1 HP**. This is why loss must be a hard bail at runtime.
- **Drops are probabilistic** — confirmed live: `GET /monsters/chicken` returns drops as `{code, rate, min_quantity, max_quantity}` (e.g. `egg` at `rate: 12` ⇒ ~1-in-12 per win). The `prospecting` stat boosts drop rate (+0.1%/point). So the `:sim` uses _expected_ drops and treats overflow as a risk, not a hard blocker.
- **Monster stats are static reference data** — fetched from `/monsters` (and item stats from `/items`); they don't vary per character or per fight, which is what makes the ~1-day TTL disk cache the right call (no vendoring).
- **Other live stats noted for later** — `haste` reduces the fight cooldown (folded into the cost formula above), `wisdom` boosts XP (+0.1%/pt), `prospecting` boosts drops; none change win/lose, so the deterministic simulator can ignore them except where noted.

## References

- [`docs/ARCHITECTURE.md`](../docs/ARCHITECTURE.md) — the layering and invariants this plan must respect.
- Combat existing groundwork: [`core/src/step.rs`](../core/src/step.rs) (`FightResult`/`FightOutcome`/`Slot`), [`core/src/machine.rs`](../core/src/machine.rs) (request + response parsing), [`core/src/cooldown.rs`](../core/src/cooldown.rs) (`fight` formula), [`fennel/lib/actions.fnl`](../fennel/lib/actions.fnl) (`:fight` stub).
- **Combat formula & stats (the source for the damage/turn/initiative rules above):** [Combat & Stats — docs.artifactsmmo.com/concepts/stats_and_fights](https://docs.artifactsmmo.com/concepts/stats_and_fights).
- Game API (shapes confirmed live for this spike via the `nillinbot` token): Gameplay docs — actions: https://docs.artifactsmmo.com/concepts/actions/ ; OpenAPI spec: https://api.artifactsmmo.com/openapi.json ; usage guide: https://docs.artifactsmmo.com/
</content>

</invoke>
