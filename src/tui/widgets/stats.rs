//! Stats pane. Compact: a vertical list of the load-bearing numbers (hp,
//! position, primary atk/def, crit, haste) so a tall-narrow tile reads with
//! little whitespace. Modal adds the full per-element attack/resist block +
//! initiative (§4.4). One widget, two scales (§4.2).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use artifacts_core::combat::CombatStats;
use artifacts_core::step::CharacterView;

use crate::tui::glyphs;
use crate::tui::theme;

use super::{pane_block, Scale};

pub fn render(buf: &mut Buffer, area: Rect, v: &CharacterView, focused: bool, scale: Scale) {
    let block = pane_block("STATS", focused);
    let inner = block.inner(area);
    block.render(area, buf);

    // Derive combat stats once; both scales read the same values.
    let cs = CombatStats::from(v);
    let lines = match scale {
        Scale::Compact => compact(v, &cs),
        Scale::Modal => modal(v, &cs),
    };
    Paragraph::new(lines).render(inner, buf);
}

/// One `<glyph> <label>  <value>` row, glyph colored, label dimmed.
fn row(
    glyph: &'static str,
    glyph_color: ratatui::style::Color,
    label: &str,
    value: String,
) -> Line<'static> {
    Line::from(vec![
        Span::from(glyph).fg(glyph_color),
        Span::raw(" "),
        Span::from(format!("{label:<5}")).fg(theme::DIM),
        Span::raw(" "),
        Span::raw(value),
    ])
}

fn compact(v: &CharacterView, cs: &CombatStats) -> Vec<Line<'static>> {
    let atk: i32 = cs.attack.iter().sum();
    let def: i32 = cs.res.iter().sum();
    vec![
        row(
            glyphs::HP,
            theme::BAD,
            "hp",
            format!("{}/{}", v.hp, v.max_hp),
        ),
        row(glyphs::ATK, theme::WARN, "atk", atk.to_string()),
        row(glyphs::DEF, theme::ACCENT, "def", def.to_string()),
        row(glyphs::POS, theme::DIM, "pos", format!("({},{})", v.x, v.y)),
        row(" ", theme::DIM, "crit", format!("{}%", v.critical_strike)),
        row(" ", theme::DIM, "hst", v.haste.to_string()),
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
