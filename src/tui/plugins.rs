//! The tiling layer. Each dashboard pane is a [`HypertilePlugin`] that renders
//! the matching widget and, while the runtime is in `PluginInput` mode, forwards
//! key events to the shared [`App`]. Every pane reads the same live character
//! view / run state, so they share one `App` behind `Rc<RefCell<..>>`: the
//! `ratatui-hypertile-extras` runtime owns focus and geometry (resize, swap),
//! the `App` owns everything else.
//!
//! The layout is fixed at four panes — [`build_runtime`] wires the tree once and
//! the event loop never forwards split/close keys — so users can resize and
//! rearrange the content panes but not add or remove them.

use std::cell::RefCell;
use std::rc::Rc;

use ratatui::buffer::Buffer;
use ratatui::layout::{Direction, Rect};
use ratatui_hypertile::raw::Node;
use ratatui_hypertile::{EventOutcome, HypertileEvent, KeyCode, Modifiers, PaneId};
use ratatui_hypertile_extras::{HypertilePlugin, HypertileRuntime, MoveBindings};

pub use ratatui_hypertile_extras::hypertile_event_from_crossterm;

use crate::tui::app::{App, Pane};
use crate::tui::widgets::{self, Scale};

/// The one `App`, shared into every pane plugin.
type Shared = Rc<RefCell<App>>;

/// Maps live pane ids back to the pane kind, so the footer, key routing, and the
/// zoom overlay can ask "what kind of pane holds focus?" without threading the
/// concrete plugin type around.
pub struct Panes {
    stats: PaneId,
    inventory: PaneId,
    workflows: PaneId,
    run: PaneId,
}

impl Panes {
    pub fn kind(&self, id: PaneId) -> Option<Pane> {
        match id {
            id if id == self.stats => Some(Pane::Stats),
            id if id == self.inventory => Some(Pane::Inventory),
            id if id == self.workflows => Some(Pane::Workflows),
            id if id == self.run => Some(Pane::Run),
            _ => None,
        }
    }

    pub fn id(&self, pane: Pane) -> PaneId {
        match pane {
            Pane::Stats => self.stats,
            Pane::Inventory => self.inventory,
            Pane::Workflows => self.workflows,
            Pane::Run => self.run,
        }
    }
}

// ─── plugins ────────────────────────────────────────────────────────────────

struct StatsPlugin {
    app: Shared,
}
impl HypertilePlugin for StatsPlugin {
    fn render(&mut self, area: Rect, buf: &mut Buffer, is_focused: bool) {
        let app = self.app.borrow();
        let view = app.view.get();
        widgets::stats::render(buf, area, &view, is_focused, Scale::Compact);
    }
}

struct InventoryPlugin {
    app: Shared,
}
impl HypertilePlugin for InventoryPlugin {
    fn render(&mut self, area: Rect, buf: &mut Buffer, is_focused: bool) {
        let app = self.app.borrow();
        let view = app.view.get();
        widgets::inventory::render(buf, area, &app, &view, is_focused, Scale::Compact);
    }

    fn on_event(&mut self, event: &HypertileEvent) -> EventOutcome {
        let HypertileEvent::Key(chord) = event else {
            return EventOutcome::Ignored;
        };
        let mut app = self.app.borrow_mut();
        match chord.code {
            KeyCode::Up => app.scroll_inventory(-1),
            KeyCode::Down => app.scroll_inventory(1),
            _ => return EventOutcome::Ignored,
        }
        EventOutcome::Consumed
    }
}

struct WorkflowsPlugin {
    app: Shared,
}
impl HypertilePlugin for WorkflowsPlugin {
    fn render(&mut self, area: Rect, buf: &mut Buffer, is_focused: bool) {
        let app = self.app.borrow();
        widgets::workflows::render(buf, area, &app, is_focused, Scale::Compact);
    }

