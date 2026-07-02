//! Run pane: the flat skeleton with per-row glyph states driven by the pure
//! reducer over the live id-log (§3.3, §4.4). Compact renders glyphs; modal adds
//! the state word (§3.3: "words only in the zoomed detail view").

use ratatui::layout::Rect;
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::tui::app::App;
use crate::tui::glyphs;
use crate::tui::reducer::{reduce, Cell, RowState};
use crate::tui::skeleton::{PlanStep, StepKind};
use crate::tui::theme;

use super::{pane_block, Scale};

pub fn render(f: &mut Frame, area: Rect, app: &App, focused: bool, scale: Scale) {
    let block = pane_block("RUN", focused);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(session) = &app.session else {
        f.render_widget(
            Paragraph::new(Span::from("select a workflow and press r").fg(theme::DIM)),
            inner,
        );
        return;
    };
    let Some(skeleton) = session.skeleton.get() else {
        f.render_widget(
            Paragraph::new(Span::from("preparing run…").fg(theme::ACCENT)),
            inner,
        );
        return;
    };

    // Reduce under the lock — `reduce` is pure CPU (microseconds) and the log
    // only grows, so cloning the whole (possibly thousands-long) Vec each frame
    // would be pure waste. `phase()` is `Copy`, so the status guard drops at once.
    let phase = session.status.lock().unwrap().phase();
    let rows = {
        let log = session.progress.lock().unwrap();
        reduce(skeleton, &log, phase)
    };

    let visible = inner.height as usize;
    // Anchor the viewport so the row the user cares about stays in view:
    //   1) the Active row while running (unchanged behavior);
    //   2) else the death row on a failed run (keep the fatal step visible);
    //   3) else the last "reached" row on a done run — the highest index whose
    //      cell isn't Pending — so the closing travel/deposit/result isn't clipped;
    //   4) else the top.
    let anchor = rows
        .iter()
        .position(|r| r.cell == Cell::Active)
        .or_else(|| rows.iter().position(|r| r.death))
        .or_else(|| rows.iter().rposition(|r| r.cell != Cell::Pending))
        .unwrap_or(0);
    let start = anchor.saturating_sub(visible.saturating_sub(1));

    let lines = build_lines(skeleton, &rows, app.spinner, scale, start, visible);
    f.render_widget(Paragraph::new(lines), inner);
}

/// Build the run-panel lines for a `(skeleton, reduced rows)` pair — the pure
/// core of rendering, extracted so a `TestBackend` snapshot can exercise it
/// without a live `App` (§7 Tier 3).
pub fn build_lines(
    skeleton: &[PlanStep],
    rows: &[RowState],
    spinner: usize,
    scale: Scale,
    start: usize,
    visible: usize,
) -> Vec<Line<'static>> {
    skeleton
        .iter()
        .zip(rows.iter())
        .skip(start)
        .take(visible)
        .map(|(step, state)| row_line(step, state, spinner, scale))
        .collect()
}

fn row_line(step: &PlanStep, state: &RowState, frame: usize, scale: Scale) -> Line<'static> {
    let indent = "  ".repeat(step.depth as usize + 1);
    let label = step.label_text();

    let (glyph, color) = match step.kind {
        StepKind::Loop => (glyphs::LOOP.to_string(), theme::ACCENT),
        StepKind::When => (glyphs::WHEN.to_string(), theme::ACCENT),
        StepKind::Action => (
            glyphs::cell(state.cell, frame).to_string(),
            cell_color(state),
        ),
    };

    let mut spans = vec![
        Span::raw(indent),
        Span::from(glyph).fg(color),
        Span::raw(" "),
    ];

    // Loop rows append the k/N counter; when/action rows just show the label.
    if step.kind == StepKind::Loop {
        spans.push(Span::from(label).fg(cell_color(state)));
        if let Some(iter) = state.iter {
            spans.push(Span::raw(" "));
            spans.push(Span::from(iter_text(iter)).fg(theme::ACCENT).bold());
        }
    } else {
        let style = if state.death {
            Style::default().fg(theme::BAD).bold()
        } else {
            Style::default().fg(cell_color(state))
        };
        spans.push(Span::styled(label, style));
    }

    if scale == Scale::Modal {
        spans.push(Span::from(format!("   [{}]", state_word(state))).fg(theme::DIM));
    }
    Line::from(spans)
}

fn cell_color(state: &RowState) -> ratatui::style::Color {
    if state.death {
        return theme::BAD;
    }
    match state.cell {
        Cell::Done => theme::OK,
        Cell::Active => theme::ACCENT,
        Cell::Pending => theme::DIM,
        Cell::Skipped => theme::SKIP,
    }
}

