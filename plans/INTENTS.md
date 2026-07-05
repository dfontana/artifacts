# Plan: one-place intent definitions

**Status: implemented.** All three phases (wire.rs + IntentWire, the
`host_fn` registration list, and the `withdraw-item` prove-out) have landed.
Every design decision in this document was made in advance; the implementing
agent transcribed, not chose. All `file:line` references are against commit
`b7415f3` (`main`), the state before phase 1.

**How to implement:** three phases, each one `jj` revision, each gated on the
commands in §8 before moving on. Phases 1 and 2 are pure refactors (no
observable behavior change); phase 3 adds one genuinely new intent
(`withdraw-item`) end-to-end to prove the new shape on a real addition.

---

## 1. The problem

A game "intent" is a single character action — move, gather, fight, rest,
deposit. Making a **new** one reachable from a Fennel workflow today means
editing **five files** at **seven sites** (§2), in dependency order. Four of
those sites are enum matches the compiler checks; the dangerous pair is in
`src/lua.rs`, where a `&str` name list (`RUN_HOST_FNS`) and a `&str` match
(`register_run_host_fns`) must agree, checked by nothing but a runtime
`unreachable!()` (`src/lua.rs:478`) that only fires when the code path
actually executes.

Evidence this chain rots:

1. **`deposit_all` has no `Intent` variant** (`src/character.rs:58-74`): it
   is a fully registered host fn that loops `Intent::DepositItem` per slot.
   Legitimate (there is no single-request deposit-everything endpoint), but
   it means the `Intent`↔host-fn relationship has a silent branch that is
   invisible from the enum alone.
2. **Historical: half-wired intents were culled.** The doc comment at
   `core/src/step.rs:31-34` records that `craft`, `equip`, `withdraw`, `use`,
   and `recycle` once existed as wire-format code (`Intent` variant +
   `build_request` arm) with no host fn behind them — "untested wire-format
   code only rots" — and were deleted. Nothing structural prevents that
   half-wired state from recurring; only the culling policy does.

Goal: adding an intent becomes a set of small, colocated, compiler- or
single-list-enforced edits — no cross-file name synchronization, and no way
to compile a half-wired intent whose failure only surfaces at runtime.

---

## 2. Current-state inventory (what gets replaced)

| # | Site | File:line | Fate under this plan |
| --- | --- | --- | --- |
| 1 | `Intent` enum | `core/src/step.rs:35-42` | Hand-written in new `core/src/wire.rs` as tuple variants wrapping the wire structs; re-exported from `step.rs` so external paths don't change. |
| 2 | `build_request` + `post`/`post_json` | `core/src/machine.rs:155-185` | Deleted; becomes `intent.request()` delegating to per-intent `IntentWire::request`. Helpers move to `wire.rs`. |
| 3 | `parse_action_response` per-intent match + local payload structs | `core/src/machine.rs:187-293` | Per-intent half becomes `intent.outcome(payload)`; payload structs (`FightData`, `FightCharacter`, `DetailsData`) move to `wire.rs`. Envelope/cooldown/character resolution stays in `machine.rs`. |
| 4 | `OutcomeKind` enum | `core/src/step.rs:171-187` | **Unchanged, stays hand-written** (see §3, decision 4). |
| 5 | `Character` wrapper methods | `src/character.rs:32-74` | Unchanged; remain the convention (see §3, decision 5). Construction sites updated to the new variant shape. |
| 6 | `RUN_HOST_FNS` const | `src/lua.rs:415-423` | Deleted. |
| 7 | `register_run_host_fns` `&str` match + plan-context stub loop | `src/lua.rs:433-483` and `src/lua.rs:395-403` | Replaced by one list of `host_fn(...)` registrations that serves **both** the live and plan-context paths (§5). |

Not touched: `fennel/lib/actions.fnl`'s `def-action` contract (already the
"one place" pattern — asserts `:cost`/`:sim`/`:run` at load time),
`host.cooldown_cost`'s formula-name match (`src/lua.rs:232-263` — keyed by
formula, not intent; several intents can share one formula; see §9), the
`Step`/`Method`/`Progress` state machine, and the scheduler.

Complete list of `Intent` construction sites (verified by grep; these are the
only call-site churn in phase 1):

- `src/character.rs:33,37,41,45,49,67` (the six `submit` calls)
- `core/src/machine.rs` tests: `Intent::Gather` at `:343,:382,:401,:432`,
  `Intent::Move { x: 2, y: 0 }` at `:449`

Complete list of exhaustive `OutcomeKind` consumers outside `core`: none.
(`src/lua.rs:444` is an `if let`; `src/scheduler.rs:129` only constructs
`OutcomeKind::NoOp`.) Adding variants in phase 3 breaks nothing downstream.

