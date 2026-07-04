//! Header pane: name, level, xp bar, gold, live cooldown bar (§4.4). The
//! cooldown bar is derived from `cooldown_expiration` vs the wall clock (§3.8),
//! ticking every frame with no scheduler involvement.

use artifacts_core::step::CharacterView;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::tui::glyphs;
use crate::tui::theme;

use super::{cooldown_remaining, fmt_commas, fmt_k, text_bar};

pub fn render(f: &mut Frame, area: Rect, v: &CharacterView) {
    let block = Block::default().borders(Borders::ALL);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let xp_ratio = if v.max_xp > 0 {
        v.xp as f64 / v.max_xp as f64
    } else {
        0.0
    };
    let cd = cooldown_remaining(&v.cooldown_expiration);
    // The cooldown bar fills relative to a nominal window; clamp so a long
    // cooldown still reads as "full then draining".
    let cd_ratio = (cd / 30.0).clamp(0.0, 1.0);

    let mut spans = vec![
        Span::from(v.name.to_string()).fg(theme::ACCENT).bold(),
        Span::raw("  "),
        Span::from(format!("Lv{}", v.level)).bold(),
        Span::raw("   xp "),
        Span::from(text_bar(xp_ratio, 7)).fg(theme::XP),
        Span::raw(format!(" {}/{}", fmt_k(v.xp), fmt_k(v.max_xp))),
        Span::raw("    "),
        Span::from(glyphs::GOLD).fg(theme::GOLD),
        Span::from(format!(" {}", fmt_commas(v.gold))).fg(theme::GOLD),
        Span::raw("    cd "),
    ];
    if cd > 0.0 {
        spans.push(Span::from(text_bar(cd_ratio, 5)).fg(theme::COOLDOWN));
        spans.push(Span::raw(format!(" {cd:.1}s")));
    } else {
        spans.push(Span::from("ready").fg(theme::OK));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), inner);
}
