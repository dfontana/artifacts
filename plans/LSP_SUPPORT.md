# Plan: Fennel LSP support for editing workflows in Helix

How to get a working language-server experience when editing the `.fnl` files in
`fennel/` (workflows and the lib), and how to make the LSP aware of the host
surface that `src/lua.rs` injects into the Lua/Fennel environment at runtime.

This document is the agreed design from a research pass. It records **what to do
and why**, so it can be resumed from a blank slate. It has two halves:

1. **Part A — Basic setup** (install + suppress false diagnostics). Small, do
   this first; it makes editing usable today.
2. **Part B — Full experience** (real completion/hover/signatures via a
   generated docset, plus CLI commands to generate and staleness-check it). This
   is the larger design we worked out and the main reason this doc exists.

Read `docs/ARCHITECTURE.md` and `src/lua.rs` first — everything here hinges on
the fact that the Fennel runtime surface is **assembled at startup by Rust**, not
defined in any `.fnl` file the LSP can see.

---

## 0. The core problem (read this first)

Workflow files like `fennel/workflows/farm-copper.fnl` call symbols such as
`seq`, `action`, `repeat_until`, `is_full`, and `host.move`. **None of these are
defined in any `.fnl` file.** They are injected into Lua globals at runtime by
`setup_lua` in `src/lua.rs`:

- **`host.*`** — Rust closures registered in `register_host_functions` and
  `register_run_host_fns` (`src/lua.rs:168` and `:347`), stored under the global
  `host` table.
- **Lib exports** — `load_lib` (`src/lua.rs:75`) evaluates `actions.fnl`,
  `predicates.fnl`, `interp.fnl` and splats each returned export table's keys
  into globals via `globals.set(k, v)`. That is how `seq`, `action`,
  `repeat_until`, `repeat_n`, `when_pred`, `plan`, `run`, `set_actions`,
  `actions`, and the predicates (`is_full`, `hp_below`, `is_at`, `is_winnable`)
  become globally callable.

Because this happens at runtime, a static analyzer sees none of it and flags
every reference as an unknown global. Everything below is about closing that gap.

---

## Part A — Basic setup (do this first)

### A.1 Helix already has Fennel built in

Helix ships a **default** language config for Fennel: it points the language
server at the command `fennel-ls` and the formatter at `fnlfmt`, and the build in
use already has the tree-sitter parser + highlight queries. **No `languages.toml`
changes are required.** Confirm with:

```
hx --health fennel
```

Observed state before installing anything (the relevant lines):

```
Configured language servers:
  ✘ fennel-ls: 'fennel-ls' not found in $PATH
Configured formatter:
  ✘ 'fnlfmt' not found in $PATH
Tree-sitter parser: ✓
Highlight queries: ✓
```

So the only missing pieces are the binaries. After install, both ✘ should turn
green and the LSP attaches automatically.

(Helix config for this machine lives at `~/.config/helix` →
`/home/koss/opencode/dotfiles/config/helix`. `languages.toml` there currently
configures only `python` and `roc`; **leave Fennel to the built-in default**
unless we deliberately switch language servers — see A.4.)

### A.2 Install `fennel-ls`

There are **two** Fennel language servers. Helix's built-in default invokes the
command `fennel-ls`, which is **XeroOL's** fennel-ls (written in Fennel/Lua).
Prefer this one so we match the default and its docset mechanism (Part B needs
it).

```bash
git clone https://git.sr.ht/~xerool/fennel-ls
cd fennel-ls
make
make install PREFIX=$HOME      # installs to ~/bin; or: sudo make install
```

- Needs a Lua interpreter present (`make LUA=luajit` to pick one).
- Also packaged: AUR (`fennel-ls`, `fennel-ls-git`), nixpkgs, and unofficial
  `.deb`s at https://apt.technomancy.us.
- **No cargo/luarocks install for this server.**

Optionally also install `fnlfmt` (the formatter Helix already references) so
`:format` / auto-format works.

Re-run `hx --health fennel` to confirm both binaries are found.

### A.3 The alternative server (and why we are NOT using it)

rydesun's **`fennel-language-server`** (https://github.com/rydesun/fennel-language-server)
is written in Rust and installs via `cargo install --git …`. If we used it we
would have to override the command in `languages.toml`, and it has weaker
handling of unknown globals and no docset mechanism. **Decision: use XeroOL's
`fennel-ls`** — it matches Helix's default and supports the docset path in Part B.

