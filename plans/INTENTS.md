# Plan: one-place intent definitions

**Status: proposed / not yet implemented.** No code changes accompany this
document; it records a pain point observed while working on `plans/TUI.md`'s
neighboring code and a recommended fix for the future.

This mirrors `plans/TUI.md`'s format: decisions in tables, every claim
anchored to `file:line`, a sketch of the new design against the old, and an
explicit recommendation rather than an open menu of options.

---

## 1. The problem

A game "intent" is a single character action — move, gather, fight, rest,
craft, deposit. Adding a **new** one that a Fennel workflow can actually call
today means editing **five files** at **up to eight sites**, in an order that
is easy to get wrong, with only a runtime panic (not a compiler error) if a
step is missed. The goal of this plan is an abstraction where adding an intent
is roughly **one struct definition** (its wire format, outcome parsing,
cooldown formula) **plus one small registration entry** (its host-fn
binding) — not a scavenger hunt across the crate boundary.

---

## 2. The current chain

Concretely, making one new intent — say `Gather` — reachable from a Fennel
workflow requires touching all of the following, in dependency order:

| # | Site | File:line | What you write |
| --- | --- | --- | --- |
| 1 | `Intent` variant | `core/src/step.rs:36-43` | A new enum arm carrying the action's typed payload (e.g. `Craft { code: Code, quantity: u32 }`). |
| 2 | `build_request` arm | `core/src/machine.rs:173-189` | Maps the variant to an HTTP method + path + JSON body (`Step::Request`). |
| 3 | `parse_action_response` arm | `core/src/machine.rs:246-286` | Maps the variant to an `OutcomeKind`, reading whatever shape of `details`/`fight` the live API returns for that action. |
| 4 | `OutcomeKind` variant | `core/src/step.rs:173-191` | The outcome payload shape (e.g. `Craft { items: Vec<DropItem> }`) the arm in #3 constructs. |
| 5 | `Character` method | `src/character.rs` (e.g. `craft` at 48-53, `deposit_item` at 55-60) | A thin wrapper that builds the `Intent` and calls `self.submit(...)`, giving the intent a name workflow-facing Rust code can call. |
| 6 | `RUN_HOST_FNS` entry | `src/lua.rs:423-431` | Add the host-fn name to the `&[&str]` list both the live registration and the plan-context stub loop iterate. |
| 7 | `register_run_host_fns` match arm | `src/lua.rs:441-491` (arms at 446-469) | The actual `lua.create_function` closure: pull typed args out of Lua, call the `Character` method from #5, marshal the result back. |
| 8 | `def-action` entry | `fennel/lib/actions.fnl` | A `:cost`/`:sim`/`:run` triple so the plan pass can predict the action and the run pass can call `host.<name>` from #7. `def-action` already *enforces* all three fields at load time (see the file's header comment), so this one is arguably already "one place" — but it is the *last* link, and is only reachable once 1-7 exist. |

Optionally, a ninth site: if the new action needs a client-side cooldown
estimate for the plan pass, `host.cooldown_cost`'s op-string match in
`src/lua.rs:232-271` needs a new arm too (see §4 — `recycling` already has a
formula there with nothing behind it).

