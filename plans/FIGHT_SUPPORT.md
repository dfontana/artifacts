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

(Verify against the live API before encoding — see [References](#references). Encoded here as the model the simulator must reproduce.)

A fight is **turn-based and almost entirely deterministic**. Player and monster each have:

- `hp`
- four **attack** stats: `attack_fire`, `attack_earth`, `attack_water`, `attack_air`
- four **resistance** stats: `res_fire`, `res_earth`, `res_water`, `res_air`
- a **critical strike** chance (`critical_strike`, percent)
- the player additionally has percent **damage** boosts per element (`dmg_fire`, …) from gear

Per attacking turn, for each element the attacker deals roughly:

```
base   = attack_element * (1 + dmg_element/100)
dealt  = base * (1 - target.res_element/100)      ; resistance is a percent reduction
        (×1.5 on a critical strike)
```

summed over the four elements, floored, minimum damage applies. The player strikes first; combatants alternate until one reaches 0 HP or a **turn cap** (~50–100 turns) is hit — hitting the cap counts as a **loss**. Drops, XP, and gold are awarded only on a win and already come back in the `fight` response (`turns`, `result`, `xp`, `gold`, `drops` — see [`core/src/machine.rs:240`](../core/src/machine.rs)).

**The only stochastic input is the critical strike.** That gives us two honest ways to report "success chance":

- **Deterministic lower bound** — simulate with crits off. If the player wins this, the fight is _guaranteed_ winnable. Cheap, no RNG, and a safe gate for "should the bot engage." Recommend this as the v1 feasibility signal.
- **Monte Carlo** — run the turn loop N times sampling crits, report win %. Needed only for the marginal fights the deterministic pass calls losses; defer to v2.

This matters for the design: because the resolution is a pure function of two stat blocks, **the combat simulator belongs in `core/` as a pure function**, exposed to the plan pass through a `host.*` formula exactly like `cooldown_cost` and `path_hops` are today. No I/O, fully unit-testable against known fixtures.

## What each layer needs

### `core/` — the pure combat brain (the load-bearing piece)

New module, e.g. `core/src/combat.rs`:

- `struct CombatStats { hp, attack: [i32;4], res: [i32;4], dmg: [i32;4], critical_strike }` — one shape for both player and monster.
- `struct MonsterView { code, level, hp, attack/res per element, ... }` and a map-content notion of **where** monsters are (see map below).
- `fn simulate(player: &CombatStats, monster: &CombatStats, opts) -> FightPrediction` returning `{ result: Win|Lose, turns, player_hp_remaining, ... }`. Deterministic (crits off) by default; an `opts.sample_crits` path enables Monte Carlo later.
- A `combat_cooldown(turns)` already exists as `cooldown::formulas::fight` — keep it, but the simulator now supplies the _real_ turn count instead of the flat 5.

Extend `core/src/map.rs` (or a sibling) so map tiles can carry **content** (`{type: "monster", code: "chicken"}`), enabling "find the nearest tile with monster X." Today `map.rs` only does pathfinding over coordinates.

Extend `CharacterView` ([`core/src/step.rs:110`](../core/src/step.rs)) with the combat stats and equipped slots, so a live character can be turned into `CombatStats`. The `Slot` enum already enumerates every equipment slot ([`core/src/step.rs:34`](../core/src/step.rs)).

Keep all of this serde/thiserror-only — it's the whole reason `core` is a separate crate.

### Host bridge — `src/lua.rs`

New **pure** host fns (registered always, like `cooldown_cost`), backing the `:cost`/`:sim` facets so the plan pass needs no network:

- `host.monster_stats(code) -> {hp, attack_*, res_*, ...}` — monster stat block for the simulator. Source: a monster dataset (fetched once and cached; see data-loading note below).
- `host.item_stats(code) -> {slot, attack_*, res_*, dmg_*, ...}` — so equipment planning can compute the stat delta of swapping gear.
- `host.simulate_fight(player_stats, monster_stats) -> {result, turns, hp_remaining}` — thin wrapper over `core::combat::simulate`, the predicate/sim pass calls this.
- `host.monster_tile(code) -> {x, y, level}` — nearest monster tile, for `travel-to` targeting (mirrors how the gather tile is modelled).

The model-state surface (`predicate_state`, [`src/lua.rs:79`](../src/lua.rs)) gains the combat fields (`hp`, `max-hp` already exist; add `attack-*`/`res-*`/equipped gear) so combat predicates read the same shape in plan and run — the same drift-prevention discipline the doc already enforces.

Run-only host fns to add (the live counterparts already half-exist):

- `host.equip(code, slot)` / `host.unequip(slot)` — `Intent::Equip`/`Intent::Unequip` and the `action/equip`/`action/unequip` requests already exist in `core` ([`core/src/machine.rs:187`](../core/src/machine.rs)); they just need `Character` methods (`character.rs` already has `equip`) surfaced as host fns.
- `host.withdraw_item(code, qty)` — `Intent::WithdrawItem` exists in `core`; needed to pull a better weapon/consumable out of the bank before a fight.

### Authoring — `fennel/lib/`

**Rewrite `:fight` in `actions.fnl`** so all three facets are real (the file's central invariant — [`fennel/lib/actions.fnl:1`](../fennel/lib/actions.fnl)):

- `:cost` — `host.simulate_fight` → use the predicted `turns` in `cooldown_cost :fight {:turns ...}` instead of the flat 5.
- `:sim` — apply the prediction to the model state: subtract predicted HP loss, and on a predicted win add the (expected) drops to the model inventory via the existing `inv-add` helper. This is what lets the plan pass catch "inventory has no room for the drop" and "this fight drops you below 0 HP."
- `:run` — unchanged (`host.fight`).

**New actions:** `:equip` / `:unequip` (wrap the new host fns), `:withdraw-item`. Each with the full `{:cost :sim :run}` trio.

**New predicates** in `predicates.fnl` (read model state; work in plan and run alike):

- `winnable? [monster-code st]` — `host.simulate_fight` says Win. The feasibility gate.
- `hp-full? [st]` and the existing `hp-below?` — drive the rest-before-fight loop.

**New blocker** in the plan pass (`interp.fnl`): when `:fight`'s prediction is a loss, call `acc-add-blocker` ("would lose fight vs <monster> with current stats") — exactly the mechanism inventory-overflow already uses ([`fennel/lib/interp.fnl:75`](../fennel/lib/interp.fnl)). This is how planning "recommends what's needed to get success rate up": the blocker names the failing fight, and (stretch) the planner can try equipment permutations from inv+bank and report the cheapest loadout that flips it to winnable.

A reference workflow `fennel/workflows/farm-chickens.fnl` (the combat analogue of `farm-copper.fnl`): rest to full → ensure inventory room → travel to monster tile → `repeat-until inventory-full?` (rest if `hp-below?`, then fight) → bank drops.

### Planner / runtime — `src/planner.rs`, `src/live.rs`

- `PlanSeed` ([`src/planner.rs`](../src/planner.rs)) and `PlanResult` grow combat fields: seed the player's combat stats + a target monster from the live character (`PlanSeed::from_view`), and surface the per-fight win prediction in the result so the CLI can print "fight feasible: yes/no, ~N turns."
- The **data-loading question** (the one real new I/O concern): monster and item stats must come from somewhere for the offline plan. Options, cheapest first: (a) bundle a static snapshot of `/monsters` and `/items` as a vendored JSON fixture loaded into `core` (matches the "plan needs no network" property, goes stale); (b) fetch-and-cache on the `plan <wf> <character>` path (already does two fetches — character + map); (c) hybrid: vendored default, refreshed on the live path. **Recommend (c).**
- Runtime is mostly there: the scheduler already executes `Intent::Fight` and parses the `FightResult`. The gap is **choreography**, which is authored in Fennel (rest/equip/withdraw around the fight), not new runtime code. One safety addition worth making in the live loop: treat a `FightOutcome::Lose` as a stop/bail signal rather than blindly looping, so a misprediction can't death-spiral.

## The four jobs, mapped

| TODO job | Where it lands |
| --- | --- |
| **Identify fight options** | `map.rs` tile content + `host.monster_tile` / `host.monster_stats`; "what can I fight near me / at all." |
| **Plan feasibility & win chance** | `core::combat::simulate` (deterministic gate; Monte Carlo later) via `host.simulate_fight`; `winnable?` predicate; loss → `acc-add-blocker`. |
| **Recommend how to raise win %** | plan pass tries gear from inventory+bank (`host.item_stats` deltas) and reports the loadout/rest that flips a blocked fight to winnable. |
| **Execute** | already works: `:fight` `:run` → `host.fight` → `Intent::Fight`; add equip/withdraw/rest choreography actions + live loss-bail. |
| **Claim rewards** | drops return in the fight response automatically; the work is _making room_ — `:fight` `:sim` adds drops so the plan catches overflow, and the workflow deposits/withdraws around capacity. |

## Build order (suggested)

1. **`core::combat::simulate` + fixtures.** Pure, deterministic, unit-tested against known monster/character matchups. Nothing else can be trusted until this matches the live server — validate by comparing `simulate` against real `fight` responses for a few monsters (the live-test harness already exists).
2. **Monster/item data into `core` + host fns** (`monster_stats`, `item_stats`, `simulate_fight`), vendored-snapshot first.
3. **Real `:fight` `:cost`/`:sim`** + the loss blocker + `winnable?` predicate. Now `plan` tells the truth about combat.
4. **Equip/withdraw/rest actions** + a `farm-chickens.fnl` workflow + live loss-bail.
5. **(v2) Equipment recommendation** and **Monte Carlo win %** for marginal fights.

Steps 1–3 deliver the core value (you can _plan_ a fight honestly); 4 makes a real farming loop; 5 is optimization.

## Open questions / things to confirm against the live API

- Exact damage formula, resistance scaling/cap, and the critical-strike multiplier — encode only after verifying against [the combat docs and OpenAPI spec](#references).
- The turn cap and what a cap-out counts as (assumed loss here).
- Whether monster stats are static enough to vendor, or vary by event/server state.
- Drop tables: the `:sim` pass needs _expected_ drops to predict inventory pressure — confirm whether drops are deterministic-on-win or probabilistic (affects whether overflow is a hard blocker or a risk).

## References

- [`docs/ARCHITECTURE.md`](../docs/ARCHITECTURE.md) — the layering and invariants this plan must respect.
- Combat existing groundwork: [`core/src/step.rs`](../core/src/step.rs) (`FightResult`/`FightOutcome`/`Slot`), [`core/src/machine.rs`](../core/src/machine.rs) (request + response parsing), [`core/src/cooldown.rs`](../core/src/cooldown.rs) (`fight` formula), [`fennel/lib/actions.fnl`](../fennel/lib/actions.fnl) (`:fight` stub).
- Game API (verify formulas/shapes here, not from this repo's assumptions): Gameplay docs — actions & combat: https://docs.artifactsmmo.com/concepts/actions/ ; OpenAPI spec: https://api.artifactsmmo.com/openapi.json ; usage guide: https://docs.artifactsmmo.com/
</content>

</invoke>
