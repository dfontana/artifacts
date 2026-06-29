# Artifacts MMO Client

A Rust + Fennel client for [Artifacts MMO](https://docs.artifactsmmo.com/). The core
is **sans-I/O** (pure game semantics — cooldowns, rate-limit buckets, the
request/response state machine — with no sockets or clocks), and bot logic is
authored in **Fennel**. Because a workflow is data rather than opaque code, the same
source runs through three interpreters: `estimate` (predict time/actions/cost, no
I/O), `simulate` (run the control flow against mock game data to resolve loop
counts), and `run` (real execution).

## Layout

Two crates. The split is deliberate and minimal: `core` is its own crate so its
dependency tree (serde/thiserror only — no tokio, reqwest, or mlua) *proves* the
sans-I/O property at compile time. Everything that does I/O lives in the single
`artifacts` crate as plain modules.

```
core/        Sans-I/O brain — pure deps only
src/         The `artifacts` crate: I/O, runtime, Fennel host, CLI
tests/       Integration tests (hermetic acceptance + live API)
fennel/      Fennel workflow layer (the three interpreters live here)
vendor/      Pinned single-file Fennel compiler (fennel.lua)
```

### `core/` (crate `artifacts-core`)

Sans-I/O brain: `CharacterState`/rate-limit buckets (`state.rs`), the
`Step`/`Intent`/`Outcome` types (`step.rs`), cooldown cost formulas
(`cooldown.rs`), response-code classification (`error.rs`), the
`next_step`/`handle_response` machine (`machine.rs`), and A* overworld
pathfinding (`map.rs`). No async, no HTTP — by construction.

### `src/` (crate `artifacts`)

| Module | What it does |
|---|---|
| `driver` | I/O behind a `Driver` trait: `mock` (fake clock + canned responses) for tests, `http` (reqwest + tokio) for the live API. |
| `scheduler` / `character` / `view` | Bridge intents to outcomes — the async scheduler, the blocking `Character` facade, and the synchronously-readable view. |
| `lua` | Embeds Fennel and registers the host functions the workflow layer calls. |
| `planner` | Runs the offline `estimate`/`simulate` passes, returning plain Rust structs. |
| `live` | Wires a driver to the scheduler + `Character` and runs a workflow's `run` pass against the real game. |
| `main.rs` | Thin CLI: `artifacts estimate\|simulate <wf.fnl>` (offline) or `artifacts run <wf.fnl> <character>` (live, needs `ARTIFACTS_TOKEN`). |

### `fennel/`

| Path | What it does |
|---|---|
| `lib/actions.fnl` | The action vocabulary — each action defined **once** as a `{:cost :sim :run}` record so all three passes share one definition. |
| `lib/predicates.fnl` | Workflow predicates (`inventory-full?`, `hp-below?`, …) evaluated identically against the live view and the mock model. |
| `lib/interp.fnl` | The `estimate` / `simulate` / `run` interpreters. |
| `workflows/` | Authored bot workflows (e.g. `farm-copper.fnl`). |

## Building & testing

```sh
cargo test --test farm_copper   # hermetic acceptance test (offline)
cargo run -- estimate fennel/workflows/farm-copper.fnl
```

Live tests and the `run` command require `ARTIFACTS_TOKEN`.
