# Artifacts MMO Client

A Rust + Fennel client for [Artifacts MMO](https://docs.artifactsmmo.com/). The core
is **sans-I/O** (pure game semantics — cooldowns, rate-limit buckets, the
request/response state machine — with no sockets or clocks), and bot logic is
authored in **Fennel**. Because a workflow is data rather than opaque code, the same
source runs through three interpreters: `estimate` (predict time/actions/cost, no
I/O), `simulate` (run the control flow against mock game data to resolve loop
counts), and `run` (real execution).

## Layout

```
crates/      Rust workspace (sans-I/O core → I/O drivers → runtime → CLI)
fennel/      Fennel workflow layer (the three interpreters live here)
vendor/      Pinned single-file Fennel compiler (fennel.lua)
```

### `crates/`

| Crate | What it does |
|---|---|
| `core` | Sans-I/O brain: `CharacterState`/rate-limit buckets (`state.rs`), the `Step`/`Intent`/`Outcome` types (`step.rs`), cooldown cost formulas (`cooldown.rs`), response-code classification (`error.rs`), the `next_step`/`handle_response` machine (`machine.rs`), and A* overworld pathfinding (`map.rs`). No async, no HTTP. |
| `driver` | I/O behind a `Driver` trait: `MockDriver` (fake clock + canned responses) for tests, `HttpDriver` (reqwest + tokio) for the live API. |
| `runtime` | Scheduler + `Character` facade and the Fennel host. `scheduler.rs`/`character.rs`/`view.rs` bridge intents to outcomes; `lua.rs` embeds Fennel and registers host functions; `planner.rs` runs the offline `estimate`/`simulate` passes; `live.rs` wires a driver to the `run` pass. |
| `cli` | Thin binary: `artifacts estimate\|simulate <wf.fnl>` (offline) or `artifacts run <wf.fnl> <character>` (live, needs `ARTIFACTS_TOKEN`). |
| `tests` | Hermetic acceptance test (`farm_copper.rs`) proving the run/estimate/simulate split, plus live API integration tests (`live_api.rs`). |

### `fennel/`

| Path | What it does |
|---|---|
| `lib/actions.fnl` | The action vocabulary — each action defined **once** as a `{:cost :sim :run}` record so all three passes share one definition. |
| `lib/predicates.fnl` | Workflow predicates (`inventory-full?`, `hp-below?`, …) evaluated identically against the live view and the mock model. |
| `lib/interp.fnl` | The `estimate` / `simulate` / `run` interpreters. |
| `workflows/` | Authored bot workflows (e.g. `farm-copper.fnl`). |

## Building & testing

```sh
cargo test                 # runs the hermetic acceptance test (offline)
cargo run -p artifacts-cli -- estimate fennel/workflows/farm-copper.fnl
```

Live tests and the `run` command require `ARTIFACTS_TOKEN`.
