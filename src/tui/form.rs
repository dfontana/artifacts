//! The parameter form (`plans/DYNAMIC_WORKFLOWS.md` §8-M6): the modal that opens
//! when a parameterized workflow is launched from the TUI. Pure state — no Lua,
//! no I/O — testable exactly like the reducer. The event layer drives it through
//! the small method surface here; `widgets::form` renders it; `App::submit_form`
//! turns a clean submit into a run launch.
//!
//! Division of labor (design §2.3): the form validates **shape** only — required
//! present, numbers parse, enum membership — the same checks
//! `fennel/lib/params.fnl` enforces at load, so a blocked submit and a coercion
//! error can never disagree. Semantic validity ("no such monster") stays with
//! the loud host lookups during build/plan. Completion likewise *assists*: free
//! text is always allowed, the candidate lists never restrict.

use std::collections::BTreeSet;

use crate::data::{MonsterData, NpcItemData, RecipeData, ResourceData};
use crate::workflow::{ParamSpec, ParamType, WorkflowInfo};

/// How many prefix-filtered suggestions show under the focused field.
const MAX_SUGGESTIONS: usize = 5;

/// The fixed eight skills (design §2.2's `:skill` completion source), sorted.
const SKILLS: [&str; 8] = [
    "alchemy",
    "cooking",
    "fishing",
    "gearcrafting",
    "jewelrycrafting",
    "mining",
    "weaponcrafting",
    "woodcutting",
];

/// The reference datasets completion candidates are drawn from. Each is
/// optional — a dataset that wasn't loaded just yields no suggestions for the
/// param types it backs (free text still works; completion only assists).
pub struct CompletionSources<'a> {
    pub resources: Option<&'a ResourceData>,
    pub monsters: Option<&'a MonsterData>,
    pub recipes: Option<&'a RecipeData>,
    pub npc_items: Option<&'a NpcItemData>,
}

/// One form field: a declared param, its current text value, its inline shape
/// error, and the full completion candidate list (sorted, deduped) built ONCE
/// when the form opens.
pub struct FormField {
    pub spec: ParamSpec,
    pub value: String,
    pub error: Option<String>,
    candidates: Vec<String>,
}

impl FormField {
    /// The prefix-filtered top suggestions for the current text (the first is
    /// the Tab-accept target). An exact match is not re-suggested.
    pub fn suggestions(&self) -> Vec<&str> {
        self.candidates
            .iter()
            .filter(|c| c.starts_with(&self.value) && c.as_str() != self.value)
            .take(MAX_SUGGESTIONS)
            .map(String::as_str)
            .collect()
    }

    fn validate(&mut self) {
        self.error = validate_value(&self.spec, &self.value);
    }
}

/// The open param form: one field per declared param, a focus cursor, and the
/// workflow it will launch. Lives as `Option<ParamForm>` on `App` (a transient
/// modal, exactly like the command palette's `Option<Palette>`).
pub struct ParamForm {
    /// The workflow this form launches on submit (also the modal title).
    pub workflow: String,
    /// The workflow's one-line `:doc`, shown under the title.
    pub doc: Option<String>,
    pub fields: Vec<FormField>,
    pub focus: usize,
}

impl ParamForm {
    /// Build the form for a workflow's declared params: required fields first,
    /// then optional (name order within each group — the marshalled schema is
    /// already name-sorted, and the partition is stable). Values prefill from
    /// declared defaults; completion candidates are built once per field.
    pub fn new(workflow: String, info: &WorkflowInfo, sources: &CompletionSources) -> Self {
        let fields = ordered_params(info)
            .into_iter()
            .map(|spec| FormField {
                value: spec.default.clone().unwrap_or_default(),
                error: None,
                candidates: candidates(spec, sources),
                spec: spec.clone(),
            })
            .collect();
        Self {
            workflow,
            doc: info.doc.clone(),
            fields,
            focus: 0,
        }
    }

