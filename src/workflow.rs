//! The workflow-module protocol (`plans/DYNAMIC_WORKFLOWS.md` §2), implemented
//! ONCE on the Rust side. A workflow `.fnl` file no longer evaluates to a bare
//! AST: it evaluates to a **module** — `{:doc <string?> :params <schema?>
//! :build (fn [params ctx] ast-or-nil)}`. This module is the single seam
//! `planner`, `live`, and the TUI go through; each used to call `eval_fennel`
//! directly.
//!
//! [`load`] evaluates the source, validates it exports the protocol, coerces the
//! caller's raw string params through `fennel.lib.params` (validation + coercion
//! live in Fennel, one implementation for every entry point), and calls `build`.
//! [`schema`] marshals just `:doc` + `:params` without calling `build` — the CLI
//! help and the TUI workflow list read it.

use anyhow::{anyhow, Context, Result};
use mlua::prelude::*;

use crate::lua::{eval_fennel, require_module};

/// The nine declared param types (`plans/DYNAMIC_WORKFLOWS.md` §2.2). `:string`
/// plus the five game-code types carry plain strings; `:number`/`:bool`/`:enum`
/// are the coercing/constraining ones. Semantic validity of a game code is a
/// downstream host lookup, never checked here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    String,
    Number,
    Bool,
    Enum,
    Item,
    Resource,
    Monster,
    Npc,
    Skill,
}

impl ParamType {
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "string" => Self::String,
            "number" => Self::Number,
            "bool" => Self::Bool,
            "enum" => Self::Enum,
            "item" => Self::Item,
            "resource" => Self::Resource,
            "monster" => Self::Monster,
            "npc" => Self::Npc,
            "skill" => Self::Skill,
            other => {
                return Err(anyhow!(
                    "unknown param type ':{other}' (expected one of :string :number \
                     :bool :enum :item :resource :monster :npc :skill)"
                ))
            }
        })
    }

    /// The keyword name (without the leading colon), for display.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Bool => "bool",
            Self::Enum => "enum",
            Self::Item => "item",
            Self::Resource => "resource",
            Self::Monster => "monster",
            Self::Npc => "npc",
            Self::Skill => "skill",
        }
    }
}

/// One declared param, marshalled from a workflow's `:params` schema. `default`
/// is stringified for display (it may be authored as a number/bool in Fennel).
#[derive(Debug, Clone)]
pub struct ParamSpec {
    pub name: String,
    pub ptype: ParamType,
    pub required: bool,
    pub default: Option<String>,
    pub doc: Option<String>,
    pub options: Vec<String>,
}

/// A workflow's declared surface, sans `build`: its one-line `:doc` and its
/// params (sorted by name for deterministic output).
#[derive(Debug, Clone)]
pub struct WorkflowInfo {
    pub doc: Option<String>,
    pub params: Vec<ParamSpec>,
}

/// Load a workflow: evaluate `src`, validate the module protocol, coerce `params`
/// (raw `key=value` string pairs) through `fennel.lib.params`, and call
/// `build(coerced, ctx)`. Returns `Ok(None)` iff `build` returned nil (the
/// done-sentinel M7's campaign loop stops on), `Ok(Some(ast))` otherwise.
///
/// `ctx` is the read-only seed-state snapshot the caller built (the same table
/// the plan pass seeds from, or one built from the live view) — passed to
/// `build` as its second argument.
pub fn load(
    lua: &Lua,
    src: &str,
    name: &str,
    params: &[(String, String)],
    ctx: LuaTable,
) -> Result<Option<LuaValue>> {
    let module = eval_module(lua, src, name)?;
    let build = require_build(&module, name)?;

    // The declared schema (default empty), validated then used to coerce the raw
    // params — both in Fennel, so the CLI and a composing/TUI caller share one
    // implementation and one set of error messages.
    let schema = read_params_table(lua, &module)?;
    let params_mod = require_module(lua, "fennel.lib.params")
        .map_err(|e| anyhow!("require fennel.lib.params: {e}"))?;
    let validate: LuaFunction = params_mod
        .get("validate_schema")
        .map_err(|e| anyhow!("fennel.lib.params has no validate_schema: {e}"))?;
    validate
        .call::<LuaValue>(&schema)
        .map_err(|e| anyhow!("workflow '{name}': invalid :params schema: {e}"))?;

    let coerce: LuaFunction = params_mod
        .get("coerce")
        .map_err(|e| anyhow!("fennel.lib.params has no coerce: {e}"))?;
    let raw = lua.create_table().map_err(|e| anyhow!("{e}"))?;
    for (k, v) in params {
        raw.set(k.as_str(), v.as_str())
            .map_err(|e| anyhow!("{e}"))?;
    }
    let coerced: LuaValue = coerce
        .call((schema, raw))
        .map_err(|e| anyhow!("workflow '{name}': parameters: {e}"))?;

    let ast: LuaValue = build
        .call((coerced, ctx))
        .map_err(|e| anyhow!("workflow '{name}': build: {e}"))?;
    if ast.is_nil() {
        Ok(None)
    } else {
        Ok(Some(ast))
    }
}