---

## 3. Design decisions (all resolved — do not reopen)

| # | Decision | Choice | Why |
| --- | --- | --- | --- |
| 1 | Enum variant shape | **Tuple variants wrapping per-intent wire structs**: `Intent::Move(wire::Move)`, not named fields on the enum | Makes the single dispatcher trivial (every `wire()` arm is `Intent::X(w) => w`), keeps `Intent` small/`Clone`/exhaustively matched, and puts the payload type and its behavior on one struct. Construction churn is 11 sites, all listed in §2. |
| 2 | Dispatch mechanism | **No macros: hand-written enum + a single `Intent::wire(&self) -> &dyn IntentWire` dispatcher facade**; `request`/`outcome` are one-line calls through it | Trait objects were rejected *in the queue* (a `Box<dyn>` queue would cost `Clone` and compiler-checked exhaustiveness), but the facade borrows `&dyn` only transiently at the call site — the enum stays plain while `wire()`'s exhaustive match becomes the **only** intent match in the crate. Per intent that is two one-line, compile-enforced edits (variant + arm): too little boilerplate to justify a macro's readability/tooling tax. |
| 3 | Where per-intent code lives | **New module `core/src/wire.rs`**: trait, payload structs, per-intent structs + impls, the `Intent` enum + dispatcher. `step.rs` re-exports `Intent`. | One file holds everything per-intent on the `core` side. Re-export keeps `artifacts_core::step::Intent` working so `character.rs`/`scheduler.rs` imports don't change. |
| 4 | `OutcomeKind` generation | **Stays a hand-written enum in `step.rs`** | It is not 1:1 with intents: `NoOp` is constructed by the scheduler with no intent, and `Deposit` serves both `deposit_item` and the `deposit_all` loop. A wire impl returning a nonexistent variant is a compile error, so hand-maintenance is already safe; any generation scheme would have to special-case the non-1:1 variants for zero safety gain. |
| 5 | `Character` wrapper methods | **Remain the convention (one thin method per intent)**; `submit` stays private | The wrapper is 3 lines, gives live tests (`tests/live_api.rs`) their call surface, and keeps `lua.rs` free of `Intent`/`wire` imports. Exposing `submit` to skip wrappers saves nothing measurable. |
| 6 | Host-fn side | **One list of `host_fn(...)` calls inside `register_run_host_fns(lua, host, Option<Character>)`, where `host_fn` is a plain generic helper fn (no macro), used by both live and plan-context paths** | Host-fn binding is inherently runtime (closures per `Lua` state), so there is no compile-time exhaustiveness to lose — the win is one list instead of two. Taking `Option<Character>` makes the plan-context stubs *the same registrations* by construction: an entry can never be live-only or stub-only. |
| 7 | Envelope payload transport | **`ActionPayload` struct in `wire.rs`, `#[serde(flatten)]`-ed into `machine.rs`'s response `Data`** | A future intent needing a new response field (e.g. a `bank` payload) adds it to `ActionPayload` in `wire.rs` — `machine.rs` is not touched. |
| 8 | Stub error message | Keep the exact string `"run-pass host fn called in plan context"` | Preserves observable behavior in phase 2 (pure refactor). No test asserts on it today, but there is no reason to change it either. |
| 9 | Prove-out intent for phase 3 | **`withdraw-item`**, not craft/equip/use/recycle | It is the only candidate with zero unknowns: endpoint mirrors the verified deposit endpoint, response parsed identically (`details.items`), and its cooldown formula is the existing `deposit` formula (`core/src/cooldown.rs:47-50` already documents "Deposit/**Withdraw**/Give: 3s per distinct item type"). The others need live cooldown/response verification first (§9). |

---

## 4. Phase 1 — `core/src/wire.rs` (pure refactor)

**Revision message:** `refactor(core): one-place intent definitions (wire.rs + IntentWire)`

### 4.1 New file `core/src/wire.rs`

Write it exactly as follows (this is the full file, not a sketch):