    /// Move focus to the next field, wrapping (Down).
    pub fn focus_next(&mut self) {
        self.focus_by(1);
    }

    /// Move focus to the previous field, wrapping (Up).
    pub fn focus_prev(&mut self) {
        self.focus_by(-1);
    }

    fn focus_by(&mut self, delta: isize) {
        if self.fields.is_empty() {
            return;
        }
        let n = self.fields.len() as isize;
        self.focus = (self.focus as isize + delta).rem_euclid(n) as usize;
    }

    /// Type into the focused field, revalidating it. `:bool` fields take no
    /// free text: Space toggles, `t`/`f` set — the value is always the string
    /// "true"/"false" (params travel as strings end to end).
    pub fn input(&mut self, c: char) {
        let Some(f) = self.fields.get_mut(self.focus) else {
            return;
        };
        if f.spec.ptype == ParamType::Bool {
            match c {
                ' ' => {
                    f.value = if f.value == "true" { "false" } else { "true" }.into();
                }
                't' => f.value = "true".into(),
                'f' => f.value = "false".into(),
                _ => return,
            }
        } else {
            f.value.push(c);
        }
        f.validate();
    }

    /// Delete from the focused field: one char, or the whole value for a
    /// `:bool` (whose value is only ever set whole).
    pub fn backspace(&mut self) {
        let Some(f) = self.fields.get_mut(self.focus) else {
            return;
        };
        if f.spec.ptype == ParamType::Bool {
            f.value.clear();
        } else {
            f.value.pop();
        }
        f.validate();
    }

    /// Accept the focused field's top suggestion (Tab). A no-op when nothing
    /// matches the typed prefix.
    pub fn accept_suggestion(&mut self) {
        let Some(f) = self.fields.get_mut(self.focus) else {
            return;
        };
        if let Some(top) = f.suggestions().first().map(|s| s.to_string()) {
            f.value = top;
            f.validate();
        }
    }

    /// True while any field carries a validation error — submit is blocked.
    pub fn blocked(&self) -> bool {
        self.fields.iter().any(|f| f.error.is_some())
    }

    /// Validate every field and, when clean, yield the `k=v` string pairs to
    /// launch with. Only non-empty fields travel: an empty optional is simply
    /// not passed, so the Fennel-side coercion applies its declared default.
    /// `None` = blocked; the fields' inline errors say why.
    pub fn submit(&mut self) -> Option<Vec<(String, String)>> {
        for f in &mut self.fields {
            f.validate();
        }
        if self.blocked() {
            return None;
        }
        Some(
            self.fields
                .iter()
                .filter(|f| !f.value.is_empty())
                .map(|f| (f.spec.name.clone(), f.value.clone()))
                .collect(),
        )
    }
}

/// The form's field order: required params first, then optional, name order
/// within each group (`workflow::schema` already name-sorts; the stable
/// partition preserves that). The workflow list hint uses the same order.
pub fn ordered_params(info: &WorkflowInfo) -> Vec<&ParamSpec> {
    let mut specs: Vec<&ParamSpec> = info.params.iter().collect();
    specs.sort_by_key(|p| !p.required);
    specs
}

/// The workflow list's compact param hint: `(target, qty?)` — required params
/// bare, optional marked with `?`. `None` for a parameterless workflow.
pub fn param_hint(info: &WorkflowInfo) -> Option<String> {
    if info.params.is_empty() {
        return None;
    }
    let parts: Vec<String> = ordered_params(info)
        .into_iter()
        .map(|p| {
            if p.required {
                p.name.clone()
            } else {
                format!("{}?", p.name)
            }
        })
        .collect();
    Some(format!("({})", parts.join(", ")))
}