### A.4 `flsproject.fnl` — suppress the false diagnostics

`fennel-ls` is configured by the **nearest `flsproject.fnl` walking up from the
edited file**. Create one at the **repo root** (`/home/koss/opencode/artifacts/`):

```fennel
{:fennel-path "./fennel/lib/?.fnl;./fennel/workflows/?.fnl;./fennel/?.fnl"
 :lua-version "lua5.4"
 :extra-globals "host seq action repeat_until repeat_n when_pred plan run set_actions actions is_full hp_below is_at is_winnable"}
```

`extra-globals` is a space-separated list of allowed global identifiers.
Per the manual: *"These identifiers and any of their fields will be considered
valid and won't produce diagnostics."* Listing `host` alone covers every
`host.*` access.

**Config fields that exist** (from the fennel-ls manual; defaults shown):

| Field | Default | Meaning |
| --- | --- | --- |
| `fennel-path` | `./?.fnl;./?/init.fnl;src/?.fnl;src/?/init.fnl` | module search path |
| `macro-path` | `./?.fnl;./?/init-macros.fnl;…` | macro search path |
| `lua-version` | `lua5.4` | `lua5.1`–`lua5.4`, `intersection`, `union` |
| `libraries` | `{}` | docset names → bool (this is the Part B hook) |
| `extra-globals` | `""` | space-separated allowed globals (suppression only) |
| `lints` | — | per-lint boolean flags |

### A.5 The hard limitation of Part A

**`extra-globals` only suppresses diagnostics. It gives NO completion, hover, or
signatures.** The manual is explicit that custom globals cannot be given types or
signatures via config. Part A makes editing quiet and usable; Part B is what
gives a real IDE experience.

One inherent nuance: because the lib files are loaded by Rust rather than via
Fennel `require`, fennel-ls cannot link a workflow's `seq` reference back to its
definition in `interp.fnl` — they are disconnected for static analysis. No config
bridges that; the docset in Part B is the way to surface signatures/docs.

---

## Part B — Full experience via a generated docset

### B.1 What a docset is

fennel-ls reads **docsets**: Lua files in `~/.local/share/fennel-ls/docsets/`
(XDG-respecting), enabled per-project via `flsproject.fnl`:

```fennel
{:libraries {:artifacts true}}   ;; loads ~/.local/share/fennel-ls/docsets/artifacts.lua
```

A docset declares the API surface so fennel-ls can offer **completion + hover +
signatures**. Functions in a docset carry Fennel's standard metadata keys
`:fnl/docstring` and `:fnl/arglist` (the same keys Fennel itself uses). Working
templates to copy the exact file shape from: technomancy's extractor at
https://git.sr.ht/~technomancy/fennel-ls-docsets (love2d / tic-80). **TODO on
resume: pull one of those files as the concrete format template** — sourcehut
502'd during research so the exact byte-level structure was not captured here.
Related: https://github.com/jaawerth/fnldocstor documents the metadata format.

### B.2 Can we generate it from `lua.rs`? Can mlua introspect?

**Partly — and the split matters.** Runtime introspection (mlua + Lua) reliably
yields the **set of names** and whether each is a function/table, but **not**
signatures, parameter names, types, or docstrings for Rust-backed functions.
Those exist only in source.

What mlua 0.10.5 (`mlua = { version = "0.10", features = ["lua54","vendored"] }`,
`Cargo.toml:18`) actually gives us:

| Want | At runtime? | How |
| --- | --- | --- |
| Names in the `host` table | ✅ | iterate `host.pairs::<String, Value>()` |
| function vs table vs value | ✅ | the `Value` discriminant |
| Which globals *we* injected | ✅ | diff globals vs a fresh `Lua::new()`, or enumerate the known export tables |
| `host.*` param names / types / arity | ❌ | Rust closures are `what = "C"` to Lua; `Function::info()` returns only source/line/`what` — useless here |
| Fennel lib fn arglist + docstring | ⚠️ conditional | via the Fennel `metadata` table (see B.3) |

So mlua's real contribution is **table enumeration** — enough to keep the *name
set* honest, not enough to build signatures. `Function::info()` is the only
"introspection" mlua exposes and it is empty for Rust closures.

### B.3 Where the signatures actually live (two halves)

The surface divides cleanly, and each half is sourced differently:

