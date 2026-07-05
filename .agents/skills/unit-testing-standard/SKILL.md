---
name: unit-testing-standard
description: How to write and review tests in this repo. Enforces a "few, broad, entrypoint-driven" testing standard over many narrow per-function unit tests. Use this whenever writing new tests, reviewing existing ones, deciding whether a change needs test coverage, or consolidating/pruning `#[test]` blocks. Triggers on requests like "add a test for this", "write tests for X", "is this well tested", "clean up these tests", or when a code change touches Fennel workflows, the driver/MockDriver, core game logic, or the TUI.
---

# Unit testing standard for this repo

This repo's default failure mode is **too many small tests**: one `#[test]`
per private helper, per branch, per edge case. Each one individually looks
reasonable, but together they lock in implementation details, and every
internal refactor forces you to rewrite a pile of tests that were never
actually protecting a user-visible behavior. The standard here inverts that:
prefer **fewer tests that each exercise a real entrypoint end-to-end**, and
reserve narrow unit tests for the rare case where no entrypoint test can
reach the logic cheaply.

The guiding question before writing any test: **"what does a person driving
this system actually give it, and what do they actually see back?"** Write
the test in those terms. If you find yourself asserting on a private field,
an intermediate struct, or a helper function three layers below the public
API, that's the signal to zoom out.

## The three entrypoints in this repo

Almost everything worth testing is reachable through one of these. Look for
an existing test file exercising the same entrypoint before adding a new one
— you likely want to extend it (a new scenario, a new assertion on the same
run) rather than create a new file.

1. **Fennel workflow execution** — `setup_lua` + `eval_fennel` (`src/lua.rs`)
   against a `MockDriver` (`src/driver/mock.rs`). This is the real entrypoint
   users script against. A test here writes a workflow in actual Fennel
   source, scripts the HTTP responses the mock server would give, runs it,
   and asserts on the outcome: final character/bank state, the action plan,
   the progress log. See `tests/farm_copper.rs` and `tests/tui_alignment.rs`
   for the shape — mock data and expected outputs are derived from formulas
   in a doc comment, not hand-tuned magic numbers.
2. **The live scheduler around a `Driver`** — `live::spawn_scheduler` /
   `tests/common::spawn_mock`, for behavior that only shows up once actions
   are actually dispatched and cooldowns/rate-limiting apply.
3. **Core game-rule functions with no I/O** — `artifacts-core`'s `combat::simulate`,
   `map::GameMap::path_hops`, etc. These *are* entrypoints in their own right
   (the core crate's public API), so direct tests are fine — but see
   "Consolidating niche cases" below before adding a fourth or fifth `#[test]`
   for the same function.

`MockDriver` is the only isolation boundary. It replaces the network, not the
game logic — never mock the function whose behavior you're trying to prove.
If a scenario needs a response shape `MockDriver`/`tests/common` doesn't
support yet, extend the shared fixtures there (see below), and make sure the
shape matches the real API: check `.agents/skills/artifacts-api-spec` (the
`artifacts-api-spec` skill) for the actual endpoint schema rather than
guessing at field names or envelope structure. A mock that silently drifts
from the real API is worse than no mock — every test built on it develops a
false sense of security.

## What "asserting on behavior, not internals" means concretely

- Assert on `CharacterView`, bank/inventory contents, the returned plan
  summary, or the TUI's rendered frame/log — things a caller of the system
  actually receives.
- Don't assert on an intermediate struct's private field, a counter that only
  exists inside a loop, or that a specific private helper was called N times.
- If you can't get the assertion you want without reaching into internals,
  that's usually a sign the public API is missing a way to observe the thing
  you care about — surface it there, don't route around it in the test.

## Fixture hygiene

- Every test builds its own map/character/driver from scratch (`make_map`,
  `char_json`, fresh `MockDriver`). Don't reach for `static`/`lazy_static`
  shared state, and don't let one test mutate a fixture another test also
  reads — tests must be order-independent and parallel-safe by construction,
  not by convention.
- Shared *builders* (`tests/common/mod.rs`) are good — they're pure functions
  that return a fresh value each call. Shared *instances* are not.
- When a new scenario needs a fixture variant `tests/common` doesn't have
  (a new response envelope, a new map shape), add a builder function there
  instead of hand-rolling `serde_json::json!` inline in the test — the next
  test that needs the same shape shouldn't have to duplicate it (`farm_copper.rs`'s
  doc comment calls out this exact rationale: an API-schema tweak should be a
  one-place fix). Treat this as a required step, not an optional nicety: if
  you catch yourself writing `serde_json::json!({"error": ...})` or similar
  directly inside a test body, stop and add the builder to `tests/common/mod.rs`
  first, the same way `response()` and `fight_win()` already exist there for
  the success envelopes. "I only need it once" is never true — the next
  person adding an error-path test needs exactly the same envelope.