/// Shape-only validation, mirroring what `fennel/lib/params.fnl` enforces at
/// load (design §2.3): required params must be present, `:number` must parse,
/// `:enum` must be one of its options (`:bool` can only be "true"/"false", an
/// invariant `input` already maintains). Game-code semantic validity is
/// deliberately NOT checked here — that is the loud host lookups' job during
/// build/plan; a second registry of what codes exist would drift.
fn validate_value(spec: &ParamSpec, value: &str) -> Option<String> {
    if value.is_empty() {
        return spec.required.then(|| "required".to_string());
    }
    match spec.ptype {
        ParamType::Number if value.parse::<f64>().is_err() => Some("not a number".into()),
        ParamType::Bool if value != "true" && value != "false" => Some("not true/false".into()),
        ParamType::Enum if !spec.options.iter().any(|o| o == value) => {
            Some(format!("not one of [{}]", spec.options.join(", ")))
        }
        _ => None,
    }
}

/// The completion candidate list for one param — design §2.2's type→completion
/// table — sorted and deduped (`BTreeSet`), built once at form open. `:item` is
/// the union of recipe output codes, the NPC catalog's item codes, and
/// monster + resource drop codes.
fn candidates(spec: &ParamSpec, src: &CompletionSources) -> Vec<String> {
    let mut set: BTreeSet<String> = BTreeSet::new();
    match spec.ptype {
        // Free text / toggle — nothing to complete.
        ParamType::String | ParamType::Number | ParamType::Bool => {}
        ParamType::Enum => set.extend(spec.options.iter().cloned()),
        ParamType::Skill => set.extend(SKILLS.iter().map(|s| s.to_string())),
        ParamType::Resource => {
            if let Some(d) = src.resources {
                set.extend(d.iter().map(|(c, _)| c.as_str().to_string()));
            }
        }
        ParamType::Monster => {
            if let Some(d) = src.monsters {
                set.extend(d.iter().map(|(c, _)| c.as_str().to_string()));
            }
        }
        ParamType::Npc => {
            if let Some(d) = src.npc_items {
                set.extend(
                    d.iter()
                        .flat_map(|(_, listings)| listings.iter())
                        .map(|l| l.npc.as_str().to_string()),
                );
            }
        }
        ParamType::Item => {
            if let Some(d) = src.recipes {
                set.extend(d.iter().map(|(c, _)| c.as_str().to_string()));
            }
            if let Some(d) = src.npc_items {
                set.extend(d.iter().map(|(c, _)| c.as_str().to_string()));
            }
            if let Some(d) = src.monsters {
                set.extend(
                    d.iter()
                        .flat_map(|(_, m)| m.drops.iter())
                        .map(|drop| drop.code.as_str().to_string()),
                );
            }
            if let Some(d) = src.resources {
                set.extend(
                    d.iter()
                        .flat_map(|(_, r)| r.drops.iter())
                        .map(|drop| drop.code.as_str().to_string()),
                );
            }
        }
    }
    set.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use artifacts_core::combat::MonsterView;
    use artifacts_core::drop::DropRate;
    use artifacts_core::map::ResourceView;
    use artifacts_core::npc::NpcItemView;
    use artifacts_core::recipe::{RecipeCraft, RecipeView};

    fn spec(
        name: &str,
        ptype: ParamType,
        required: bool,
        default: Option<&str>,
        options: &[&str],
    ) -> ParamSpec {
        ParamSpec {
            name: name.into(),
            ptype,
            required,
            default: default.map(Into::into),
            doc: None,
            options: options.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn drop_of(code: &str) -> DropRate {
        DropRate {
            code: code.into(),
            rate: 1,
            min_quantity: 1,
            max_quantity: 1,
        }
    }

    fn resource(code: &str, drop: &str) -> ResourceView {
        ResourceView {
            code: code.into(),
            level: 1,
            skill: "mining".into(),
            drops: vec![drop_of(drop)],
        }
    }

    fn monster(code: &str, drop: &str) -> MonsterView {
        MonsterView {
            code: code.into(),
            name: code.into(),
            level: 1,
            hp: 10,
            attack_fire: 0,
            attack_earth: 0,
            attack_water: 0,
            attack_air: 0,
            res_fire: 0,
            res_earth: 0,
            res_water: 0,
            res_air: 0,
            critical_strike: 0,
            initiative: 0,
            drops: vec![drop_of(drop)],
        }
    }

    const NO_SOURCES: CompletionSources<'static> = CompletionSources {
        resources: None,
        monsters: None,
        recipes: None,
        npc_items: None,
    };

    /// One table-driven sweep over the form's tricky cases (mirroring the
    /// reducer's test shape): open/prefill/ordering, focus wrap, typing +
    /// backspace, bool toggling, number/enum/required validation blocking
    /// submit, completion filtering + acceptance, and the exact submitted
    /// pairs (empty optionals omitted).
    #[test]
    fn form_sweep() {
        // Params arrive name-sorted, as workflow::schema marshals them.
        let info = WorkflowInfo {
            doc: Some("farm things".into()),
            params: vec![
                spec("deposit", ParamType::Bool, false, None, &[]),
                spec("mode", ParamType::Enum, false, None, &["fast", "safe"]),
                spec("qty", ParamType::Number, false, Some("1"), &[]),
                spec("target", ParamType::Resource, true, None, &[]),
            ],
        };
        let resources = ResourceData::from_vec(vec![
            resource("copper_rocks", "copper_ore"),
            resource("iron_rocks", "iron_ore"),
        ]);
        let sources = CompletionSources {
            resources: Some(&resources),
            monsters: None,
            recipes: None,
            npc_items: None,
        };
        let mut form = ParamForm::new("farm".into(), &info, &sources);

        // ─── open: field order (required first, then name order), prefills ────
        let names: Vec<&str> = form.fields.iter().map(|f| f.spec.name.as_str()).collect();
        assert_eq!(names, ["target", "deposit", "mode", "qty"]);
        assert_eq!(form.fields[3].value, "1", "default prefilled");
        assert_eq!(form.fields[0].value, "", "no default → empty");
        assert!(form.fields.iter().all(|f| f.error.is_none()));

        // The list hint shares the same order; optional params marked `?`.
        assert_eq!(
            param_hint(&info).as_deref(),
            Some("(target, deposit?, mode?, qty?)")
        );

        // ─── focus cycling wraps both ways ─────────────────────────────────────
        assert_eq!(form.focus, 0);
        form.focus_prev();
        assert_eq!(form.focus, 3, "wraps backwards");
        form.focus_next();
        assert_eq!(form.focus, 0);

        // ─── required+empty blocks submit with an inline error ─────────────────
        assert!(form.submit().is_none());
        assert_eq!(form.fields[0].error.as_deref(), Some("required"));
        assert!(form.blocked());

        // ─── typing, prefix completion, acceptance, backspace ──────────────────
        for c in "cop".chars() {
            form.input(c);
        }
        assert_eq!(form.fields[0].value, "cop");
        assert!(
            form.fields[0].error.is_none(),
            "error cleared on edit (shape ok; semantic validity is downstream)"
        );
        assert_eq!(form.fields[0].suggestions(), ["copper_rocks"]);
        form.accept_suggestion();
        assert_eq!(form.fields[0].value, "copper_rocks");
        assert_eq!(
            form.fields[0].suggestions(),
            Vec::<&str>::new(),
            "an exact match is not re-suggested"
        );
        form.backspace();
        assert_eq!(form.fields[0].value, "copper_rock");
        assert_eq!(form.fields[0].suggestions(), ["copper_rocks"]);
        form.accept_suggestion();

        // ─── :bool — Space toggles, t/f set, free text ignored ─────────────────
        form.focus_next(); // deposit
        form.input(' ');
        assert_eq!(form.fields[1].value, "true");
        form.input(' ');
        assert_eq!(form.fields[1].value, "false");
        form.input('t');
        assert_eq!(form.fields[1].value, "true");
        form.input('x');
        assert_eq!(form.fields[1].value, "true", "free text ignored on :bool");

        // ─── :enum — membership validated, options are the candidates ──────────
        form.focus_next(); // mode
        form.input('z');
        assert!(form.fields[2]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("not one of")));
        assert!(form.submit().is_none(), "enum error blocks submit");
        form.backspace();
        assert!(
            form.fields[2].error.is_none(),
            "empty optional is valid again"
        );
        assert_eq!(form.fields[2].suggestions(), ["fast", "safe"]);

        // ─── :number — parse failure blocks submit ─────────────────────────────
        form.focus_next(); // qty
        form.backspace(); // "1" → ""
        form.input('a');
        assert_eq!(form.fields[3].error.as_deref(), Some("not a number"));
        assert!(form.submit().is_none());
        form.backspace();
        form.input('2');

        // ─── clean submit: field-order k=v pairs, empty optional omitted ───────
        let pairs = form.submit().expect("all fields validate");
        assert_eq!(
            pairs,
            vec![
                ("target".to_string(), "copper_rocks".to_string()),
                ("deposit".to_string(), "true".to_string()),
                ("qty".to_string(), "2".to_string()),
            ]
        );
    }

    /// The `:item` union builder deduplicates across its four legs and sorts;
    /// `:npc` completes merchant codes (not item codes); `:skill` is the fixed
    /// eight; a missing dataset just contributes nothing.
    #[test]
    fn completion_candidates_union_dedup_sorted() {
        // copper_dagger is BOTH a recipe output and an npc listing (dedup);
        // feather comes from a monster drop, copper_ore from a resource drop.
        let recipes = RecipeData::from_items(vec![RecipeView {
            code: "copper_dagger".into(),
            craft: Some(RecipeCraft {
                skill: None,
                level: None,
                items: vec![],
                quantity: 1,
            }),
        }]);
        let npcs = NpcItemData::from_vec(vec![
            NpcItemView {
                code: "copper_dagger".into(),
                npc: "smith".into(),
                currency: "gold".into(),
                buy_price: Some(5),
                sell_price: None,
            },
            NpcItemView {
                code: "health_potion".into(),
                npc: "alchemist".into(),
                currency: "gold".into(),
                buy_price: Some(2),
                sell_price: None,
            },
        ]);
        let monsters = MonsterData::from_vec(vec![monster("chicken", "feather")]);
        let resources = ResourceData::from_vec(vec![resource("copper_rocks", "copper_ore")]);
        let sources = CompletionSources {
            resources: Some(&resources),
            monsters: Some(&monsters),
            recipes: Some(&recipes),
            npc_items: Some(&npcs),
        };

        let item_info = WorkflowInfo {
            doc: None,
            params: vec![spec("item", ParamType::Item, true, None, &[])],
        };
        let form = ParamForm::new("craft".into(), &item_info, &sources);
        assert_eq!(
            form.fields[0].candidates,
            ["copper_dagger", "copper_ore", "feather", "health_potion"],
            "union of recipe outputs ∪ npc catalog ∪ drop codes, deduped + sorted"
        );

        let npc_info = WorkflowInfo {
            doc: None,
            params: vec![spec("vendor", ParamType::Npc, true, None, &[])],
        };
        let form = ParamForm::new("buy".into(), &npc_info, &sources);
        assert_eq!(form.fields[0].candidates, ["alchemist", "smith"]);

        let skill_info = WorkflowInfo {
            doc: None,
            params: vec![spec("skill", ParamType::Skill, true, None, &[])],
        };
        let form = ParamForm::new("train".into(), &skill_info, &sources);
        assert_eq!(form.fields[0].candidates, SKILLS);

        // No datasets → no candidates, but the field still takes free text.
        let form = ParamForm::new("craft".into(), &item_info, &NO_SOURCES);
        assert!(form.fields[0].candidates.is_empty());
    }
}