```rust
//! One-place intent definitions.
//!
//! Each intent's wire format (request building) and outcome parsing live
//! together on one struct implementing `IntentWire`. The `Intent` enum wraps
//! those structs, and `Intent::wire` is the single dispatcher — the only
//! match over intents in the crate; `request`/`outcome` are one-line facades
//! through it. Adding an intent on the `core` side is: one struct +
//! `IntentWire` impl below, one enum variant, one `wire()` arm, and (if it
//! produces a new outcome shape) one `OutcomeKind` variant in `step.rs` —
//! all compile-checked: a variant without a `wire()` arm is a non-exhaustive
//! match, an arm whose struct lacks the impl won't coerce to
//! `&dyn IntentWire`, and neither can fail at runtime.
//!
//! The host-fn binding lives in `src/lua.rs` (`register_run_host_fns`) and
//! the Fennel action in `fennel/lib/actions.fnl`; see plans/INTENTS.md for
//! the full checklist.

use serde_json::json;

use crate::ident::Code;
use crate::step::{DropItem, FightOutcome, FightResult, Method, OutcomeKind, Step};

/// The two per-intent halves of the request/response cycle. `request` builds
/// the HTTP call; `outcome` interprets the action-specific part of a 2xx
/// response. Everything shared (envelope, cooldown, character snapshot, error
/// classification) stays in `machine.rs`. Object-safe by design — the only
/// consumer is `Intent::wire`, which hands impls out as `&dyn IntentWire`.
pub(crate) trait IntentWire {
    fn request(&self) -> Step;
    fn outcome(&self, payload: ActionPayload) -> OutcomeKind;
}

/// The action-specific portion of a 2xx action response — the fields whose
/// presence and shape depend on which intent was sent. `machine.rs` flattens
/// this into its response envelope and hands it to the pending intent's
/// `outcome`. A future intent needing a new response field adds it here.
#[derive(Debug, Default, serde::Deserialize)]
pub(crate) struct ActionPayload {
    #[serde(default)]
    pub fight: Option<FightData>,
    #[serde(default)]
    pub details: Option<DetailsData>,
}

/// `action/fight`'s payload. The live API tucks xp/gold/drops inside
/// `fight.characters[]`, not at the top of `fight`.
#[derive(Debug, Default, serde::Deserialize)]
pub(crate) struct FightData {
    pub turns: u32,
    pub result: String,
    /// Per-character fight outcome (xp/gold/drops live here on the live API).
    #[serde(default)]
    pub characters: Vec<FightCharacter>,
}

#[derive(Debug, Default, serde::Deserialize)]
pub(crate) struct FightCharacter {
    #[serde(default)]
    pub xp: u32,
    #[serde(default)]
    pub gold: u32,
    #[serde(default)]
    pub drops: Vec<DropItem>,
}

/// The `details` payload (DropSchema / hp_restored) non-fight actions return.
#[derive(Debug, Default, serde::Deserialize)]
pub(crate) struct DetailsData {
    #[serde(default)]
    pub items: Vec<DropItem>,
    #[serde(default)]
    pub hp_restored: Option<u32>,
}

/// A bodyless POST action (character name rides in the path, set by the driver).
fn post(path: &str) -> Step {
    Step::Request {
        method: Method::Post,
        path: path.into(),
        body: None,
    }
}

/// A POST action carrying a JSON body.
fn post_json(path: &str, body: serde_json::Value) -> Step {
    Step::Request {
        method: Method::Post,
        path: path.into(),
        body: Some(serde_json::to_vec(&body).unwrap()),
    }
}

/// The intents the workflow layer can actually reach today — each variant
/// wraps its wire struct (request + outcome parsing in one place, below)
/// and is backed by a run host fn in `src/lua.rs`. Further live actions
/// (craft, equip, use, recycle, …) get a variant when their host fn lands,
/// not before — untested wire-format code only rots.
#[derive(Debug, Clone)]
pub enum Intent {
    Move(Move),
    Gather(Gather),
    Fight(Fight),
    Rest(Rest),
    DepositItem(DepositItem),
}

impl Intent {
    /// The single dispatcher: resolve this intent's `IntentWire` impl. This
    /// is the ONLY match over intents in the crate — every other consumer
    /// goes through the facades below. The `&dyn` borrow is transient (just
    /// this call), so the enum itself stays small, `Clone`, and plain: the
    /// queue never holds a trait object.
    fn wire(&self) -> &dyn IntentWire {
        match self {
            Intent::Move(w) => w,
            Intent::Gather(w) => w,
            Intent::Fight(w) => w,
            Intent::Rest(w) => w,
            Intent::DepositItem(w) => w,
        }
    }

    /// Build the HTTP request for this intent.
    pub(crate) fn request(&self) -> Step {
        self.wire().request()
    }

    /// Parse the action-specific outcome of a 2xx response.
    pub(crate) fn outcome(&self, payload: ActionPayload) -> OutcomeKind {
        self.wire().outcome(payload)
    }
}

#[derive(Debug, Clone)]
pub struct Move {
    pub x: i32,
    pub y: i32,
}

impl IntentWire for Move {
    fn request(&self) -> Step {
        post_json("action/move", json!({"x": self.x, "y": self.y}))
    }

    fn outcome(&self, _payload: ActionPayload) -> OutcomeKind {
        OutcomeKind::Move
    }
}

#[derive(Debug, Clone)]
pub struct Gather;

impl IntentWire for Gather {
    fn request(&self) -> Step {
        post("action/gathering")
    }

    fn outcome(&self, payload: ActionPayload) -> OutcomeKind {
        OutcomeKind::Gather {
            items: payload.details.unwrap_or_default().items,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Fight;

impl IntentWire for Fight {
    fn request(&self) -> Step {
        post("action/fight")
    }

    fn outcome(&self, payload: ActionPayload) -> OutcomeKind {
        let f = payload.fight.unwrap_or_default();
        // xp/gold/drops are reported per character; this client drives one.
        let per_char = f.characters.into_iter().next().unwrap_or_default();
        OutcomeKind::Fight(FightResult {
            turns: f.turns,
            result: if f.result == "win" {
                FightOutcome::Win
            } else {
                FightOutcome::Lose
            },
            xp: per_char.xp,
            gold: per_char.gold,
            drops: per_char.drops,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Rest;

impl IntentWire for Rest {
    fn request(&self) -> Step {
        post("action/rest")
    }

    fn outcome(&self, payload: ActionPayload) -> OutcomeKind {
        OutcomeKind::Rest {
            hp_restored: payload
                .details
                .unwrap_or_default()
                .hp_restored
                .unwrap_or(0),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DepositItem {
    pub code: Code,
    pub quantity: u32,
}

impl IntentWire for DepositItem {
    fn request(&self) -> Step {
        post_json(
            "action/bank/deposit/item",
            json!({"code": self.code, "quantity": self.quantity}),
        )
    }

    fn outcome(&self, payload: ActionPayload) -> OutcomeKind {
        OutcomeKind::Deposit {
            items: payload.details.unwrap_or_default().items,
        }
    }
}
```

