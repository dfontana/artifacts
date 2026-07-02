# Artifacts MMO Client

A Rust + Fennel client for [Artifacts MMO](https://docs.artifactsmmo.com/). The core is **sans-I/O** (pure game semantics — cooldowns, rate-limit buckets, the request/response state machine — with no sockets or clocks), and bot logic is authored in **Fennel**. Because a workflow is data rather than opaque code, the same source runs through two interpreters: `plan` (predict time/actions/cost and feasibility by walking the control flow against a seed state — no I/O; seed it from a live character for a per-character prediction) and `run` (real execution).

## Layout

Two crates. The split is deliberate and minimal: `core` is its own crate so its dependency tree (serde/thiserror only — no tokio, reqwest, or mlua) _proves_ the sans-I/O property at compile time. Everything that does I/O lives in the single `artifacts` crate as plain modules.

```
core/        Sans-I/O brain — pure game semantics, pure deps only
src/         The `artifacts` crate: I/O, runtime, Fennel host, CLI
tests/       Integration tests (hermetic acceptance + live API)
fennel/      Fennel workflow layer (the two interpreters live here)
vendor/      Pinned single-file Fennel compiler (fennel.lua)
```

Bot logic is authored in `fennel/`; the Rust crates execute or predict it. For a full breakdown of how a Fennel workflow maps down through the host bridge, the runtime, and the sans-I/O `core` — and where to add a new action, predicate, or game rule — see [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Building & testing

```sh
cargo test --test farm_copper   # hermetic acceptance test (offline)
cargo run -- plan fennel/workflows/farm-copper.fnl
```

Live tests and the `run` command require `ARTIFACTS_TOKEN`.

## TUI

`artifacts tui <character>` opens a live terminal dashboard for one character: its stats and inventory (refreshed while idle), the workflows under `fennel/workflows/`, an inline feasibility/cost plan for the selected one, and a truthful per-step progress cursor when you run it. Three modes — Normal (`←→↑↓` move focus, `⏎` interact, `z` zoom, `q` quit), Interact (operate the focused pane: Workflows `↑/↓` select, `p` plan, `r` run / `R` override an infeasible plan; Run `x` stop), and Focus (`z` zooms the focused pane to a pop-over). The power bar always lists the bindings valid in the current mode. See [`plans/TUI.md`](plans/TUI.md) for the design.

Needs `ARTIFACTS_TOKEN`. The step/stat icons are widely-supported Unicode (a braille spinner, box-drawing bars); a Nerd Font renders them most cleanly but is not required (glyphs are centralized in `src/tui/glyphs.rs` for an easy swap).

## Formatting docs

Markdown is formatted with [Prettier](https://prettier.io/), with prose left unwrapped (one line per paragraph) so diffs stay sentence-level rather than reflowing whole paragraphs:

```sh
npx --yes prettier --prose-wrap never --write '**/*.md'
```

## External references

Authoritative sources for the game API — pull these when verifying a request body, cooldown formula, rate-limit bucket, or response code rather than trusting this repo's encoded assumptions.

| Reference | URL |
| --- | --- |
| OpenAPI spec (request/response shapes, field names) | https://api.artifactsmmo.com/openapi.json |
| API usage guide | https://docs.artifactsmmo.com/ |
| — Authorization (Bearer token) | https://docs.artifactsmmo.com/api_guide/authorization/ |
| — Rate limits (the bucket windows) | https://docs.artifactsmmo.com/api_guide/rate_limits/ |
| — Response codes | https://docs.artifactsmmo.com/api_guide/response_codes/ |
| Gameplay docs — actions & cooldowns | https://docs.artifactsmmo.com/concepts/actions/ |

API base URL: `https://api.artifactsmmo.com`.