## Consolidating niche cases

Before adding a new `#[test]`, check whether it's really a new scenario or
just another point in the input space of a scenario you already cover. Signs
you should fold it into an existing test instead of adding a new one:

- It exercises the same entrypoint with the same setup, just tweaking one
  input value to hit a different branch (e.g. a different resistance %, a
  different cooldown level).
- Its name is `test_<function>_<one_branch>` — a strong smell that tests are
  organized by internal code path rather than by observable scenario.

When that's the case, fold multiple cases into one test function with several
assertions in sequence (see `core/src/combat.rs`'s
`damage_bonus_and_resistance_round_half_up`, which checks three rounding
cases in one test) rather than one `#[test]` per case. A test function should
map to a *scenario a user could describe in one sentence* ("a stalemate hits
the turn cap as a loss"), not to a code path.

This also applies when reviewing/pruning existing tests: if you see a cluster
of `#[test]`s that all set up the same fixture and differ only in one input
and one assertion, that's a consolidation opportunity, not a coverage
requirement to preserve as-is. Reducing test count while keeping the same
scenarios covered is a win, not a regression — fewer tests means less to
rewrite the next time an internal refactor happens.

Consolidating is more than merging function bodies and picking better names —
that's the easy 80% and it's tempting to stop there. While you're in a test
file for this reason, also do the two things a rename-only pass skips:

- **Extract repeated setup.** If every test in the file does the same
  `enqueue` + `next_step` + "assert it's a `Request`" dance before the part
  that's actually interesting, pull that into a small helper (a `fn started(...)`
  local to the test module is enough) and use it everywhere in the file, not
  just in the test you happened to be touching. Leaving three other tests
  with the old duplicated boilerplate while the one you merged gets a helper
  is half a cleanup.
- **Tighten assertions that were already loose.** A pattern like
  `assert!(matches!(step, Step::Request { .. } | Step::Sleep { .. }))` accepts
  either outcome and therefore can't fail — it was never really testing
  anything. If you're already touching this test for consolidation, check
  whether its assertions actually pin down a specific expected value (the
  precise `Sleep` reason, the precise error variant) rather than "some
  vaguely plausible result." Fixing this is part of meeting the standard, not
  a separate task to defer.

## When not to write a test

Don't pad coverage for its own sake. Skip tests for:

- Trivial pass-through/getters, `Default` impls, or plain data structs with
  no logic.
- Pure UI layout/glue code with no branching (e.g. wiring a widget's props
  through) — the TUI's *behavior* (state transitions, what a given log
  produces on screen) is worth testing via the reducer/alignment tests;
  the wiring that renders it usually isn't.
- Generated/derived code (serde impls, API bindings).
- Anything already covered by an existing entrypoint-level test as a side
  effect — don't add a redundant narrow test just because the function is
  "new," if a broader test already exercises it.

If a change has no user-observable behavior to describe in one sentence, it
probably doesn't need a new test.

## Gated/live tests

`tests/live_api.rs` and the `#[ignore]`d smoke test in `src/driver/http.rs`
hit the real Artifacts API and are excluded from the default run. These are
for verifying `MockDriver`'s scripted shapes still match production, not for
day-to-day coverage — don't move logic-under-test into this category just to
avoid building a proper mock scenario.

## Quick checklist before submitting a test

1. Can I name the scenario in one sentence, from the caller's point of view?
2. Am I asserting on output the system actually returns/exposes, not an
   internal detail?
3. Did I build fresh fixtures instead of sharing mutable state with another test?
4. Is this genuinely a new scenario, or another input value for a scenario
   already covered — and if the latter, can it become a second assertion in
   an existing test instead of a new `#[test]`?
5. If it touches HTTP shapes, does the mocked response match the real API
   spec (check the `artifacts-api-spec` skill), not just what the code
   currently happens to parse?