Note the enum variant `Move` and the struct `Move` share a name in the same
module — that is intentional and legal (separate namespaces); construction
reads `Intent::Move(wire::Move { x, y })`.

### 4.2 `core/src/lib.rs`

Add `pub mod wire;` to the module list (alphabetical position: after `step`).

### 4.3 `core/src/step.rs`

- Delete the `Intent` enum and its doc comment (`core/src/step.rs:31-42`).
- In its place add:

```rust
pub use crate::wire::Intent;
```

- Everything else (`OutcomeKind`, `Outcome`, `CharacterView`, `DropItem`,
  `FightResult`, …) is unchanged.

### 4.4 `core/src/machine.rs`

- Delete `post`, `post_json`, and `build_request`
  (`core/src/machine.rs:155-185`) — they moved to `wire.rs`.
- In `next_step`, replace `let step = build_request(&intent);` with
  `let step = intent.request();` (`core/src/machine.rs:85`).
- Rewrite `parse_action_response` as follows — the envelope/cooldown/
  character-resolution half stays, the per-intent half delegates:

```rust
fn parse_action_response(body: &[u8], intent: Option<&Intent>) -> Result<Outcome, GameError> {
    use crate::wire::ActionPayload;

    // All action responses have a common envelope shape.
    #[derive(serde::Deserialize)]
    struct Envelope {
        data: Data,
    }

    // The live API is not uniform: most actions return a single `character`,
    // but `action/fight` returns a `characters` array. Accept both. The
    // action-specific fields (fight, details, …) flatten into ActionPayload,
    // so a new intent's payload field is added in wire.rs, not here.
    #[derive(serde::Deserialize)]
    struct Data {
        cooldown: Cooldown,
        #[serde(default)]
        character: Option<CharacterView>,
        #[serde(default)]
        characters: Vec<CharacterView>,
        #[serde(flatten)]
        payload: ActionPayload,
    }

    let env: Envelope = serde_json::from_slice(body)?;
    let data = env.data;

    // A 200 with no pending intent is an impossible state — `next_step`
    // always sets one before a request is sent. Surface it loudly rather
    // than laundering it into a benign NoOp (which the scheduler reports
    // as ordinary success); see the sibling "no character" check below for
    // the same class of invariant violation.
    let Some(intent) = intent else {
        return Err(GameError::Internal(
            "200 response with no pending intent".into(),
        ));
    };
    let kind = intent.outcome(data.payload);

    // Resolve the character snapshot from whichever field the action populated.
    let character = data
        .character
        .or_else(|| data.characters.into_iter().next())
        .ok_or_else(|| GameError::Internal("action response had no character".into()))?;

    Ok(Outcome {
        cooldown: data.cooldown,
        character,
        kind,
    })
}
```

- Fix imports: the `use crate::step::{...}` list drops `Method` and
  `OutcomeKind` (no longer referenced here); everything else stays.
