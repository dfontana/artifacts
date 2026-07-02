//! Inventory pane: occupied slots (code + quantity) with a `used/max` header.
//! Compact clips to the pane; modal scrolls the full list (§4.4). Scroll offset
//! lives in the App (Interact `↑/↓`).

use artifacts_core::step::CharacterView;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::tui::app::App;
use crate::tui::theme;

use super::{pane_block, Scale};

pub fn render(
    f: &mut Frame,
    area: Rect,
    app: &App,
    v: &CharacterView,
    focused: bool,
    _scale: Scale,
) {
    // Borrow the occupied slots (shared with `inventory_slots_used`) — `&str`
    // pairs, so no per-slot `String` is allocated on this per-frame path.
    let occupied: Vec<(&str, u32)> = v.occupied_items().collect();

    let title = format!(
        "INVENTORY   {}/{}",
        v.inventory_count(),
        v.inventory_max_items
    );
    let block = pane_block(&title, focused);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if occupied.is_empty() {
        f.render_widget(Paragraph::new(Span::from("empty").fg(theme::DIM)), inner);
        return;
    }

    let rows = inner.height as usize;
    let start = app.inventory_scroll.min(occupied.len().saturating_sub(1));
    let lines: Vec<Line> = occupied
        .iter()
        .skip(start)
        .take(rows)
        .map(|(code, qty)| {
            Line::from(vec![
                Span::raw(format!("  {code:<14}")),
                Span::from(format!("x{qty}")).fg(theme::ACCENT),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}