That is **5-6 structural edits across 4 Rust-crate files** (`step.rs`,
`machine.rs`, `character.rs`, `lua.rs`) before the Fennel side (#8) can even
be written, plus the always-required Fennel entry. Missing any one of 1-7
either fails to compile (good — steps 1-4 are enum matches, so `rustc` catches
a missed arm) or **compiles fine and panics at runtime** (bad — steps 6-7 are
a `&str` list plus a `&str` match, and nothing checks they stay in sync except
the `unreachable!()` at `src/lua.rs:486`, which only fires when that code
path actually executes).

---

## 3. Existing inconsistencies (evidence this chain is error-prone)

Three real gaps in the current code, each a symptom of the same problem —
verified against the source, not hypothetical:

1. **`deposit_all` has no `Intent` variant.** `Character::deposit_all`
   (`src/character.rs:62-81`) is a fully registered, callable host fn (in
   `RUN_HOST_FNS` and its match arm) — but it has no corresponding
   `Intent::DepositAll`. It works by fetching `occupied_items()` from the
   live view and looping `Intent::DepositItem` once per slot. This is a
   legitimate design choice (there is no single-request "deposit everything"
   endpoint to model), but it means the intent chain has a silent branch: some
   host fns are 1:1 with an `Intent` variant, others are client-side loops
   over one. Any future refactor of the `Intent`↔host-fn relationship has to
   know this asymmetry exists, because it isn't visible from the enum alone.

2. **`Craft` has an `Intent` variant *and* a `Character::craft` method, but is
   unreachable from any workflow.** `Intent::Craft` exists
   (`core/src/step.rs:41`), `build_request` and `parse_action_response` both
   handle it (`core/src/machine.rs:180-183`, `:271-273`), and
   `Character::craft` exists (`src/character.rs:48-53`) — but `"craft"` is
   **not** in `RUN_HOST_FNS` (`src/lua.rs:423-431`), so no host fn exists to
   call it, and `fennel/lib/actions.fnl`'s `:craft` action's `:run` field is a
   hard-coded `(error "craft :run not implemented (no host.craft
   registered)")`. Four of the six sites in §2 are done; the two host-fn
   sites (6-7) were never added; the result is dead code path with a
   deliberately loud failure at the last mile rather than a silent no-op —
   better than nothing, but it shows how easy it is to stop three-quarters of
   the way through the chain and not notice, because nothing forces the
   remaining sites to be touched.

3. **Previously removed intents (equip, unequip, withdraw, use, recycle)
   existed as `Intent` + `build_request` wire format only**, with no
   `Character` method and no host fn — i.e. steps 1-2 of §2 done, steps 5-7
   never done. `core/src/step.rs:31-34`'s doc comment on `Intent` records why
   they were culled: "untested wire-format code only rots." The
   `cooldown_cost` formula table in `src/lua.rs:232-271` still carries a
   `"recycling"` arm (backed by `formulas::recycling` in
   `core/src/cooldown.rs:52-55`) with **no** `Intent::Recycle` and no
   `host.recycle` behind it — a plan-pass cost estimate for an action that
   cannot actually run. This is the same asymmetry as #2, one layer further
   out: a fragment of the chain surviving after the rest was deleted, because
   there is no single place whose removal deletes the whole feature at once.

Taken together: nothing enforces that the six-to-eight sites for one intent
move in lockstep. Partial completion is silent (compiles, some tests may even
pass) rather than loud, and the only two failure modes today are "workflow
author gets a `nil call` on `host.craft` at runtime" and "a doc comment
explaining after the fact why something was deleted."

---

## 4. Proposed abstraction: one definition, one registry

### 4.1 Shape of the fix

Collapse each intent's **wire format**, **outcome parsing**, **cooldown
formula**, and **host-fn binding** into a single per-intent definition, and
replace the current five hand-written dispatch sites (`build_request`,
`parse_action_response`, `RUN_HOST_FNS`, `register_run_host_fns`'s match, and
the `Character` wrapper method) with **generated or table-driven** dispatch
that reads that one definition.

Two things make this a two-part design rather than one, and that split is a
deliberate, not accidental, seam:

- **`core` stays sans-I/O.** `core/src/machine.rs` and `core/src/step.rs`
  know nothing about `mlua`, `Character`, or Fennel — that boundary is
  load-bearing (see `docs/ARCHITECTURE.md`) and this plan does not propose
  crossing it. So the wire-format/outcome-parsing half of an intent's
  definition lives in `core`, and the host-fn-binding half lives in
  `src/lua.rs` alongside `Character`. One intent → one declaration in each
  crate, not one declaration spanning both.
- **The `Intent` enum itself should stay a plain enum**, not a `Box<dyn
  Trait>`. `Core::queue: VecDeque<Intent>` and the whole `next_step`/
  `handle_response` state machine benefit from `Intent` being small, `Clone`,
  and matched exhaustively by the compiler — that exhaustiveness is exactly
  what already catches a missed `build_request` or `parse_action_response`
  arm today. A trait-object design would trade that compile-time guarantee
  for runtime dispatch, which is a strict regression for the two sites (1-4
  in §2) that are already safe. The unsafe sites are 5-7 (the `&str`-keyed
  host-fn plumbing) — that's where the fix should concentrate.

### 4.2 `core` side: a table macro generates variants + both matches

Today steps 1, 2, 3, 4 in §2 are four independent hand-written edits (enum
variant, `build_request` arm, `parse_action_response` arm, `OutcomeKind`
variant) that must stay aligned by eye. Replace them with one
`macro_rules!` table, invoked once, whose body is a list of per-intent rows:

```rust
// core/src/step.rs (sketch)
define_intents! {
    Move    { x: i32, y: i32 }              => wire::Move,
    Gather  {}                               => wire::Gather,
    Fight   {}                               => wire::Fight,
    Rest    {}                               => wire::Rest,
    Craft   { code: Code, quantity: u32 }   => wire::Craft,
    Deposit { code: Code, quantity: u32 }   => wire::Deposit,
    Equip   { code: Code, slot: Slot }      => wire::Equip,   // <- new intent, one row
}
```

Each `wire::X` is a small struct/impl (not a closure — keeps it readable and
debuggable) implementing one trait:

```rust
trait IntentWire {
    type Args;                                  // the payload carried on the Intent variant
    fn request(args: &Self::Args) -> Step;       // method + path + body
    fn outcome(args: &Self::Args, data: Data) -> OutcomeKind; // parse the response
}
```

The macro expands each row into: the `Intent::X { .. }` variant, one arm of
`build_request`, and one arm of `parse_action_response` that delegates to
`wire::X::request`/`::outcome`. Adding `Equip` becomes: write `struct
wire::Equip;` with its `request`/`outcome` impl (this *is* "one struct"), add
one row to the table (this *is* "one enum entry"). The exhaustiveness the
compiler gives today is preserved — the macro still expands to real `match`
arms, so a variant with no `wire::` impl fails to compile, not to run.

### 4.3 `src/lua.rs` side: fold `RUN_HOST_FNS` + its match into one table

Steps 6-7 in §2 are the actually dangerous pair: `RUN_HOST_FNS` (a `&[&str]`)
and `register_run_host_fns`'s `match *name { ... }` are two independent lists
that must list exactly the same names in a way nothing but a runtime
`unreachable!()` (`src/lua.rs:486`) checks. Collapse them into one table of
closures built in one place:

```rust
// src/lua.rs (sketch)
fn run_host_fns(lua: &Lua, char: &Arc<Character>) -> LuaResult<Vec<(&'static str, LuaFunction)>> {
    macro_rules! host_fn {
        ($name:literal, |$c:ident, $($arg:ident : $ty:ty),*| $body:expr) => {
            (
                $name,
                lua.create_function({
                    let $c = Arc::clone(char);
                    move |_, ($($arg,)*): ($($ty,)*)| $body
                })?,
            )
        };
    }
    Ok(vec![
        host_fn!("gather",       |c, | done(c.gather())),
        host_fn!("move",         |c, x: i32, y: i32| done(c.move_to(x, y))),
        host_fn!("fight",        |c, | { /* loss-bail unchanged */ }),
        host_fn!("rest",         |c, | done(c.rest())),
        host_fn!("deposit_item", |c, code: String, qty: u32| done(c.deposit_item(code, qty))),
        host_fn!("deposit_all",  |c, | { c.deposit_all().map_err(lua_err)?; Ok(()) }),
        host_fn!("equip",        |c, code: String, slot: String| done(c.equip(code, slot))), // <- new
        host_fn!("view",         |c, | { /* unchanged */ }),
    ])
}
```

The plan-context stub loop (`src/lua.rs:405-410`) already iterates "the same
list" as the live registration, by comment convention — under this design it
iterates the *same runtime `Vec`*, by construction, so "can't drift" becomes
a property of the code rather than a code comment asking the reader to trust
it. Adding a new intent's host binding is one line in this `vec![...]`
literal, colocated with every other binding instead of split across a
separate `const` array and a `match`.

### 4.4 The Fennel side is already close to right

`fennel/lib/actions.fnl`'s `def-action` (requiring `:cost`/`:sim`/`:run` at
load time — see the file's own header comment) is already the "one place"
pattern this plan wants for the rest of the chain. This proposal does not
touch it. The payoff is that once §4.2-4.3 land, a `def-action`'s `:run`
field pointing at `host.equip` is guaranteed to resolve to something real,
because the host-fn table and the `Intent` table are each single sources of
truth instead of five independently-maintained sites.

### 4.5 Sketch: adding `equip` today vs. under this design

| | Today | Under this design |
| --- | --- | --- |
| `core/src/step.rs` | Add `Intent::Equip { .. }` by hand. | Add one row to `define_intents!`. |
| `core/src/machine.rs` | Add a `build_request` arm and a `parse_action_response` arm by hand. | Write `wire::Equip`'s `request`/`outcome` impl (the "one struct"). |
| `core/src/step.rs` (`OutcomeKind`) | Add a variant by hand. | Emitted by the macro from the `wire::Equip::outcome` return type. |
| `src/character.rs` | Add an `equip` method by hand. | Optional thin convenience wrapper; not load-bearing (host fn can call `Intent::Equip` submission directly). |
| `src/lua.rs` (`RUN_HOST_FNS`) | Add `"equip"` to the array by hand. | N/A — no separate array. |
| `src/lua.rs` (match arm) | Add a match arm by hand, hope the name matches the array entry. | Add one `host_fn!(...)` row to the `vec![...]`. |
| `fennel/lib/actions.fnl` | Add a `def-action` (already required today). | Unchanged — still required, now guaranteed to resolve. |
| Failure mode if incomplete | Compiles; runtime `unreachable!()` panic or `nil` host call, only caught if that code path executes. | A missing `wire::` impl is a compile error; a missing `host_fn!` row means `host.equip` simply doesn't exist yet — same as today's "not implemented" but never silently half-wired. |

Net: **2 touch points** (one `core` struct/row, one `lua.rs` table row) down
from **5-6**, and the two that remain are exactly the two places that
inherently differ per crate (sans-I/O wire format vs. `mlua`/`Character`
binding) — not two more accidental copies of the same information.

---

## 5. Design choices and trade-offs

| Question | Options | Recommendation | Why |
| --- | --- | --- | --- |
| Declarative macro vs. proc-macro | `macro_rules!` table vs. a `syn`/`quote` proc-macro crate | **`macro_rules!`** | ~6-8 intents total; a proc-macro adds a new crate, a build-time cost, and an indirection tax (readers must mentally expand it) that isn't earned at this scale. `macro_rules!` table-of-rows is a well-worn pattern (Fennel's own `def-action` is conceptually the same idea) and every expansion is one `cargo expand` away from being readable. |
| Declarative macro vs. plain data table (`&[IntentDef]` + `dyn`/fn-pointer dispatch) | Struct-of-fn-pointers built at runtime vs. macro-generated match arms | **Macro-generated matches**, not a runtime table, for the `core` side | `Intent` needs to stay a plain `enum` for `Core`'s queue and for compiler-checked exhaustiveness (§4.1); a runtime `&[IntentDef]` table can't give you a compile error for a missing arm, only a missing-entry panic — reintroducing the exact failure mode (§3, item 2) this plan exists to remove. |
| Runtime table for the *host-fn* side specifically | Keep `RUN_HOST_FNS` + match (status quo) vs. one `Vec<(&str, LuaFunction)>` built in one function | **One `Vec`, built in one function (§4.3)** | Host-fn binding is inherently runtime (Lua functions are built per `Lua` state), so there's no compile-time exhaustiveness to lose here — the win is purely "one list instead of two lists that must agree," which a plain `Vec` literal already gives without any macro. |
| Keep `Character` convenience methods | Required per intent vs. optional | **Optional** | `src/character.rs`'s methods (`move_to`, `gather`, …) exist for readability at call sites, not because `submit(Intent::X)` doesn't work directly. Once host-fn closures can call `char.submit(Intent::Equip { .. })` inline, a wrapper method is a nice-to-have, not a required chain link — cuts one more site off the "always touch this" list. |
| `cooldown_cost`'s op-string table (`src/lua.rs:232-271`) | Leave as-is vs. fold into the same per-intent definition | **Leave as a known follow-up, not in scope here** | It's keyed by *formula name* (`"movement"`, `"crafting"`, …), which is coarser than *intent* — several intents can share a formula, and (per §3 item 3) a formula can exist with no intent behind it at all. Folding it into `wire::X` would need a `cooldown_formula` field on the trait in §4.2; worth doing once §4.2-4.3 prove out, but adding it now would grow this plan's surface before the core idea is validated. |

