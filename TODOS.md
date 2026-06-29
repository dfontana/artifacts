# Diagram the Repo's structure
Context: It's unclear how files inside `src/` map to pruposes. There's `live.rs`, `planner.rs`, `scheduler.rs` which all map to specific modes from the cli in `main.rs`. Then there's `character.rs` and `view.rs` which aren't clear what they are for. `lua.rs` seems to glue some of these together. Point: The only obvious connection in the `src/` folder is how `driver.rs` maps to `core/` crate. It should be more clearly stated in a `docs/ARCHITECTURE.md` how all user intended game logic should be defined in fennel files, and how those then map back to the underlying codebase. Some of the existing README.md can be moved into this document to avoid duplication of docs.
Goal: Help both humans and agents better understand how code relates to each other, to make more targeted edits and understand high level orchestration
Deliverable: `docs/ARCHITECTURE.md`

# Combine Simulate & Estimate
Context: Estimate approximates how much a workflow will cost while simulate seems to only go one step further to indicate if it's 'feasible'. Neither consider a character and the current state of said character. This should be improved by first combining the functionalities to one (simplfiying the overall surface) and then starting the process from the current state of a given character rather. This will net us a more accurate view of whether the character is able to take the next action and the chance of success (eg fight failure/success, ability to move, craft, harvest, etc).
Goal: Simplify two commands that seem very similar and make the overall process more accurate. If we can estimate success of a workflow, we can start considering higher order planning (choosing workflows, etc).
Deliverable: A PR with the change for human review.

# Spike: Fight Support
Context: We have some of the groundwork in the `core/` for fight support, but none of the fennel support to actually: identify fight options, plan which fights are feasible to take on (at what success chance), execute the fight, and then claim rewards. There's a number of dimensions that have to be considered, such as how much inventory room there is to collect rewards after the fight, or what items are in the inventory (or even the bank) that can be used to make the fight more successful. The planning stages should account for this and recommend what actions need to be taken to get success rate up. The execution state should be able to utilize that output. All of this should be scriptable from Fennel, following the same design choices as movement or gathering.
Goal: Identify what needs to happen to support combat
Deliverable: `plans/FIGHT_SUPPORT.md`

# Spike: Progress bars on running workflows?
Context: How can we get feedback on a running workflow? Maybe best to just use the artifacts live viewer rather than attempt this in the CLI, avoiding complications.
Goal: Weight the pros and cons of adding progress bar communication and outputs from Fennel -> Rust -> event loop
Deliverable: `plans/PROGRESS_BARS.md`

# Spike: TUI for Character state & Workflows
Context: While artifacts provides a rich interface for viewing characters, it would be nice to launch a simple TUI which shows character stats (and location) & inventory state in a table; a selectable list of workflows the user has defined; and the ability to execute simulations or actual workflow runs (including spinners for representing running workflows). The TUI would utilize colors and font-icons where appropriate to keep information dense and easy to navigate. Should be based on ratatui.rs. The character stats and inventory would update as the workflow progresses.
Goal: Rather than use a CLI to trigger workflows, we can use a TUI to do so.
Deliverable: `plans/TUI.md`

