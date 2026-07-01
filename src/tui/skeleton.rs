//! `PlanStep`: the owned, `Send` skeleton row the Lua run worker marshals from
//! the Fennel `skeleton` walk and hands to the UI thread (`plans/TUI.md` §3.2).
//! No `mlua` types leak out, so it crosses the thread boundary cleanly.

use std::collections::HashMap;

use mlua::prelude::*;

use crate::tui::NodeId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    /// A leaf action (`travel-to`, `gather`, `fight`, …) → one row.
    Action,
    /// A `repeat-until` / `repeat-n` header → one row, its body once at depth+1.
    Loop,
    /// A `when` guard header → one row; its body carries this row's id as guard.
    When,
}

/// One row of the flat run-panel skeleton. Loops appear once (`count` = the k/N
/// denominator); `loop_start_id` is the loop body's first node — the iteration
/// boundary the reducer counts. `guard_id` is the id of the enclosing `when`
/// (the skip key), or `None` for unguarded rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStep {
    pub id: NodeId,
    pub depth: u16,
    pub kind: StepKind,
    /// The node op: an action name, or `repeat-until`/`repeat-n`/`when`.
    pub op: String,
    /// Action arguments, stringified (e.g. `["2", "0"]` for `travel-to`).
    pub args: Vec<String>,
    /// Loop label (`:fights`) or when-predicate name, when one is given.
    pub label: Option<String>,
    /// Loop rows only: the k/N denominator. `repeat-n` fills it from the walk;
    /// `repeat-until` is `None` until joined from the plan's resolved counts.
    pub count: Option<u32>,
    pub loop_start_id: Option<NodeId>,
    pub guard_id: Option<NodeId>,
}

impl PlanStep {
    /// The human-readable label rendered in the run panel. Presentation lives
    /// here (the TUI layer), not in Fennel — see §3.2.
    pub fn label_text(&self) -> String {
        match self.kind {
            StepKind::Loop => match &self.label {
                Some(l) => l.clone(),
                None if self.op == "repeat-n" => "repeat".to_string(),
                None => "loop".to_string(),
            },
            StepKind::When => match &self.label {
                Some(p) => format!("when {p}?"),
                None => "when".to_string(),
            },
            StepKind::Action => self.action_label(),
        }
    }

    fn action_label(&self) -> String {
        let a = |i: usize| self.args.get(i).map(String::as_str).unwrap_or("");
        match self.op.as_str() {
            "travel-to" => format!("travel ({},{})", a(0), a(1)),
            "gather" => "gather".to_string(),
            "fight" => "fight".to_string(),
            "rest" => "rest".to_string(),
            "deposit-all" => "deposit-all".to_string(),
            "deposit-item" => format!("deposit {}", a(0)),
            "craft" => format!("craft {}", a(0)),
            _ if self.args.is_empty() => self.op.clone(),
            _ => format!("{} {}", self.op, self.args.join(" ")),
        }
    }
}

/// Marshal the Fennel `skeleton` result (an array of row tables) into owned
/// `PlanStep`s. Called on the run-worker thread before the blocking run (§3.1).
pub fn marshal(rows: &LuaTable) -> LuaResult<Vec<PlanStep>> {
    let mut out = Vec::with_capacity(rows.raw_len());
    for row in rows.sequence_values::<LuaTable>() {
        out.push(from_lua_row(&row?)?);
    }
    Ok(out)
}

fn from_lua_row(row: &LuaTable) -> LuaResult<PlanStep> {
    let kind = match row.get::<String>("kind")?.as_str() {
        "action" => StepKind::Action,
        "loop" => StepKind::Loop,
        "when" => StepKind::When,
        other => {
            return Err(LuaError::RuntimeError(format!(
                "unknown skeleton kind: {other}"
            )))
        }
    };
    Ok(PlanStep {
        id: row.get("id")?,
        depth: row.get("depth")?,
        kind,
        op: row.get("op")?,
        args: read_args(row)?,
        label: row.get::<Option<String>>("label")?,
        count: row.get::<Option<u32>>("count")?,
        loop_start_id: row.get::<Option<NodeId>>("loop-start-id")?,
        guard_id: row.get::<Option<NodeId>>("guard-id")?,
    })
}

fn read_args(row: &LuaTable) -> LuaResult<Vec<String>> {
    let Some(args) = row.get::<Option<LuaTable>>("args")? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(args.raw_len());
    for v in args.sequence_values::<LuaValue>() {
        out.push(arg_to_string(v?));
    }
    Ok(out)
}

/// Stringify one AST argument. Integers/floats render without a `.0` tail so a
/// `travel-to [2 0]` reads as `travel (2,0)`, not `(2.0,0.0)`.
fn arg_to_string(v: LuaValue) -> String {
    match v {
        LuaValue::Integer(i) => i.to_string(),
        LuaValue::Number(n) if n.fract() == 0.0 => (n as i64).to_string(),
        LuaValue::Number(n) => n.to_string(),
        LuaValue::String(s) => s.to_string_lossy(),
        LuaValue::Boolean(b) => b.to_string(),
        other => format!("{other:?}"),
    }
}

/// Read the plan's id-keyed loop counts (`loop-counts[id] = {label, count}`)
/// into `id → count`, so `repeat-until` denominators can be joined onto the
/// skeleton by node identity — never by label, which could collide (§3.2).
pub fn read_loop_counts(plan_result: &LuaTable) -> LuaResult<HashMap<NodeId, u32>> {
    let mut m = HashMap::new();
    if let Some(lc) = plan_result.get::<Option<LuaTable>>("loop-counts")? {
        for pair in lc.pairs::<NodeId, LuaTable>() {
            let (id, entry) = pair?;
            m.insert(id, entry.get::<u32>("count")?);
        }
    }
    Ok(m)
}

/// Fill each loop row's `count` from the plan's resolved counts. `repeat-n` rows
/// already carry a static count; only unresolved (`repeat-until`) rows are
/// joined. A count the plan never resolved stays `None` → renders `k/?`.
pub fn join_loop_counts(steps: &mut [PlanStep], counts: &HashMap<NodeId, u32>) {
    for s in steps.iter_mut() {
        if s.kind == StepKind::Loop && s.count.is_none() {
            s.count = counts.get(&s.id).copied();
        }
    }
}