---

## 6. What this plan does *not* propose

- No change to the `Step`/`Method`/`Progress` state machine shapes.
- No change to `fennel/lib/actions.fnl`'s `def-action` contract.
- No proc-macro dependency.
- No attempt to unify the `core`-side and `lua.rs`-side definitions into a
  single cross-crate declaration — the sans-I/O boundary is intentional and
  this plan preserves it (§4.1).
- No change to already-working intents (`Move`, `Gather`, `Fight`, `Rest`,
  `DepositItem`) beyond re-expressing them through the new table — the
  observable behavior (wire format, outcome shape, cooldown) is unchanged.

---

## 7. Suggested implementation order

1. Write `wire::X` structs + the `IntentWire` trait + `define_intents!` for
   the five *already-working* intents first (`Move`, `Gather`, `Fight`,
   `Rest`, `DepositItem`), as a pure refactor with no behavior change —
   `core`'s existing unit tests in `core/src/machine.rs` (the `#[cfg(test)]`
   module, e.g. `test_499_reschedules_not_errors`) are the regression gate.
2. Fold `RUN_HOST_FNS` + `register_run_host_fns`'s match into the single
   `host_fn!`-table function in `src/lua.rs`, again as a pure refactor over
   the same five intents.
3. Only once 1-2 are landed and green, use the new shape to finish `Craft`
   (add its `host_fn!` row — it already has a `core`-side definition per §3
   item 2) and confirm `fennel/lib/actions.fnl`'s `:craft` `:run` field can
   drop its `(error ...)` stub.
4. Add genuinely new intents (`Equip`, `Withdraw`, `Use`, `Recycle`) one at a
   time, each as a single PR: one `wire::X` + one `define_intents!` row + one
   `host_fn!` row + one `def-action` in Fennel — proving the "one struct, one
   entry" claim on a real addition rather than a refactor.