/// Evaluate `src` and marshal just its `:doc` + `:params` schema — WITHOUT
/// calling `build`. The CLI usage errors and the TUI workflow list read this.
pub fn schema(lua: &Lua, src: &str, name: &str) -> Result<WorkflowInfo> {
    let module = eval_module(lua, src, name)?;
    // Enforce the protocol here too, so a bare-AST file surfaces the same loud
    // guidance whether it is planned, run, or merely listed.
    require_build(&module, name)?;

    let doc = module.get::<Option<String>>("doc").unwrap_or(None);
    let schema = read_params_table(lua, &module)?;

    let mut params = Vec::new();
    for pair in schema.pairs::<String, LuaTable>() {
        let (pname, spec) = pair.map_err(|e| anyhow!("workflow '{name}': reading :params: {e}"))?;
        let type_str: String = spec
            .get("type")
            .map_err(|_| anyhow!("workflow '{name}': param '{pname}' has no :type"))?;
        let ptype = ParamType::parse(&type_str)
            .with_context(|| format!("workflow '{name}': param '{pname}'"))?;
        let required: bool = spec.get("required").unwrap_or(false);
        let default = spec
            .get::<LuaValue>("default")
            .ok()
            .filter(|v| !v.is_nil())
            .map(|v| scalar_to_string(&v));
        let doc = spec.get::<Option<String>>("doc").unwrap_or(None);
        let options: Vec<String> = spec
            .get::<LuaTable>("options")
            .map(|t| t.sequence_values::<String>().flatten().collect())
            .unwrap_or_default();
        params.push(ParamSpec {
            name: pname,
            ptype,
            required,
            default,
            doc,
            options,
        });
    }
    params.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(WorkflowInfo { doc, params })
}

/// Evaluate `src` and require the result be a table (the module). A non-table
/// (e.g. a bare AST that somehow isn't a table, or a stray literal) gets the
/// same prescriptive guidance as a missing `:build`.
fn eval_module(lua: &Lua, src: &str, name: &str) -> Result<LuaTable> {
    let v = eval_fennel(lua, src, name).map_err(|e| anyhow!("load workflow '{name}': {e}"))?;
    match v {
        LuaValue::Table(t) => Ok(t),
        _ => Err(bare_ast_error(name)),
    }
}

/// A module must export a `:build` function. A bare AST (a table with `:type`
/// but no `:build`) or anything else fails with prescriptive guidance.
fn require_build(module: &LuaTable, name: &str) -> Result<LuaFunction> {
    match module.get::<LuaValue>("build") {
        Ok(LuaValue::Function(f)) => Ok(f),
        _ => Err(bare_ast_error(name)),
    }
}

/// `:params` defaults to an empty table when absent (`plans/DYNAMIC_WORKFLOWS.md`
/// §2.1). A non-table `:params` is a loud error.
fn read_params_table(lua: &Lua, module: &LuaTable) -> Result<LuaTable> {
    match module.get::<LuaValue>("params") {
        Ok(LuaValue::Nil) | Err(_) => lua.create_table().map_err(|e| anyhow!("{e}")),
        Ok(LuaValue::Table(t)) => Ok(t),
        Ok(other) => Err(anyhow!(
            "workflow :params must be a table of param specs, got {}",
            other.type_name()
        )),
    }
}

fn bare_ast_error(name: &str) -> anyhow::Error {
    anyhow!(
        "workflow '{name}' must export a module table {{:build ...}}; a bare AST \
         is no longer accepted — wrap it as `{{:build (fn [_ _] <ast>)}}` (see \
         plans/DYNAMIC_WORKFLOWS.md §2.1)"
    )
}

/// Stringify a scalar Lua value (for a param's `:default`, which may be authored
/// as a number/bool/string).
fn scalar_to_string(v: &LuaValue) -> String {
    match v {
        LuaValue::String(s) => s.to_string_lossy().to_string(),
        LuaValue::Integer(i) => i.to_string(),
        LuaValue::Number(n) => n.to_string(),
        LuaValue::Boolean(b) => b.to_string(),
        _ => String::new(),
    }
}
