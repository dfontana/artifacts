---
name: tui-iteration
description: How to test and iterate on the ratatui TUI (`artifacts tui <character>`) by remote-controlling a zellij pane — launch it, drive keypresses, and capture screen dumps to see the actual rendered layout without a human at the keyboard. Use whenever changing code under src/tui or verifying a TUI behavior/layout claim.
---

# TUI Iteration via Zellij Remote Control

This is a **living document**. The TUI (`src/tui`, invoked as `artifacts tui <character>`)
is a ratatui full-screen app — it can't be driven by piping stdin/stdout like a normal CLI.
Zellij's `action` subcommands let us open a pane, type into it, and read back its rendered
screen, which is what makes iteration possible without a human watching. Update this file
when the loop below breaks or when you find a better way to drive/observe the TUI.

## Prerequisites

- Build first, outside the TUI's raw-mode screen, so compile errors show up as normal
  terminal output instead of getting swallowed or garbling the alt-screen:
  ```
  cargo build --bin artifacts
  ```
- Known test character: `nillinbot` (see live-integration-test skill / project memory for
  account details). Run with `./target/debug/artifacts tui nillinbot`.

## 1. Open a pane

```
zellij action new-pane -d right --cwd "$(pwd)" --tab-id "$(zellij action current-tab-info --json | jq '.tab_id')" -n tui-test -- bash
```
This works even if the current shell shows `ZELLIJ=0`, as long as a zellij session is
active (check `zellij list-sessions`) — the CLI targets it via `ZELLIJ_SESSION_NAME` in
the environment. It prints a pane id like `terminal_2` — save it, every following command
targets this pane explicitly with `-p`.

## 2. Get env vars into the pane

The new pane is a fresh bash shell and does **not** inherit env vars you sourced in your
own shell (e.g. `ARTIFACTS_TOKEN`/`ARTIFACTS_SECRET` from `.envrc`). Source them inside the
pane itself as part of the launch command, don't assume they're already there.

## 3. Send a command into the pane

`write-chars` only types the text — it does not press Enter, so always follow it with a
`send-keys ... Enter`:
```
zellij action write-chars -p terminal_2 "set -a; source .envrc; set +a; ./target/debug/artifacts tui nillinbot"
zellij action send-keys -p terminal_2 Enter
```

## 4. Observe the screen

```
zellij action dump-screen -p terminal_2
```
Prints the rendered viewport to stdout (add `--full` for scrollback, `--path <file>` to
write to a file, `--ansi` to keep styling). This is how you "see" the TUI without a
screenshot — read the box-drawing output as the actual layout.

## 5. Interact and re-observe

Drive the TUI the same way a user would, one `send-keys` call per keystroke/chord, then
dump-screen again to see the effect:
```
zellij action send-keys -p terminal_2 Down
zellij action send-keys -p terminal_2 Enter
zellij action dump-screen -p terminal_2
```
Use this to click through focus panels, select workflows, trigger runs (`r`), toggle zoom
(`z`), etc. — whatever the change under test needs to exercise.

**Key-sending gotchas:**

- `send-keys` takes **named** keys (`Enter`, `Esc`, `Down`, `Left`, `Tab`, single chars like
  `q`/`z`/`]`). Do **not** feed it raw bytes: `send-keys "$(printf '\r')"` fails with
  `empty key` — use the name `Enter`.
- For **modified** keys (Shift/Ctrl+Arrow) `send-keys` has no name, so push the raw CSI
  escape as literal input with `write-chars`:
  ```
  zellij action write-chars -p terminal_2 "$(printf '\033[1;2A')"   # Shift+Up
  ```
  The final letter picks the arrow: `A` up, `B` down, `C` right, `D` left; `1;2` is Shift
  (`1;5` Ctrl, `1;3` Alt). The app's crossterm parses these as the real modified key events.
- A **panic** in the TUI drops the process out of raw mode and prints the panic message as
  normal pane text — so `dump-screen` captures it. If a dump shows a shell prompt plus a
  `panicked at …` line, the TUI crashed; read the message, don't assume a capture glitch.

## 6. Rebuilding after a code change

The running process holds the old binary in memory — rebuilding on disk does not hot-swap
it. After `cargo build --bin artifacts` again:
1. Kill the running TUI in the pane: `zellij action send-keys -p terminal_2 Ctrl c`
   (or 'q' if the TUI is in a state that accepts a quit keybind).
2. Relaunch it (step 3) before dumping the screen again.

## Loop summary

build → launch in pane → send-keys to interact → dump-screen to observe → repeat,
rebuilding/relaunching whenever source changes.

## Known issues / open questions

(Keep this section updated as we hit problems.)

- **Focus/selection highlight is usually color-only, so a plain `dump-screen` won't show
  it.** A focused pane typically changes only its border/​title *color*, not the box-drawing
  characters, and plain `dump-screen` strips color. To confirm which pane is focused either
  dump with `--ansi` and read the escape codes, or verify *functionally* — e.g. enter a
  pane's interact mode and check that the footer/mode line shows that pane's bindings. The
  functional check is faster and less noisy than parsing ANSI.

- **`dump-screen` output has been reliable so far — trust it.** While testing
  the Focus-mode zoom (a `Clear` + centered 70%x70% modal over the normal
  two-column body), `dump-screen` showed fragments of the underlying
  STATS/WORKFLOWS/PLAN/RUN panes visible around the modal's edges. This
  first looked like a capture bug (plain and `--ansi` dumps matched each
  other suspiciously well), then — after a hasty read of a user-provided
  screenshot — looked like a real layout bug (background bleeding through
  a supposedly full overlay). Both readings were wrong: it's the intended
  design. The zoom is a floating modal over a visible backdrop (`render_body`
  draws the full layout every frame; the modal covers only the centered
  region on top of it, deliberately leaving the surrounding panes visible),
  the same pattern as a dialog box over a dimmed background. `dump-screen`
  was accurate the whole time — it matched the screenshot exactly once
  actually compared carefully. **Lesson:** don't assume a capture tool is
  wrong just because the output looks unusual, and don't assume unusual
  output is an app bug either — an overlay with a visible backdrop is a
  legitimate, common UI pattern. Confirm the *design intent* (ask, or check
  the code/comments) before calling something a bug at all.
