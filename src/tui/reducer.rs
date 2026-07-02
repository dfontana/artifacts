//! The pure cursor reducer (`plans/TUI.md` §3.3): fold the append-only progress
//! id-log into a per-row glyph state. Pure Rust — no Lua, no network — which is
//! exactly why the tricky logic (loop `k/N`, when-skip, terminal frames) is
//! unit-testable. This is the primary test gate.

use std::collections::HashMap;

use crate::tui::skeleton::{PlanStep, StepKind};
use crate::tui::NodeId;

/// The run's terminal disposition, passed in so the reducer renders the final
/// frame correctly: without it the last fired id would read as *active* forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunPhase {
    Running,
    Done,
    Failed,
}

/// The state of one skeleton row for the current frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    /// Not yet reached.
    Pending,
    /// The live cursor (latest fired id) while running.
    Active,
    /// Reached and completed.
    Done,
    /// A guarded row whose `when` fired but which was itself skipped this pass.
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowState {
    pub cell: Cell,
    /// Loop rows only: `(k, N)` — iterations counted so far and the denominator
    /// (`None` → the plan never resolved it, renders `k/?`). `k` may exceed `N`
    /// on plan/reality divergence; the reducer never panics, the renderer shows
    /// `>N`.
    pub iter: Option<(u32, Option<u32>)>,
    /// True on the single row a `Failed` run died on (the last log entry).
    pub death: bool,
}