- **Host half (`host.*`, Rust).** The signature is the Rust tuple in each
  `lua.create_function(|_, (x1,y1,x2,y2): (i32,i32,i32,i32)| …)` plus the `///`
  doc comment in `src/lua.rs`. **Source-only**; nothing extracts it
  automatically without a proc-macro or parsing the Rust. → hand-authored, but
  change-detected (see B.4).

- **Lib half (`seq`, `action`, `repeat_until`, `is_full`, …, Fennel).** These
  have real arglists, and several already have docstrings (e.g. every predicate
  in `predicates.fnl`). Fennel stores `:fnl/arglist` and `:fnl/docstring` in the
  compiler's `metadata` table when compiled with metadata enabled (Fennel's
  default). Since `setup_lua` already holds the `fennel` table
  (`src/lua.rs:36`), we can read e.g.
  `(fennel.metadata:get f :fnl/arglist)` / `:fnl/docstring` for each exported
  global. → **fully auto-generatable, docstrings included.**

  Caveat to verify on resume: metadata must be enabled when `load_lib` evals the
  libs. Fennel enables metadata by default, but `load_lib` currently passes only
  `{filename}` as opts (`src/lua.rs:76`); confirm metadata is present (or pass
  the option explicitly) before relying on extraction. `interp.fnl`'s
  constructors (`seq`, `action`, …) have arglists but no docstrings; that is
  fine — arglist alone is a useful signature.

### B.4 Recommended design

A single Rust source-of-truth for the host half, runtime extraction for the lib
half, and two thin CLI subcommands. The CLI is a hand-rolled `match` on the
subcommand string in `src/main.rs:40` (no clap), so adding subcommands is
trivial — follow the existing `plan` / `run` arms.

**1. A `const` host-API table in Rust** — the one place host signatures are
hand-written, and ideally the same table that drives registration:

```rust
struct HostFn { name: &'static str, arglist: &'static str, doc: &'static str }

const HOST_FNS: &[HostFn] = &[
    HostFn { name: "path_hops", arglist: "[x1 y1 x2 y2]",
             doc: "Integer hop count via A* (Manhattan fallback)." },
    HostFn { name: "find_tile", arglist: "[content-type code]",
             doc: "Nearest map tile {x y} carrying that content, from spawn." },
    // … one entry per registered host fn …
];
```

Drive both `register_host_functions` *and* the docset from this table where
practical. Then add a **unit test asserting `HOST_FNS` names == the live `host`
table keys** after `setup_lua(None, None, None)`. That is the introspectable
invariant: you can never add/remove a host fn without updating `HOST_FNS`.
(The test **cannot** verify an arglist is correct — runtime has no ground truth
for Rust closure params — so say that explicitly in the test comment. That gap is
irreducible and small: the hand-written arglist sits right next to the
`create_function` call it describes.)