- In the `#[cfg(test)]` module, update the five construction sites:
  `Intent::Gather` → `Intent::Gather(crate::wire::Gather)` (4×),
  `Intent::Move { x: 2, y: 0 }` → `Intent::Move(crate::wire::Move { x: 2, y: 0 })`.
  (Or add `use crate::wire;` at the top of the test module and write
  `Intent::Gather(wire::Gather)` — either is fine; test assertions are
  untouched.)

### 4.5 `src/character.rs`

- Add `wire` to the `artifacts_core` import:
  `use artifacts_core::{error::GameError, ident::Code, step::{Intent, Outcome}, wire};`
- Update the six construction sites:

```rust
self.submit(Intent::Move(wire::Move { x, y }))
self.submit(Intent::Gather(wire::Gather))
self.submit(Intent::Fight(wire::Fight))
self.submit(Intent::Rest(wire::Rest))
self.submit(Intent::DepositItem(wire::DepositItem { code: code.into(), quantity }))
// and in deposit_all's loop:
self.submit(Intent::DepositItem(wire::DepositItem { code, quantity: qty }))
```

Method signatures, docs, and `deposit_all`'s logic are unchanged.

### 4.6 Behavior invariants for phase 1

Byte-for-byte identical requests and outcome parsing. Specifically: same
paths, same JSON bodies (`Code` is `#[serde(transparent)]`, so
`json!({"code": self.code})` serializes exactly as before), same
`unwrap_or_default()` fallbacks, same two `GameError::Internal` messages.
The regression gate is §8 phase 1.

---

## 5. Phase 2 — one host-fn table in `src/lua.rs` (pure refactor)

**Revision message:** `refactor: fold RUN_HOST_FNS + its match into one host_fn registration list`

### 5.1 Delete

- The `RUN_HOST_FNS` const and its doc comment (`src/lua.rs:409-423`).
- The plan-context stub block in `register_host_functions`
  (`src/lua.rs:392-403` — the whole `if let Some(char) … else { … }`).

### 5.2 Replace the call site

At the end of `register_host_functions` (where the deleted branch was),
before `lua.globals().set("host", host)?;`:

```rust
    // Run-pass fns: registered against the live character when present; in
    // plan context (no character) the SAME registrations become loud stubs —
    // one list by construction, so the two paths can't drift.
    register_run_host_fns(lua, &host, character)?;
```

The existing `let live_view: Option<SharedView> = character.as_ref().map(…)`
for `find_tile` (`src/lua.rs:313`) already runs earlier and only borrows, so
moving `character` here compiles as-is.

### 5.3 Rewrite `register_run_host_fns`

Replace the whole function (`src/lua.rs:433-483`) with:

```rust
/// Register one run-pass host fn: one `lua.create_function` + `host.set`
/// pair. With a live `Character`, `body` runs against it; with `None` (plan
/// context) the registered closure raises loudly before `body` runs — the
/// same registration serves both paths, so they can't drift. `body` receives
/// the live `&Character`, the Lua handle (name it `_lua` when unused), and
/// the args mlua decoded (a tuple, or `()` for arg-less fns).
fn host_fn<A, R>(
    lua: &Lua,
    host: &LuaTable,
    char: &Option<Arc<Character>>,
    name: &str,
    body: impl Fn(&Character, &Lua, A) -> LuaResult<R> + Send + 'static,
) -> LuaResult<()>
where
    A: mlua::FromLuaMulti,
    R: mlua::IntoLuaMulti,
{
    let char = char.clone();
    let f = lua.create_function(move |lua, args: A| {
        let Some(c) = char.as_deref() else {
            return Err(lua_err("run-pass host fn called in plan context"));
        };
        body(c, lua, args)
    })?;
    host.set(name, f)
}

/// Register every run-pass host fn. The `host_fn` calls below are the single
/// list of run fns — live binding and plan-context stub come from the same
/// entry, and adding an intent's binding is one call here; there is no
/// separate name list to keep in sync.
fn register_run_host_fns(
    lua: &Lua,
    host: &LuaTable,
    character: Option<Character>,
) -> LuaResult<()> {
    let char: Option<Arc<Character>> = character.map(Arc::new);

    host_fn(lua, host, &char, "gather", |c, _lua, ()| done(c.gather()))?;
    host_fn(lua, host, &char, "move", |c, _lua, (x, y): (i32, i32)| {
        done(c.move_to(x, y))
    })?;
    host_fn(lua, host, &char, "fight", |c, _lua, ()| {
        let outcome = c.fight().map_err(lua_err)?;
        // Live loss-bail: a loss respawns the character at spawn with 1 HP,
        // so looping into another fight death-spirals. Stop the workflow.
        if let OutcomeKind::Fight(ref f) = outcome.kind {
            if f.result == FightOutcome::Lose {
                return Err(lua_err(
                    "fight lost — bailing (character respawned at 1 HP); the plan \
                     pass should have flagged this as not winnable",
                ));
            }
        }
        Ok(())
    })?;
    host_fn(lua, host, &char, "rest", |c, _lua, ()| done(c.rest()))?;
    host_fn(
        lua,
        host,
        &char,
        "deposit_item",
        |c, _lua, (code, qty): (String, u32)| done(c.deposit_item(code, qty)),
    )?;
    host_fn(lua, host, &char, "deposit_all", |c, _lua, ()| {
        c.deposit_all().map_err(lua_err)?;
        Ok(())
    })?;
    // view() -> the predicate-facing model-state table for the live
    // character, built through the same helper the plan pass uses so the
    // two can't drift (see predicate_state).
    host_fn(lua, host, &char, "view", |c, lua, ()| {
        let v = c.view.get();
        predicate_state(
            lua,
            v.x,
            v.y,
            v.hp,
            v.max_hp,
            v.inventory_count(),
            v.inventory_max_items,
            &CombatStats::from(&*v),
        )
    })?;

    Ok(())
}
```

