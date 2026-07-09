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
- Show the character's current task + its rewards (from `/tasks/list`). Task
  reference data was deliberately NOT cached for sim accuracy (the assigned task
  and its rewards are random server state, not fixable with reference data), so
  it's a display-only concern that belongs here — see the "Recipe / GE / task
  reference data" entry below for the reasoning.
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

# Map cache staleness vs. ephemeral event content
Context: Surfaced while trying to run a live NPC-buy workflow. The overworld map uses the same 24h TTL disk cache as monster data (`TTL` in `src/data.rs`), on the assumption the map is static. It isn't: the map carries **ephemeral event content**. `fish_merchant` (a gold merchant selling `gudgeon @ 10`) was present when we first observed it, then left the world as a timed event before we could buy — but a cache written during its visit keeps reporting it (and, conversely, a cache written while it's absent misses it). Result: `host.find_tile` resolves phantom merchants or misses live ones, so a planned/queued purchase workflow silently targets a tile that no longer has content. Refetching every launch is too costly (5 paged `/maps` calls). Note this is distinct from the `(x,y)` collision dedup already fixed in `GameMap::insert` — that fix can't help when the content simply isn't in the current feed.
Goal: A cheap/intelligent freshness signal to bust *just the map* cache when event content changes — e.g. poll an events endpoint / event log (https://docs.artifactsmmo.com/concepts/events/), key the map cache on the active-events set, or give map content a much shorter TTL than static reference data.
Deliverable: PR I can review on github