    fn on_event(&mut self, event: &HypertileEvent) -> EventOutcome {
        let HypertileEvent::Key(chord) = event else {
            return EventOutcome::Ignored;
        };
        let mut app = self.app.borrow_mut();
        match chord.code {
            KeyCode::Up => app.select(-1),
            KeyCode::Down => app.select(1),
            KeyCode::Char('t') => app.tooltip = !app.tooltip,
            KeyCode::Char('p') => app.force_refresh_plan(),
            // Capital R (or Shift+r) overrides an infeasible plan; plain r runs.
            KeyCode::Char('R') => app.launch_run(true),
            KeyCode::Char('r') if chord.modifiers.contains(Modifiers::SHIFT) => {
                app.launch_run(true)
            }
            KeyCode::Char('r') => app.launch_run(false),
            _ => return EventOutcome::Ignored,
        }
        EventOutcome::Consumed
    }
}

struct RunPlugin {
    app: Shared,
}
impl HypertilePlugin for RunPlugin {
    fn render(&mut self, area: Rect, buf: &mut Buffer, is_focused: bool) {
        let app = self.app.borrow();
        widgets::run::render(buf, area, &app, is_focused, Scale::Compact);
    }

    fn on_event(&mut self, event: &HypertileEvent) -> EventOutcome {
        let HypertileEvent::Key(chord) = event else {
            return EventOutcome::Ignored;
        };
        if chord.code == KeyCode::Char('x') {
            self.app.borrow_mut().stop_run();
            return EventOutcome::Consumed;
        }
        EventOutcome::Ignored
    }
}

// ─── construction ───────────────────────────────────────────────────────────

/// Build the tiling runtime with the fixed four-pane dashboard tree. The right
/// column (workflows over run) gets the larger share; the left column (stats
/// over inventory) is narrower. Panes carry named plugins so `Enter`/`i` opens
/// them for interaction rather than the runtime's plugin palette.
pub fn build_runtime(app: Shared) -> (HypertileRuntime, Panes) {
    // `ShiftArrows` move-bindings: plain arrows move focus, Shift+Arrows
    // rearrange the focused pane — and vim (h/j/k/l, H/J/K/L) is left unbound.
    let mut rt = HypertileRuntime::builder()
        .with_move_bindings(MoveBindings::ShiftArrows)
        .with_focus_highlight(true)
        .build();

    let a = app.clone();
    rt.register_plugin_type("stats", move || StatsPlugin { app: a.clone() });
    let a = app.clone();
    rt.register_plugin_type("inventory", move || InventoryPlugin { app: a.clone() });
    let a = app.clone();
    rt.register_plugin_type("workflows", move || WorkflowsPlugin { app: a.clone() });
    let a = app.clone();
    rt.register_plugin_type("run", move || RunPlugin { app: a.clone() });

    let panes = Panes {
        stats: PaneId::ROOT,
        inventory: PaneId::new(1),
        workflows: PaneId::new(2),
        run: PaneId::new(3),
    };

    let tree = Node::Split {
        direction: Direction::Horizontal,
        ratio: 0.34, // left column narrower; give the right side more room
        first: Box::new(Node::Split {
            direction: Direction::Vertical,
            ratio: 0.5,
            first: Box::new(Node::Pane(panes.stats)),
            second: Box::new(Node::Pane(panes.inventory)),
        }),
        second: Box::new(Node::Split {
            direction: Direction::Vertical,
            ratio: 0.62, // workflows (with the plan summary) over the run panel
            first: Box::new(Node::Pane(panes.workflows)),
            second: Box::new(Node::Pane(panes.run)),
        }),
    };
    rt.set_root(tree).expect("valid initial layout tree");
    rt.replace_pane_plugin(panes.stats, "stats")
        .expect("mount stats");
    rt.replace_pane_plugin(panes.inventory, "inventory")
        .expect("mount inventory");
    rt.replace_pane_plugin(panes.workflows, "workflows")
        .expect("mount workflows");
    rt.replace_pane_plugin(panes.run, "run").expect("mount run");
    rt.focus_pane(panes.workflows).expect("focus workflows");

    (rt, panes)
}
