# Spike: Dynamic workflows
Context: Farm 'X' is a fairly generic task, rather than needing to write the same workflow with different targets, can we parameterize the workflows for what we're after? This leads us to higher order workflows where workflows can import other workflows. For example, the TUI could provide what resource we want to seek out and gather, or we could create an algorithm starting from what we want to craft and work backwards to what we have in the bank vs what we need to gather (harvest or fight to get, etc). This can feed into a TUI v2 where the user can select something they want to craft and automatically generate the workflow for it
Goal: Use the same skeletal workflow, but parameterize it based on either a CLI parameter or another workflow invoking it. Unlock higher order workflows.
Deliverable: `plans/DYNAMIC_WORKFLOWS.md`

# Spike: Achievement workflows
Context: There are achievements in this game. Can we generate workflows that can work through achievement lists?
Goal: Automation system around achievements, which is fully scripted in fennel layer
Deliverable: `plans/ACHIEVEMENTS.md`

# TUI: Workflow Editing
Context: Once we have a TUI, one thing we'll want to be able to do is add, edit, or delete workflows while the TUI is active. Ideally we can launch $EDITOR for this, and the file names are appropriate enough to trigger built in language server providers. We'll also want to ensure the LSPs can access any host information / autocomplete the built in stubs and helpers we provide
Goal: Manage workflows within the TUI using preferred editor
Deliverable: PR I can review on github

# Spike, TUI: v2 stretch goals
Context: The following items didn't make it into the v1 UI. We should pick apart some items to address in a v2
- Display what tiles have monsters or resources on the map, identify what they drop.
- Display what crafters are on the map, what they can craft
- Display what merchanges are on the map, what they will buy/sell.
- XP / drops / gold **ticker** — a rate/delta feed (XP-per-hour, a drops log,
  gold gained this run) by diffing successive `SharedView` snapshots. The static
  header **values** (xp bar, gold) and the cooldown bar are in v1 (§3.8); only
  the time-series *deltas* are backlog.
- Mini overworld map from the fetched `GameMap`.
- Bank contents panel (needs a new `GET /my/bank/items` driver method).
- Mid-run reconcile / drift alarm.
- **Abortable cooldown sleep** for near-instant cancel.
- Graceful "stop at boundary" (drain outcomes) instead of hard kill.
Goal: Identify more TUI ergonomics or features worth adding and how we might do so
Deliverable: `plans/TUI_v2.md`

# Intents: Recipe / GE / task reference data
Context: Surfaced while wiring all character-action intents. Recipes, GE order books, and task definitions aren't loaded client-side (unlike monsters/map), so `craft`'s `:sim` adds output without consuming inputs, `recycle` doesn't add salvage, and GE/task sims are neutral. Fetching + caching this reference data would let these sims be real.
Goal: Fetch/cache recipe, GE, and task reference data so craft/GE/task plan-pass sims are accurate.
Deliverable: PR I can review on github
