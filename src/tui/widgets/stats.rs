//! Stats pane. Compact: hp, position, primary atk/def, crit, haste. Modal: the
//! full per-element attack/resist block + initiative (§4.4). One widget, two
//! scales (§4.2).

use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use artifacts_core::combat::CombatStats;
use artifacts_core::step::CharacterView;

use crate::tui::glyphs;
use crate::tui::theme;

use super::{pane_block, Scale};

pub fn render(f: &mut Frame, area: Rect, v: &CharacterView, focused: bool, scale: Scale) {
    let block = pane_block("STATS", focused);
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Derive combat stats once; both scales read the same values.
    let cs = CombatStats::from(v);
    let lines = match scale {
        Scale::Compact => compact(v, &cs),
        Scale::Modal => modal(v, &cs),
    };
    f.render_widget(Paragraph::new(lines), inner);
}

fn compact(v: &CharacterView, cs: &CombatStats) -> Vec<Line<'static>> {
    let atk: i32 = cs.attack.iter().sum();
    let def: i32 = cs.res.iter().sum();
    vec![
        Line::from(vec![
            Span::from(glyphs::HP).fg(theme::BAD),
            Span::raw(format!(" {}/{}   ", v.hp, v.max_hp)),
            Span::from(glyphs::ATK).fg(theme::WARN),
            Span::raw(format!(" {atk}   ")),
            Span::from(glyphs::DEF).fg(theme::ACCENT),
            Span::raw(format!(" {def}")),
        ]),
        Line::from(vec![
            Span::from(glyphs::POS).fg(theme::DIM),
            Span::raw(format!(" ({},{})   ", v.x, v.y)),
            Span::raw(format!("crit {}%  hst {}", v.critical_strike, v.haste)),
        ]),
    ]
}

fn modal(v: &CharacterView, cs: &CombatStats) -> Vec<Line<'static>> {
    let elems = ["fire", "earth", "water", "air"];
    let mut lines = compact(v, cs);
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::from("per-element").fg(theme::DIM)));
    lines.push(Line::raw("       atk   dmg%   res"));
    for (i, e) in elems.iter().enumerate() {
        lines.push(Line::raw(format!(
            "{e:<6} {:>3}   {:>3}   {:>3}",
            cs.attack[i], cs.dmg[i], cs.res[i]
        )));
    }
    lines.push(Line::raw(""));
    lines.push(Line::raw(format!(
        "initiative {}   global dmg {}%",
        cs.initiative, cs.global_dmg
    )));
    lines
}