**2. `artifacts gen-docset`** — boots `setup_lua(None, None, None)`, then writes:
   - the docset `.lua`: **host half** from `HOST_FNS`; **lib half** pulled from
     `fennel.metadata` (arglists + docstrings, automatic);
   - the `extra-globals` string for `flsproject.fnl`, from the enumerated name
     set (keeps Part A's config honest too).

   Target path: `~/.local/share/fennel-ls/docsets/artifacts.lua` (and/or a
   repo-tracked copy, e.g. `fennel/artifacts.docset.lua`, that an install step
   symlinks/copies — decide on resume; a tracked copy is what `check` diffs
   against).

**3. `artifacts check-docset`** — regenerates in memory and diffs against the
   committed docset; **non-zero exit on mismatch**. Wire into CI / a `#[test]`.
   Because both sides derive from the same generator, this is an **exact string
   diff**, so it reliably catches: a new/removed host fn, a renamed lib export, a
   changed Fennel arglist or docstring. This is the "is it up to date?" command
   asked for.

### B.5 What is automatic vs hand-authored (the honest trade-off)

- **Fully automatic & exact:** the *name set* (both halves) and the *lib
  signatures + docstrings*.
- **Hand-authored but change-detected:** the *host signatures + docstrings*. Each
  `host.*` arglist is written once in `HOST_FNS`; `check-docset` then guarantees
  the committed docset never drifts from it, and the name-set unit test
  guarantees `HOST_FNS` never drifts from the live `host` table.
- **The one thing no tool can verify:** whether a hand-written host arglist
  actually matches the Rust closure's real params. That is the irreducible gap,
  and it is localized to the `create_function` site.

This is strictly better than Part A alone: real hover/completion, and staleness
becomes a CI failure instead of something you discover when the LSP lies to you.

---

## Appendix — the complete injected surface (as of this writing)

Enumerated from `src/lua.rs`. This is the full list of globals a workflow file
can reference; it is what `extra-globals` lists and what the docset must cover.

### Lib globals (installed by `load_lib`)

| Symbol | Source | Arglist / signature |
| --- | --- | --- |
| `seq` | interp.fnl | `[...steps]` → seq node |
| `action` | interp.fnl | `[op ...args]` → action node |
| `repeat_until` | interp.fnl | `[pred label ...steps]` |
| `repeat_n` | interp.fnl | `[n ...steps]` |
| `when_pred` | interp.fnl | `[pred ...steps]` |
| `plan` | interp.fnl | `[wf st]` → plan report |
| `run` | interp.fnl | `[wf]` |
| `set_actions` | interp.fnl | `[actions-tbl]` |
| `actions` | actions.fnl | the action table |
| `is_full` | predicates.fnl | `[st]` (has docstring) |
| `hp_below` | predicates.fnl | `[threshold st]` (has docstring) |
| `is_at` | predicates.fnl | `[x y st]` (has docstring) |
| `is_winnable` | predicates.fnl | `[monster st]` (has docstring) |

Note the deliberate underscore naming (`repeat_until`, `is_full`, …): Fennel
mangles `-`/`?` in a bare global reference, so the exported keys use
Lua-identifier-safe names. See the headers of `predicates.fnl` and `interp.fnl`,
and the `feedback_fennel_conventions` memory.

### `host` table — pure / plan-context fns (`register_host_functions`)

| Fn | Arglist | Notes |
| --- | --- | --- |
| `host.cooldown_cost` | `[op params]` | op ∈ movement/gathering/fight/rest/crafting/recycling/deposit |
| `host.gather_yield` | `[tile]` | → `{code quantity}` |
| `host.resource_level` | `[tile]` | → u32 |
| `host.path_hops` | `[x1 y1 x2 y2]` | A* or Manhattan fallback |
| `host.find_tile` | `[content-type code]` | → `{x y}` nearest from spawn |
| `host.monster_stats` | `[code]` | → combat stats + `drops` |
| `host.simulate_fight` | `[st monster-stats]` | → `{result turns hp_remaining}` |

### `host` table — run-context fns (`register_run_host_fns`)

Present only when `setup_lua` is given a `Character`; in plan context these are
**stubbed to error loudly** (`src/lua.rs:325`). The docset should still document
them (they are part of the surface workflows may call in `:run`).

| Fn | Arglist |
| --- | --- |
| `host.gather` | `[]` |
| `host.move` | `[x y]` |
| `host.fight` | `[]` |
| `host.rest` | `[]` |
| `host.deposit_item` | `[code qty]` |
| `host.deposit_all` | `[]` |
| `host.view` | `[]` → predicate-facing state table |

---

## Resume checklist

- [ ] **Part A:** install `fennel-ls` (XeroOL's), optionally `fnlfmt`; confirm
      `hx --health fennel` is green.
- [ ] **Part A:** add `flsproject.fnl` at repo root (B.1 adds `:libraries` once
      the docset exists).
- [ ] **Part B prep:** pull a real docset file from technomancy's repo to lock
      the exact `.lua` format (B.1 TODO).
- [ ] **Part B prep:** confirm Fennel metadata is available on the lib functions
      after `load_lib` (B.3 caveat); adjust `load_lib` opts if needed.
- [ ] **Part B:** introduce `HOST_FNS` + the name-set unit test.
- [ ] **Part B:** implement `gen-docset` and `check-docset` subcommands in
      `src/main.rs`; wire `check-docset` into CI.

## Key references

- fennel-ls manual: https://xerool.net/fennel-ls/docs/manual.html
- fennel-ls install: https://xerool.net/fennel-ls/docs/installation.html
- fennel-ls project: https://sr.ht/~xerool/fennel-ls/
- docset templates: https://git.sr.ht/~technomancy/fennel-ls-docsets
- docset metadata format: https://github.com/jaawerth/fnldocstor
- alt server (not used): https://github.com/rydesun/fennel-language-server
- internals this leans on: `src/lua.rs` (`setup_lua`, `load_lib`,
  `register_host_functions`, `register_run_host_fns`), `src/main.rs` (CLI match),
  `docs/ARCHITECTURE.md`.