/// Fold `log` (the ordered node-id progress log) over `skeleton` into per-row
/// states. `phase` is the run's terminal disposition. Pure and total.
pub fn reduce(skeleton: &[PlanStep], log: &[NodeId], phase: RunPhase) -> Vec<RowState> {
    let n = skeleton.len();

    // id → row index, precomputed once. This reduce runs every frame; a linear
    // `skeleton.position` per log entry was the one O(rows × log) hot spot. First
    // occurrence wins (matching `position`). `:seq` ids fire but map to no row,
    // so they are simply absent from the map (skipped here and everywhere below).
    let mut row_of: HashMap<NodeId, usize> = HashMap::with_capacity(n);
    for (i, s) in skeleton.iter().enumerate() {
        row_of.entry(s.id).or_insert(i);
    }

    // One pass over the log records every id's first and last log position —
    // enough to answer "did id fire in this window?" and "where does the current
    // loop activation start?" in O(1), instead of re-scanning the log per row.
    let mut first_pos: HashMap<NodeId, usize> = HashMap::new();
    let mut last_pos: HashMap<NodeId, usize> = HashMap::new();
    for (p, &id) in log.iter().enumerate() {
        first_pos.entry(id).or_insert(p);
        last_pos.insert(id, p);
    }

    // Cursor = the row of the last logged id that maps to a row.
    let active_row: Option<usize> = log.iter().rev().find_map(|&id| row_of.get(&id).copied());

    // Inclusive body-end row index for each loop header (a flat depth walk).
    let mut loop_end: Vec<Option<usize>> = vec![None; n];
    for (h, s) in skeleton.iter().enumerate() {
        if s.kind == StepKind::Loop {
            let d = s.depth;
            let mut j = h + 1;
            while j < n && skeleton[j].depth > d {
                j += 1;
            }
            loop_end[h] = Some(j - 1); // == h when the loop has no rows under it
        }
    }

    // For each loop header, the log index where its CURRENT activation begins:
    // the last fire of its body's first node, else (nothing counted yet) the
    // header's first fire. Computed once so body rows don't each re-scan the log.
    let mut loop_window_start: Vec<usize> = vec![0; n];
    for (h, s) in skeleton.iter().enumerate() {
        if s.kind == StepKind::Loop {
            loop_window_start[h] = s
                .loop_start_id
                .and_then(|id| last_pos.get(&id).copied())
                .or_else(|| first_pos.get(&s.id).copied())
                .unwrap_or(0);
        }
    }

    // Innermost enclosing loop header for a row (the deepest = largest index).
    let innermost = |i: usize| -> Option<usize> {
        let mut best = None;
        for (h, end) in loop_end.iter().enumerate().take(i) {
            if let Some(end) = end {
                if i <= *end {
                    best = Some(h);
                }
            }
        }
        best
    };

    let mut out = Vec::with_capacity(n);
    for (i, step) in skeleton.iter().enumerate() {
        match step.kind {
            StepKind::Loop => {
                let end = loop_end[i].unwrap_or(i);
                // Anchor to the LAST fire of the loop header, not the first. A
                // loop nested inside an outer loop re-enters (and re-fires its
                // header id) once per outer pass, so counting `loop_start_id`
                // from the first header fire would sum every outer pass's inner
                // iterations (k grows unbounded, e.g. 6/3). The last fire scopes
                // k to the current activation — consistent with the body-row
                // window below, which also `rposition`s to the current turn. For
                // a top-level loop the header fires exactly once, so first == last
                // and this is identical to the old behavior. `None` ⇒ not reached.
                let Some(&ih) = last_pos.get(&step.id) else {
                    // Loop not reached yet.
                    out.push(RowState {
                        cell: Cell::Pending,
                        iter: Some((0, step.count)),
                        death: false,
                    });
                    continue;
                };
                // k = times the body's first node fired in this activation.
                let k = match step.loop_start_id {
                    Some(s) => log[ih + 1..].iter().filter(|&&x| x == s).count() as u32,
                    None => 0,
                };
                let past = active_row.is_some_and(|a| a > end);
                // Header fired ⇒ cursor is at or past it. Done when the run is
                // over or the cursor has left the loop; otherwise the loop is
                // current (this also covers a Failed frame inside the loop).
                let cell = if phase == RunPhase::Done || past {
                    Cell::Done
                } else {
                    Cell::Active
                };
                out.push(RowState {
                    cell,
                    iter: Some((k, step.count)),
                    death: false,
                });
            }
            _ => {
                // Window start: 0 = the whole log, or — inside an actively-running
                // loop — the current iteration's first log index, so a body row
                // reflects THIS turn of the loop (rest toggles skipped/active as
                // it turns). Precomputed above, so no per-row log scan here.
                let start = match innermost(i) {
                    Some(h) => {
                        let end = loop_end[h].unwrap_or(h);
                        let loop_active = phase == RunPhase::Running
                            && active_row.is_some_and(|a| a >= h && a <= end);
                        if loop_active {
                            loop_window_start[h]
                        } else {
                            0
                        }
                    }
                    None => 0,
                };

                let is_cursor = active_row == Some(i);
                // "id fired in the window" ⇔ its last log position is at/after the
                // window start — an O(1) check in place of scanning the suffix.
                let fired = last_pos.get(&step.id).is_some_and(|&p| p >= start);
                let guard_fired = step
                    .guard_id
                    .and_then(|g| last_pos.get(&g))
                    .is_some_and(|&p| p >= start);
                let cell = if is_cursor {
                    match phase {
                        RunPhase::Running => Cell::Active,
                        _ => Cell::Done,
                    }
                } else if fired {
                    Cell::Done
                } else if guard_fired {
                    Cell::Skipped
                } else {
                    Cell::Pending
                };
                out.push(RowState {
                    cell,
                    iter: None,
                    death: is_cursor && phase == RunPhase::Failed,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(id: NodeId, depth: u16, op: &str, guard: Option<NodeId>) -> PlanStep {
        PlanStep {
            id,
            depth,
            kind: StepKind::Action,
            op: op.into(),
            args: vec![],
            label: None,
            count: None,
            loop_start_id: None,
            guard_id: guard,
        }
    }

    fn loop_row(id: NodeId, depth: u16, start: NodeId, count: Option<u32>) -> PlanStep {
        PlanStep {
            id,
            depth,
            kind: StepKind::Loop,
            op: "repeat-until".into(),
            args: vec![],
            label: Some("iters".into()),
            count,
            loop_start_id: Some(start),
            guard_id: None,
        }
    }

    fn when_row(id: NodeId, depth: u16) -> PlanStep {
        PlanStep {
            id,
            depth,
            kind: StepKind::When,
            op: "when".into(),
            args: vec![],
            label: None,
            count: None,
            loop_start_id: None,
            guard_id: None,
        }
    }

    fn cells(rows: &[RowState]) -> Vec<Cell> {
        rows.iter().map(|r| r.cell).collect()
    }

    /// One table-driven sweep over the reducer's tricky cases (§7 Tier 1):
    /// linear progression, loop k/N, when-skip, back-to-back fires in one drain,
    /// and plan/reality divergence (k > N, no panic).
    #[test]
    fn reducer_sweep() {
        // ─── linear progression ───────────────────────────────────────────────
        let linear = vec![
            action(10, 0, "gather", None),
            action(11, 0, "gather", None),
            action(12, 0, "deposit-all", None),
        ];
        assert_eq!(
            cells(&reduce(&linear, &[10], RunPhase::Running)),
            vec![Cell::Active, Cell::Pending, Cell::Pending],
        );
        assert_eq!(
            cells(&reduce(&linear, &[10, 11], RunPhase::Running)),
            vec![Cell::Done, Cell::Active, Cell::Pending],
        );
        assert_eq!(
            cells(&reduce(&linear, &[10, 11, 12], RunPhase::Done)),
            vec![Cell::Done, Cell::Done, Cell::Done],
        );

        // ─── loop iter k/N (farm-copper shape) ─────────────────────────────────
        // 0 travel · 1 loop(start=3, N=3) · 2 gather[in loop] · 3 deposit
        let copper = vec![
            action(1, 0, "travel-to", None),
            loop_row(2, 0, 3, Some(3)),
            action(3, 1, "gather", None),
            action(4, 0, "deposit-all", None),
        ];
        // seq id 0 is inert; two gathers done, mid-third.
        let r = reduce(&copper, &[0, 1, 2, 3, 3, 3], RunPhase::Running);
        assert_eq!(
            cells(&r),
            vec![Cell::Done, Cell::Active, Cell::Active, Cell::Pending],
        );
        assert_eq!(r[1].iter, Some((3, Some(3))), "loop header shows k/N");

        // Completed loop, then deposit done → all done, k stayed at N.
        let done = reduce(&copper, &[0, 1, 2, 3, 3, 3, 4], RunPhase::Done);
        assert_eq!(
            cells(&done),
            vec![Cell::Done, Cell::Done, Cell::Done, Cell::Done],
        );
        assert_eq!(done[1].iter, Some((3, Some(3))));

        // ─── plan/reality divergence: run exceeds predicted N, no panic ────────
        let diverge = reduce(&copper, &[0, 1, 2, 3, 3, 3, 3, 3], RunPhase::Running);
        assert_eq!(diverge[1].iter, Some((5, Some(3))), "k may exceed N");

        // ─── nested loop: inner k/N resets each outer iteration ────────────────
        // 0 travel · 1 outer loop(start=30,N=2) · 2 inner loop(start=40,N=3)
        //   · 3 gather[inner body] · 4 deposit. The inner header (30) fires once
        // per outer pass, so anchoring k to its FIRST fire would sum both passes
        // (5/3); the reducer anchors to its LAST fire → current pass only.
        let nested = vec![
            action(10, 0, "travel-to", None),
            loop_row(20, 0, 30, Some(2)),
            loop_row(30, 1, 40, Some(3)),
            action(40, 2, "gather", None),
            action(50, 0, "deposit-all", None),
        ];
        // Outer iter 1 fully ran the inner loop (3 gathers); now mid outer iter 2
        // with 2 inner gathers so far.
        let nest = reduce(&nested, &[10, 20, 30, 40, 40, 40, 30, 40, 40], RunPhase::Running);
        assert_eq!(
            cells(&nest),
            vec![
                Cell::Done,   // travel
                Cell::Active, // outer loop
                Cell::Active, // inner loop
                Cell::Active, // gather (cursor)
                Cell::Pending // deposit
            ],
        );
        assert_eq!(nest[1].iter, Some((2, Some(2))), "outer counts inner-header fires");
        assert_eq!(nest[2].iter, Some((2, Some(3))), "inner resets to current outer pass");

        // ─── when-skip (farm-chickens shape) ───────────────────────────────────
        // 0 travel · 1 loop(start=3,N=2) · 2 when · 3 rest[guard=2] · 4 fight · 5 travel · 6 deposit
        let chickens = vec![
            action(1, 0, "travel-to", None),
            loop_row(2, 0, 3, Some(2)),
            when_row(3, 1),
            action(4, 2, "rest", Some(3)),
            action(5, 1, "fight", None),
            action(6, 0, "travel-to", None),
            action(7, 0, "deposit-all", None),
        ];
        // Two iterations, need-rest false both times, back-to-back when+fight
        // fires in one drain (log has 3,5,3,5 with nothing between).
        let skip = reduce(&chickens, &[0, 1, 2, 3, 5, 3, 5], RunPhase::Running);
        assert_eq!(
            cells(&skip),
            vec![
                Cell::Done,    // travel
                Cell::Active,  // loop
                Cell::Done,    // when (reached this iteration)
                Cell::Skipped, // rest (guard fired, body didn't)
                Cell::Active,  // fight (cursor)
                Cell::Pending, // travel to bank
                Cell::Pending, // deposit
            ],
        );
        assert_eq!(skip[1].iter, Some((2, Some(2))));

        // Same shape but need-rest TRUE this iteration → rest done, not skipped.
        let rested = reduce(&chickens, &[0, 1, 2, 3, 4, 5], RunPhase::Running);
        assert_eq!(rested[3].cell, Cell::Done, "rest ran this iteration");
        assert_eq!(rested[4].cell, Cell::Active, "fight is the cursor");

        // ─── failure marks the death row ──────────────────────────────────────
        let failed = reduce(&chickens, &[0, 1, 2, 3, 5], RunPhase::Failed);
        assert!(failed[4].death, "fight is the death step");
        assert_eq!(failed[4].cell, Cell::Done, "death step is not left active");

        // ─── unresolved count renders k/? ─────────────────────────────────────
        let unresolved = vec![action(1, 0, "travel-to", None), loop_row(2, 0, 3, None)];
        let ur = reduce(&unresolved, &[1, 2, 3, 3], RunPhase::Running);
        assert_eq!(ur[1].iter, Some((2, None)), "None denominator → k/?");
    }
}