`done` (`src/lua.rs:427-431`) is unchanged. The `use` list at the top of the
file is unchanged (`FightOutcome`, `OutcomeKind`, `Arc`, `CombatStats` are
all still used); `FromLuaMulti`/`IntoLuaMulti` are written fully qualified in
the helper's `where` clause so it compiles regardless of what the `mlua`
prelude re-exports. If the project's `mlua` build has the `send` feature off,
the `+ Send` bound on `body` is stricter than `create_function` requires but
harmless — every closure in the list captures only `Arc<Character>` and
`Copy` data, which are `Send`.

### 5.4 Behavior invariants for phase 2

- Same seven names registered on `host` in both contexts.
- Plan-context calls to any of them raise the exact same error string as
  today (`"run-pass host fn called in plan context"`).
- One deliberate, benign difference: today the live path registers real fns
  and the plan path registers stubs via two different code paths; now one
  path does both. `host.view` in plan context still errors (it did before —
  the plan pass reads its threaded model state, never `host.view`).

---

## 6. Phase 3 — `withdraw-item`, end-to-end (the prove-out)

**Revision message:** `feat: withdraw-item intent (bank withdrawal), end-to-end`

This exercises every link of the new chain on a genuinely new intent. All
parameters are known: the endpoint mirrors the verified deposit endpoint,
the response is parsed identically (`details.items`), and the cooldown
formula is the existing `deposit` formula, whose doc comment
(`core/src/cooldown.rs:47`) already covers withdraw.

### 6.1 `core/src/wire.rs`

Add the variant and its dispatcher arm:

```rust
    // in the Intent enum:
    WithdrawItem(WithdrawItem),

    // in Intent::wire's match:
    Intent::WithdrawItem(w) => w,
```

and append the wire struct:

```rust
#[derive(Debug, Clone)]
pub struct WithdrawItem {
    pub code: Code,
    pub quantity: u32,
}

impl IntentWire for WithdrawItem {
    fn request(&self) -> Step {
        post_json(
            "action/bank/withdraw/item",
            json!({"code": self.code, "quantity": self.quantity}),
        )
    }

    fn outcome(&self, payload: ActionPayload) -> OutcomeKind {
        OutcomeKind::Withdraw {
            items: payload.details.unwrap_or_default().items,
        }
    }
}
```

### 6.2 `core/src/step.rs`

Add to `OutcomeKind`, next to `Deposit`:

```rust
    Withdraw {
        items: Vec<DropItem>,
    },
```

### 6.3 `src/character.rs`

Add below `deposit_item`:

```rust
    pub fn withdraw_item(&self, code: impl Into<Code>, quantity: u32) -> Result<Outcome, GameError> {
        self.submit(Intent::WithdrawItem(wire::WithdrawItem {
            code: code.into(),
            quantity,
        }))
    }
```

### 6.4 `src/lua.rs`

One registration, after `deposit_item`:

```rust
    host_fn(
        lua,
        host,
        &char,
        "withdraw_item",
        |c, _lua, (code, qty): (String, u32)| done(c.withdraw_item(code, qty)),
    )?;
```

### 6.5 `fennel/lib/actions.fnl`

After `:deposit-item`:

