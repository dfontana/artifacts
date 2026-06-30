# Spike: Progress bars on running workflows?
Context: How can we get feedback on a running workflow? Maybe best to just use the artifacts live viewer rather than attempt this in the CLI, avoiding complications.
Goal: Weight the pros and cons of adding progress bar communication and outputs from Fennel -> Rust -> event loop
Deliverable: `plans/PROGRESS_BARS.md`

# Spike: TUI for Character state & Workflows
Context: While artifacts provides a rich interface for viewing characters, it would be nice to launch a simple TUI which shows character stats (and location) & inventory state in a table; a selectable list of workflows the user has defined; and the ability to execute simulations or actual workflow runs (including spinners for representing running workflows). The TUI would utilize colors and font-icons where appropriate to keep information dense and easy to navigate. Should be based on ratatui.rs. The character stats and inventory would update as the workflow progresses.
Goal: Rather than use a CLI to trigger workflows, we can use a TUI to do so.
Deliverable: `plans/TUI.md`

# Spike: Achievement workflows
Context: There are achievements in this game. Can we generate workflows that can work through achievement lists?
Goal: Automation system around achievements, which is fully scripted in fennel layer
Deliverable: `plans/ACHIEVEMENTS.md`

# TUI: Workflow Editing
Context: Once we have a TUI, one thing we'll want to be able to do is add, edit, or delete workflows while the TUI is active. Ideally we can launch $EDITOR for this, and the file names are appropriate enough to trigger built in language server providers. We'll also want to ensure the LSPs can access any host information / autocomplete the built in stubs and helpers we provide
Goal: Manage workflows within the TUI using preferred editor
Deliverable: PR I can review on github

# Spike: Dynamic workflows
Context: Farm 'X' is a fairly generic task, rather than needing to write the same workflow with different targets, can we parameterize the workflows for what we're after? This leads us to higher order workflows where workflows can import other workflows.
Goal: Use the same skeletal workflow, but parameterize it based on either a CLI parameter or another workflow invoking it.
Deliverable: `plans/DYNAMIC_WORKFLOWS.md`
