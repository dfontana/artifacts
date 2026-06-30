# Spike: Progress bars on running workflows?
Context: How can we get feedback on a running workflow? Maybe best to just use the artifacts live viewer rather than attempt this in the CLI, avoiding complications.
Goal: Weight the pros and cons of adding progress bar communication and outputs from Fennel -> Rust -> event loop
Deliverable: `plans/PROGRESS_BARS.md`

# Spike: TUI for Character state & Workflows
Context: While artifacts provides a rich interface for viewing characters, it would be nice to launch a simple TUI which shows character stats (and location) & inventory state in a table; a selectable list of workflows the user has defined; and the ability to execute simulations or actual workflow runs (including spinners for representing running workflows). The TUI would utilize colors and font-icons where appropriate to keep information dense and easy to navigate. Should be based on ratatui.rs. The character stats and inventory would update as the workflow progresses.
Goal: Rather than use a CLI to trigger workflows, we can use a TUI to do so.
Deliverable: `plans/TUI.md`