```fennel
;; Withdraw from bank. The model adds items to inventory; bank stock is NOT
;; modelled, so the plan assumes the bank holds what the workflow withdraws —
;; a wrong assumption fails loudly at run time (server error), never silently
;; in the plan. Same server formula family as deposit: 3s per distinct type.
(def-action :withdraw-item
  {:bucket :action
   :cost (fn [_st _args]
           (host.cooldown_cost :deposit {:distinct_types 1}))
   :sim  (fn [st [code qty]]
           (inv-add st code qty))
   :run  (fn [_char [code qty]]
           (host.withdraw_item code qty))})
```

No `cooldown_cost` change: it reuses the `"deposit"` op.

### 6.6 Hermetic test — new file `tests/withdraw_item.rs`

Mirrors `tests/farm_copper.rs`'s structure and helpers. Full file:

```rust
/// Hermetic test for the :withdraw-item action — the prove-out addition for
/// the one-place intent chain (plans/INTENTS.md §6): plan pass predicts the
/// deposit-formula cost and the inventory gain; run pass hits the withdraw
/// endpoint via MockDriver and lands the items in the live view.
use artifacts::{
    driver::mock::{CannedResponse, MockDriver},
    lua::{eval_fennel, predicate_state, setup_lua, LuaSetupOptions},
};
use artifacts_core::{combat::CombatStats, step::CharacterView};
use mlua::prelude::*;

mod common;
use common::{char_json, response, spawn_mock, INV_MAX};

const WORKFLOW: &str = "(seq (action :withdraw-item [:copper_ore 3]))";

fn load_workflow(lua: &Lua) -> LuaValue {
    eval_fennel(lua, WORKFLOW, "withdraw.fnl").expect("failed to load workflow")
}

fn make_model_state(lua: &Lua) -> LuaTable {
    let st = predicate_state(lua, 0, 0, 100, 100, 0, INV_MAX, &CombatStats::default())
        .expect("predicate_state failed");
    st.set("inventory", lua.create_table().unwrap()).unwrap();
    st
}

#[test]
fn test_plan_pass_withdraw() {
    let lua = setup_lua(LuaSetupOptions::default()).expect("setup_lua failed");
    let wf = load_workflow(&lua);
    let st = make_model_state(&lua);

    let plan_fn: LuaFunction = lua.globals().get("plan").expect("plan not found");
    let result: LuaTable = plan_fn.call((wf, st)).expect("plan call failed");

    let seconds: f64 = result.get("seconds").expect("missing seconds");
    let actions: u32 = result.get("actions").expect("missing actions");
    let feasible: bool = result.get("feasible").expect("missing feasible");

    // deposit formula, 1 distinct type: 3s.
    assert!((seconds - 3.0).abs() < 0.01, "expected 3s, got {seconds}");
    assert_eq!(actions, 1);
    assert!(feasible);
}

#[test]
fn test_run_pass_withdraw() {
    let mut driver = MockDriver::new();
    driver.push_responses(vec![CannedResponse::new(
        "action/bank/withdraw/item",
        200,
        response(3.0, char_json(0, 0, 3, 100)),
    )]);

    let initial_view = CharacterView {
        name: "kael".into(),
        x: 0,
        y: 0,
        hp: 100,
        max_hp: 100,
        level: 1,
        inventory_max_items: INV_MAX,
        inventory: vec![],
        ..Default::default()
    };
    let (char, shared_view, scheduler_handle) = spawn_mock(driver, initial_view);

    let lua = setup_lua(LuaSetupOptions {
        character: Some(char),
        ..Default::default()
    })
    .expect("setup_lua with character failed");
    let wf = load_workflow(&lua);

    let run_fn: LuaFunction = lua.globals().get("run").expect("run fn not found");
    run_fn.call::<()>(wf).expect("workflow run failed");

    assert_eq!(
        shared_view.get().inventory_count(),
        3,
        "withdraw should land 3 items in the live view"
    );

    drop(lua); // drops Character → closes the scheduler channel
    let _ = scheduler_handle.join();
}
```

If any `common` helper signature differs from this sketch's assumptions,
follow the helper (`tests/common/mod.rs`) — assertions stay as written.

### 6.7 Live smoke test — append to `tests/live_api.rs`

One `#[ignore]`d test following the file's existing conventions (same driver
construction, same `inv_qty` helper, sequential-run warning applies). The
self-contained round trip — gather one ore, bank it, withdraw it back — so it
never depends on pre-existing bank stock:

