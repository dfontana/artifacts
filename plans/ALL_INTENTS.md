# Spike: All other intents

**Goal:** every character action the Artifacts API exposes is reachable from a
Fennel workflow, wired end to end (Fennel → `src/lua.rs` host fn → `Character` →
`Scheduler`/`Core` → `wire.rs` request/outcome), and exercised by a cross-cutting
test. This unblocks player scripting of the whole action surface.

## What was missing

The API exposes 27 `POST /my/{name}/action/*` endpoints. Before this spike, 6
were wired as intents: `move`, `gathering`, `fight`, `rest`,
`bank/deposit/item`, `bank/withdraw/item`. The other **21** existed only as raw
endpoints with no intent, no host fn, and no Fennel action — unreachable from a
workflow. This spike adds all 21.

## The 21 new intents

Grouped by family. "Fennel" is the action name a workflow author writes; "cost"
is the plan-pass cooldown formula (`host.cooldown_cost` op); "sim" is how the
plan pass advances the model inventory.

| Fennel action | Endpoint | Request body | cost | sim |
| --- | --- | --- | --- | --- |
| `craft` | `action/crafting` | `{code, quantity}` | `craft` = 5s/item | +output item |
| `recycle` | `action/recycling` | `{code, quantity}` | `recycle` = 3s/item | −recycled item |
| `use-item` | `action/use` | `{code, quantity}` | `simple` = 3s | −item |
| `delete-item` | `action/delete` | `{code, quantity}` | 3s | −item |
| `equip` | `action/equip` | `[{code, slot, quantity}]` | 3s | neutral |
| `unequip` | `action/unequip` | `[{slot, quantity}]` | 3s | neutral |
| `deposit-gold` | `action/bank/deposit/gold` | `{quantity}` | 3s | neutral |
| `withdraw-gold` | `action/bank/withdraw/gold` | `{quantity}` | 3s | neutral |
| `give-gold` | `action/give/gold` | `{quantity, character}` | 3s | neutral |
| `give-item` | `action/give/item` | `{items:[{code, quantity}], character}` | 3s/type | −item |
| `npc-buy` | `action/npc/buy` | `{code, quantity}` | 3s | +item |
| `npc-sell` | `action/npc/sell` | `{code, quantity}` | 3s | −item |
| `ge-buy` | `action/grandexchange/buy` | `{id, quantity}` | 3s | neutral |
| `ge-cancel` | `action/grandexchange/cancel` | `{id}` | 3s | neutral |
| `ge-fill` | `action/grandexchange/fill` | `{id, quantity}` | 3s | neutral |
| `task-new` | `action/task/new` | — | 3s | neutral |
| `task-complete` | `action/task/complete` | — | 3s | neutral |
| `task-cancel` | `action/task/cancel` | — | 3s | neutral |
| `task-exchange` | `action/task/exchange` | — | 3s | neutral |
| `task-trade` | `action/task/trade` | `{code, quantity}` | 3s | −item |
| `transition` | `action/transition` | — | 3s | neutral |

Cooldown formulas are from the [actions concept
doc](https://docs.artifactsmmo.com/concepts/actions/): crafting is 5s per item,
recycling 3s per item, banking/gifting 3s per distinct item type, and the rest
share a flat 3s (`formulas::simple`). The server's returned `expiration` is
authoritative at run time; these client-side formulas only feed the offline
plan.

## Where each layer changed (the add-an-intent checklist)

This followed the checklist in `docs/ARCHITECTURE.md` verbatim, once per intent:

1. **`core/src/wire.rs`** — a wire struct + `IntentWire` impl (request builder +
   outcome parser), an `Intent` enum variant, and a `wire()` dispatch arm. The
   enum's `strum::EnumIter` + the exhaustive `wire()` match make an unwired
   variant a compile error.
2. **`core/src/step.rs`** — new `OutcomeKind` variants only where the shape is
   new (see below).
3. **`src/character.rs`** — a blocking wrapper method per intent.
4. **`src/lua.rs`** — a `register_intent` match arm binding the run host fn, plus
   three new `cooldown_cost` ops (`craft`, `recycle`, `simple`). The exhaustive
   match + derived `Intent::iter()` again make an unbound intent a compile error,
   never a silent nil-call.
5. **`fennel/lib/actions.fnl`** — a `def-action` with all three of `:cost`,
   `:sim`, `:run`, asserted present at load time.

### OutcomeKind consolidation

Run host fns discard the outcome and read state through `host.view`, and the
`character` snapshot in every action response refreshes `SharedView`
regardless — so an intent's `OutcomeKind` is observability, and **nothing in the
tree branches on the new actions' outcomes**. All 21 therefore share a single
new variant, `OutcomeKind::Action { items }` (any `details.items` the response
reported; empty otherwise), produced by `IntentWire::outcome`'s **default
method** — so the 21 wire structs define only `request`. Minting per-family
variants (`Craft`/`Recycle`/`Equipment`/`GoldTransaction`) was tried and
dropped: with no consumer it was arbitrary (why name craft but not npc-buy?).
If a human-facing label is ever needed, it belongs as a data field on `Action`,
not as fresh enum arms no code matches.

## Modelling limitations (deliberate, documented)

The plan pass is only as accurate as the reference data loaded client-side, and
recipes / GE order books / task definitions / equipment layout / gold are **not**
loaded. So:

- `craft`'s `:sim` adds the crafted output but does **not** consume recipe inputs
  (unknown); `recycle` removes the recycled item but doesn't add salvage.
- Gold actions and equipment are neutral in `:sim` (gold and equipment aren't on
  the model-state surface; see `predicate_state` in `src/lua.rs`).
- GE / task-board actions are neutral (their effects aren't resolvable offline).

A wrong assumption (missing inputs, empty pack, bad order id) fails **loudly at
run time** via a server error — never silently in the plan. Run-pass behaviour is
fully functional for all 21; only the offline cost/feasibility prediction is
coarse for the actions above.

### Follow-ups this spike surfaced (not done here)

- **Gold on the model surface.** Adding `gold` to `predicate_state` would let
  `deposit-gold`/`give-gold` predicates (`gold-below?`) work in both passes and
  make the gold `:sim` faithful. Deferred: it ripples through every
  `predicate_state` caller.
- **Recipe/GE/task reference data** fetched + cached (like monsters/map) would
  let craft consume inputs and GE/task sims be real.
- **Equipment slot + GE order id as ident newtypes** (currently `String`), for
  the same misuse-resistance `Code`/`CharacterName` give.

## Testing

Per the repo's "few, broad, entrypoint-driven" standard, coverage is **one
table-driven cross-cutting test**, not 21 files: `tests/all_intents.rs`.

- `test_every_new_intent_runs_end_to_end` — for each intent, runs a one-action
  Fennel workflow through the full stack against a `MockDriver` and asserts (1)
  the exact request the intent put on the wire (method + path + JSON body,
  captured via the new `MockDriver::request_log`) and (2) that the response
  refreshed the live `SharedView`. This proves the whole Fennel→core path, and
  the wire format specifically, for every new action.
- `test_new_action_plan_costs` — runs the offline `plan` pass over a workflow of
  new actions and asserts the summed cooldown (`craft` 5s/item + `recycle`
  3s/item + two flat-3s actions = 25s), proving the `:cost` formulas are wired.

`MockDriver` gained a request log (`RecordedRequest { method, path, body }`) so a
test can assert the byte-level wire format an intent produced, not merely that
*some* request went out — the missing piece needed to prove wire correctness
hermetically.