/// `k/N`, `>N` on divergence, `k/?` when the plan never resolved the count.
fn iter_text(iter: (u32, Option<u32>)) -> String {
    match iter {
        (k, Some(n)) if k <= n => format!("{k}/{n}"),
        (_, Some(n)) => format!(">{n}"),
        (k, None) => format!("{k}/?"),
    }
}

fn state_word(state: &RowState) -> &'static str {
    if state.death {
        return "died";
    }
    match state.cell {
        Cell::Done => "done",
        Cell::Active => "active",
        Cell::Pending => "pending",
        Cell::Skipped => "skipped",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::reducer::RunPhase;
    use ratatui::backend::TestBackend;
    use ratatui::widgets::Paragraph;
    use ratatui::Terminal;

    #[allow(clippy::too_many_arguments)]
    fn step(
        id: u64,
        depth: u16,
        kind: StepKind,
        op: &str,
        args: &[&str],
        label: Option<&str>,
        count: Option<u32>,
        loop_start: Option<u64>,
        guard: Option<u64>,
    ) -> PlanStep {
        PlanStep {
            id,
            depth,
            kind,
            op: op.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            label: label.map(str::to_string),
            count,
            loop_start_id: loop_start,
            guard_id: guard,
        }
    }

    fn buffer_text(term: &Terminal<TestBackend>) -> Vec<String> {
        let buf = term.backend().buffer();
        let area = buf.area;
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    /// Tier 3 (§7): snapshot the run panel for a known reduced-row vector — the
    /// farm-chickens shape with the `rest` step skipped — to catch label/layout
    /// regressions without a terminal. `Done` phase keeps glyphs deterministic
    /// (no animated spinner).
    #[test]
    fn run_panel_snapshot() {
        let skeleton = vec![
            step(
                1,
                0,
                StepKind::Action,
                "travel-to",
                &["1", "1"],
                None,
                None,
                None,
                None,
            ),
            step(
                2,
                0,
                StepKind::Loop,
                "repeat-until",
                &[],
                Some("fights"),
                Some(2),
                Some(3),
                None,
            ),
            step(3, 1, StepKind::When, "when", &[], None, None, None, None),
            step(
                4,
                2,
                StepKind::Action,
                "rest",
                &[],
                None,
                None,
                None,
                Some(3),
            ),
            step(5, 1, StepKind::Action, "fight", &[], None, None, None, None),
            step(
                6,
                0,
                StepKind::Action,
                "travel-to",
                &["4", "1"],
                None,
                None,
                None,
                None,
            ),
            step(
                7,
                0,
                StepKind::Action,
                "deposit-all",
                &[],
                None,
                None,
                None,
                None,
            ),
        ];
        // Two iterations, rest skipped both times, then bank.
        let log = vec![0u64, 1, 2, 3, 5, 3, 5, 6, 7];
        let rows = reduce(&skeleton, &log, RunPhase::Done);
        let lines = build_lines(&skeleton, &rows, 0, Scale::Compact, 0, 10);

        let backend = TestBackend::new(40, 7);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| f.render_widget(Paragraph::new(lines), f.area()))
            .unwrap();
        let text = buffer_text(&terminal);

        assert!(
            text.iter()
                .any(|l| l.contains(glyphs::DONE) && l.contains("travel (1,1)")),
            "travel row rendered done: {text:?}"
        );
        assert!(
            text.iter()
                .any(|l| l.contains(glyphs::LOOP) && l.contains("fights") && l.contains("2/2")),
            "loop header shows label + k/N: {text:?}"
        );
        assert!(
            text.iter()
                .any(|l| l.contains(glyphs::SKIPPED) && l.contains("rest")),
            "rest rendered skipped: {text:?}"
        );
        assert!(
            text.iter()
                .any(|l| l.contains(glyphs::DONE) && l.contains("fight")),
            "fight rendered done: {text:?}"
        );
        assert!(
            text.iter().any(|l| l.contains("deposit-all")),
            "deposit-all present: {text:?}"
        );
        // Indentation reflects depth: the rest row (depth 2) is deeper-indented
        // than the fight row (depth 1).
        let rest = text.iter().find(|l| l.contains("rest")).unwrap();
        let fight = text.iter().find(|l| l.contains("fight")).unwrap();
        let indent = |l: &str| l.len() - l.trim_start().len();
        assert!(indent(rest) > indent(fight), "rest indented under its when");
    }
}