```rust
// ─── Withdraw round trip: gather → deposit → withdraw ───────────────────────

#[test]
#[ignore]
fn live_withdraw_roundtrip() {
    let (char, view, _handle) = spawn_live(driver());

    char.move_to(COPPER.0, COPPER.1).expect("move to copper");
    char.gather().expect("gather");
    let before = inv_qty(&view.get(), "copper_ore");
    assert!(before >= 1, "gather should yield copper_ore");

    char.move_to(BANK.0, BANK.1).expect("move to bank");
    char.deposit_item("copper_ore", before).expect("deposit");
    assert_eq!(inv_qty(&view.get(), "copper_ore"), 0);

    let outcome = char.withdraw_item("copper_ore", 1).expect("withdraw");
    assert!(
        matches!(outcome.kind, OutcomeKind::Withdraw { .. }),
        "expected Withdraw outcome, got {:?}",
        outcome.kind
    );
    assert_eq!(inv_qty(&view.get(), "copper_ore"), 1);
}
```

Adapt the setup line to however the existing live tests spawn their
scheduler (reuse their exact pattern — the file predates this plan and its
helpers are authoritative). If the live response shape for withdraw differs
from deposit (it should not — both are BankItemTransaction endpoints), the
fix goes in `wire::WithdrawItem::outcome` **only**; that containment is the
point of the design.

---

## 7. Documentation updates (part of the phases, not an afterthought)

- **Phase 1**, `docs/ARCHITECTURE.md`:
  - In the `core/` module table, change `step.rs`'s row to "The vocabulary:
    `Step`, `Outcome`/`OutcomeKind`, `CharacterView` (re-exports `Intent`)"
    and add a row: `wire.rs` — "One-place intent definitions: each intent's
    request format + outcome parsing on one struct; the `Intent` enum and
    its single `&dyn IntentWire` dispatcher live beside them."
- **Phase 2**, `docs/ARCHITECTURE.md`: no structural change needed (the
  host-bridge section describes behavior, which is preserved). Check the
  "run-only" fn list stays accurate.
- **Phase 3**, `docs/ARCHITECTURE.md`: rewrite the "Adding things → A new
  action" bullet to the new checklist: wire struct + enum variant + `wire()`
  arm (`core/src/wire.rs`), `OutcomeKind` variant if new shape
  (`core/src/step.rs`), `Character` wrapper, one `host_fn(...)` registration
  (`src/lua.rs`), `def-action`
  (`fennel/lib/actions.fnl`). Add `withdraw_item` to the run-only host-fn
  list in the host-bridge section.
- **Phase 3**, this file: flip the status line to "implemented".

---

## 8. Acceptance gates (run after each phase; all must pass before the next)

```sh
cargo build --workspace
cargo test -p artifacts-core          # machine.rs unit tests (the 499/486/490/200 suite)
cargo test --workspace                # hermetic: farm_copper, find_tile_origin (+ withdraw_item in phase 3)
cargo clippy --workspace --all-targets
```

Live tests (`tests/live_api.rs`) are `#[ignore]`d and token-gated; phase 3's
`live_withdraw_roundtrip` should be run once before finalizing that revision
if a token is available (see the file header for the exact invocation), and
is otherwise a documented follow-up — the hermetic tests gate the merge.

Per-phase specifics:

| Phase | Additional check |
| --- | --- |
| 1 | `cargo test -p artifacts-core` green with **zero test-assertion edits** (only `Intent` construction syntax changed). Hermetic suite green with **zero edits**. |
| 2 | Hermetic suite green with zero edits — `farm_copper::test_run_pass` exercises 5 of the 7 registrations live; `farm_copper::test_plan_pass` exercises the stub path (any drift = nil-call = loud failure). |
| 3 | `tests/withdraw_item.rs` both tests green; `cargo test -p artifacts-core` still green (proves adding a row breaks nothing). |

Each phase is one `jj` revision with the message given in its section
(plus the Co-Authored-By trailer per repo convention).

---

## 9. Out of scope (deliberate, with reasons)

- **`host.cooldown_cost`'s formula-name match** (`src/lua.rs:232-263`): keyed
  by formula, not intent — several intents share one formula (withdraw reuses
  `deposit`), so folding it into `IntentWire` would force a 1:1 mapping that
  doesn't exist. Revisit only if a new intent needs a formula that doesn't
  exist yet.
- **Craft / equip / use / recycle**: each needs live verification (response
  shape, cooldown formula) before its wire struct can be written honestly.
  Each lands as its own revision using the §6 checklist, gated on a live
  smoke test like §6.7. Do not add speculative variants — the culling policy
  (the `Intent` enum's doc comment) stands.
- **`deposit_all`**: stays a `Character`-level loop over `DepositItem` (no
  single-request endpoint exists). Its host-fn row already fits the new table.
- **No macros, no trait objects in the queue** (the transient `&dyn` borrow
  inside `Intent::wire` is the only dyn use), **no change to
  `Step`/`Progress`, no cross-crate unification** of the `core` and `lua.rs`
  definitions — the sans-I/O boundary (`docs/ARCHITECTURE.md`) is
  load-bearing and preserved.
