// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave_execute{steps[{cmd, args}], dry_run?, expected_rev?}`: a transactional batch
//! (arch §5.2, §14).
//!
//! - **One transaction, one undo step.** The steps run in a `batch` transaction of the session's
//!   connection (`edit.begin_tx`, each step with `tx`, `edit.commit_tx`), so the document only
//!   changes on commit, as one history entry `edit.undo` reverts as a whole. Queries in a batch
//!   read the transaction's working document (read-after-write within the batch).
//! - **All or nothing.** When a step fails, the transaction is cancelled and the error names
//!   the step (`failed_step`, 0-based); `Invalid` paths point into the request
//!   (`/steps/2/args/at`). A commit refused because another actor committed first fails the
//!   whole batch too.
//! - **`$ref`.** Inside `args`, an object `{"$ref": "steps[0].output.left"}` is replaced by that
//!   value of an earlier step's output (also `steps[1].output.media_ids[0]`, or the JSON Pointer
//!   form `#/steps/0/output/left`). Malformed references and references to the same or a
//!   later step are refused before anything runs.
//! - **`dry_run`.** The steps run (validation, semantic checks, outputs) and the transaction is
//!   cancelled: nothing changes, and each step lists the ops it would apply.
//! - **`expected_rev`** is checked when the transaction opens and again when it commits, so the
//!   batch applies only to the document revision the agent read.
//! - **Engine commands** (`edit.redo`, `project.open`, `media.import`, `job.*`, `view.*`) act on
//!   engine state and cannot join a document transaction: one runs alone as a single step
//!   (without `dry_run`); in a longer batch it is refused before anything runs.
//!
//! Everything runs in one blocking task, so a cancelled MCP request still ends its transaction
//! (committed or cancelled), and a dropped session rolls back whatever its connection holds.

use cutweave_commands::{CmdError, EntryKind, ResourceKind, Rev, TxId};
use serde_json::{Map, Value, json};

use crate::mcp::{
    catalog::Catalog,
    dispatch::{DispatchCall, DispatchConn},
};

/// Most steps in one batch.
pub const MAX_STEPS: usize = 100;

/// Longest transaction label the engine accepts (`edit.begin_tx`).
const MAX_LABEL_CHARS: usize = 200;

/// One resolved step.
#[derive(Clone, Debug)]
pub(crate) struct Step {
    /// The registry name (`clip.split`; `clip_split` is accepted and resolved).
    pub name: &'static str,
    /// What kind of entry it is.
    pub kind: EntryKind,
    /// Its params, `$ref`s unresolved.
    pub args: Value,
}

/// A parsed `cutweave_execute` request.
#[derive(Clone, Debug)]
pub(crate) struct Batch {
    pub steps: Vec<Step>,
    pub dry_run: bool,
    pub expected_rev: Option<Rev>,
}

/// Why a batch failed.
#[derive(Clone, Debug)]
pub(crate) struct ExecError {
    /// The error; `Invalid` paths point into the request.
    pub error: CmdError,
    /// The 0-based index of the failing step; `None` when the batch as a whole failed (bad
    /// request shape, stale `expected_rev`, refused commit).
    pub failed_step: Option<usize>,
    /// The failing step's args (resolved when the step ran), to locate a missing id.
    pub args: Option<Value>,
    /// What to do about it, when more specific than the error kind's hint.
    pub hint: Option<&'static str>,
}

/// A batch result; the error is boxed (it is large and rare).
pub(crate) type ExecResult<T> = Result<T, Box<ExecError>>;

impl ExecError {
    fn batch(error: CmdError) -> Box<ExecError> {
        Box::new(ExecError {
            error,
            failed_step: None,
            args: None,
            hint: None,
        })
    }

    fn step(index: usize, error: CmdError, args: Value) -> Box<ExecError> {
        Box::new(ExecError {
            error: error.under(&format!("/steps/{index}/args")),
            failed_step: Some(index),
            args: Some(args),
            hint: None,
        })
    }

    fn at(index: usize, error: CmdError) -> Box<ExecError> {
        Box::new(ExecError {
            error,
            failed_step: Some(index),
            args: None,
            hint: None,
        })
    }
}

/// Parses and resolves a request; nothing runs yet.
pub(crate) fn parse(catalog: &Catalog, args: Value) -> ExecResult<Batch> {
    let invalid = |path: &str, msg: &str| ExecError::batch(CmdError::invalid(path, msg));
    let Value::Object(mut map) = args else {
        return Err(invalid(
            "",
            "expected an object `{steps, dry_run?, expected_rev?}`",
        ));
    };
    if let Some(key) = map
        .keys()
        .find(|k| !matches!(k.as_str(), "steps" | "dry_run" | "expected_rev"))
    {
        return Err(invalid(
            &format!("/{}", escape(key)),
            "unknown field; expected `steps`, `dry_run` or `expected_rev`",
        ));
    }
    let dry_run = match map.remove("dry_run") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => b,
        Some(_) => return Err(invalid("/dry_run", "expected a boolean")),
    };
    let expected_rev = match map.remove("expected_rev") {
        None | Some(Value::Null) => None,
        Some(v) => Some(Rev(v.as_u64().ok_or_else(|| {
            invalid("/expected_rev", "expected a non-negative integer revision")
        })?)),
    };
    let raw = match map.remove("steps") {
        Some(Value::Array(steps)) => steps,
        None => return Err(invalid("/steps", "missing: give at least one step")),
        Some(_) => return Err(invalid("/steps", "expected an array of {cmd, args}")),
    };
    if raw.is_empty() || raw.len() > MAX_STEPS {
        return Err(invalid(
            "/steps",
            &format!("expected 1..={MAX_STEPS} steps"),
        ));
    }
    let steps = raw
        .into_iter()
        .enumerate()
        .map(|(i, step)| parse_step(catalog, i, step))
        .collect::<ExecResult<Vec<Step>>>()?;
    check_engine_steps(&steps, dry_run)?;
    check_render_steps(&steps)?;
    Ok(Batch {
        steps,
        dry_run,
        expected_rev,
    })
}

/// One `{cmd, args}` step: the command resolved, its `$ref`s checked.
fn parse_step(catalog: &Catalog, i: usize, step: Value) -> ExecResult<Step> {
    let at = |field: &str, msg: &str| {
        ExecError::at(i, CmdError::invalid(format!("/steps/{i}{field}"), msg))
    };
    let Value::Object(mut step) = step else {
        return Err(at(
            "",
            "expected an object {\"cmd\": name, \"args\": {...}}",
        ));
    };
    if let Some(key) = step.keys().find(|k| !matches!(k.as_str(), "cmd" | "args")) {
        return Err(at(
            &format!("/{}", escape(key)),
            "unknown field; a step is {\"cmd\": name, \"args\": {...}}",
        ));
    }
    let cmd = match step.remove("cmd") {
        Some(Value::String(cmd)) => cmd,
        None => return Err(at("/cmd", "missing: the command name, e.g. clip.split")),
        Some(_) => return Err(at("/cmd", "expected a command name, e.g. clip.split")),
    };
    let args = match step.remove("args") {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(Value::Object(args)) => Value::Object(args),
        Some(_) => return Err(at("/args", "expected an object of params")),
    };
    let Some(entry) = catalog.reachable(&cmd) else {
        return Err(ExecError::at(
            i,
            CmdError::not_found(ResourceKind::Command, &cmd),
        ));
    };
    check_step_refs(&args, i).map_err(|e| ExecError::step(i, e, args.clone()))?;
    Ok(Step {
        name: entry.meta.name,
        kind: entry.meta.kind,
        args,
    })
}

/// `render.*` queries render the committed document on the engine's renderer, which a
/// transaction's working document cannot use: refused before anything runs, with advice
/// ([`frame::TRANSACTION_HINT`]). This is the smaller of the two fixes (rendering a
/// transaction's view would put GPU work on the doc actor's thread).
fn check_render_steps(steps: &[Step]) -> ExecResult<()> {
    let Some((i, step)) = steps
        .iter()
        .enumerate()
        .find(|(_, s)| s.name.starts_with("render."))
    else {
        return Ok(());
    };
    Err(Box::new(ExecError {
        error: CmdError::unsupported(format!(
            "`{}` cannot run inside a cutweave_execute transaction",
            step.name
        )),
        failed_step: Some(i),
        args: None,
        hint: Some(crate::mcp::frame::TRANSACTION_HINT),
    }))
}

/// Engine commands cannot join a transaction (or be dry-run): refused before anything runs.
fn check_engine_steps(steps: &[Step], dry_run: bool) -> ExecResult<()> {
    if steps.len() > 1
        && let Some((i, step)) = steps
            .iter()
            .enumerate()
            .find(|(_, s)| s.kind == EntryKind::Engine)
    {
        return Err(ExecError::at(
            i,
            CmdError::invalid(
                format!("/steps/{i}/cmd"),
                format!(
                    "`{}` acts on engine state (history, jobs, import, project, view) and cannot \
                     join a transaction; run it as the only step, or with its own tool",
                    step.name
                ),
            ),
        ));
    }
    if dry_run
        && let [step] = steps
        && step.kind == EntryKind::Engine
    {
        return Err(ExecError::at(
            0,
            CmdError::unsupported(format!(
                "`{}` acts on engine state and cannot be dry-run",
                step.name
            )),
        ));
    }
    Ok(())
}

/// Runs `batch` on `conn` (blocking; see the module docs). Returns the structured result.
pub(crate) fn run(conn: &dyn DispatchConn, batch: &Batch) -> ExecResult<Value> {
    if let [step] = batch.steps.as_slice()
        && step.kind == EntryKind::Engine
    {
        return run_alone(conn, step, batch.expected_rev);
    }
    let opened = conn
        .call(
            DispatchCall::new(
                "edit.begin_tx",
                json!({ "kind": "batch", "label": label(&batch.steps) }),
            )
            .expecting(batch.expected_rev),
        )
        .map_err(ExecError::batch)?;
    let tx: TxId = opened
        .output
        .get("tx")
        .and_then(Value::as_str)
        .and_then(|t| t.parse().ok())
        .ok_or_else(|| {
            ExecError::batch(CmdError::internal(
                "edit.begin_tx returned no transaction id",
            ))
        })?;
    let mut outputs: Vec<Value> = Vec::with_capacity(batch.steps.len());
    let mut results = Vec::with_capacity(batch.steps.len());
    for (i, step) in batch.steps.iter().enumerate() {
        let args = match resolve_refs(&step.args, &outputs, i) {
            Ok(args) => args,
            Err(e) => {
                cancel(conn, tx);
                return Err(ExecError::step(i, e, step.args.clone()));
            }
        };
        match conn.call(DispatchCall::new(step.name, args.clone()).in_tx(tx)) {
            Ok(reply) => {
                let mut result = json!({ "cmd": step.name, "output": reply.output });
                if batch.dry_run {
                    result["ops"] = json!(reply.ops.unwrap_or_default());
                }
                outputs.push(reply.output);
                results.push(result);
            }
            Err(e) => {
                cancel(conn, tx);
                return Err(ExecError::step(i, e, args));
            }
        }
    }
    if batch.dry_run {
        let cancelled = cancel(conn, tx);
        let (rev, committed_rev) = cancelled.map_or((opened.rev, opened.committed_rev), |r| {
            (r.rev, r.committed_rev)
        });
        return Ok(json!({
            "dry_run": true,
            "rev": rev,
            "committed_rev": committed_rev,
            "steps": results,
        }));
    }
    match conn.call(
        DispatchCall::new("edit.commit_tx", json!({ "tx": tx })).expecting(batch.expected_rev),
    ) {
        Ok(committed) => Ok(json!({
            "rev": committed.output.get("rev").cloned().unwrap_or(json!(committed.rev)),
            "committed_rev": committed
                .output
                .get("committed_rev")
                .cloned()
                .unwrap_or(json!(committed.committed_rev)),
            "changed": committed.output.get("changed").cloned().unwrap_or(json!(true)),
            "steps": results,
        })),
        Err(e) => {
            // A refused commit closes the transaction; a stale `expected_rev` does not.
            cancel(conn, tx);
            Err(ExecError::batch(e))
        }
    }
}

/// A single engine-command step: a plain call, outside any transaction.
fn run_alone(conn: &dyn DispatchConn, step: &Step, expected_rev: Option<Rev>) -> ExecResult<Value> {
    let args =
        resolve_refs(&step.args, &[], 0).map_err(|e| ExecError::step(0, e, step.args.clone()))?;
    let reply = conn
        .call(DispatchCall::new(step.name, args.clone()).expecting(expected_rev))
        .map_err(|e| ExecError::step(0, e, args))?;
    Ok(json!({
        "rev": reply.rev,
        "committed_rev": reply.committed_rev,
        "steps": [{ "cmd": step.name, "output": reply.output }],
    }))
}

/// Cancels `tx`, logging (never failing) when it is already closed.
fn cancel(conn: &dyn DispatchConn, tx: TxId) -> Option<crate::mcp::dispatch::DispatchReply> {
    match conn.call(DispatchCall::new("edit.cancel_tx", json!({ "tx": tx }))) {
        Ok(reply) => Some(reply),
        Err(error) => {
            tracing::debug!(%tx, %error, "cutweave_execute: transaction already closed");
            None
        }
    }
}

/// The undo entry's label: `cutweave_execute: clip.split, clip.trim`.
fn label(steps: &[Step]) -> String {
    let names: Vec<&str> = steps.iter().map(|s| s.name).collect();
    let label = format!("cutweave_execute: {}", names.join(", "));
    if label.chars().count() <= MAX_LABEL_CHARS {
        return label;
    }
    let mut short: String = label.chars().take(MAX_LABEL_CHARS - 1).collect();
    short.push('…');
    short
}

/// JSON Pointer escaping of one reference token (RFC 6901).
pub(crate) fn escape(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

/// Replaces every `{"$ref": "..."}` in `args` of step `step` by the referenced value of an
/// earlier step's output (`outputs[N]` is step N's). `Invalid` paths are relative to `args`.
///
/// Public because `cutweave exec` batches use the same grammar (WP-1O.02 parity).
pub fn resolve_refs(args: &Value, outputs: &[Value], step: usize) -> Result<Value, CmdError> {
    let mut out = args.clone();
    resolve_in(&mut out, outputs, step, &mut String::new())?;
    Ok(out)
}

/// Checks every `$ref` in `args` of step `step` before anything runs: well-formed, and naming
/// an earlier step. Whether the field exists is only known once that step ran. `Invalid` paths
/// are relative to `args`.
///
/// Public because `cutweave exec` batches use the same grammar (WP-1O.02 parity).
pub fn check_step_refs(args: &Value, step: usize) -> Result<(), CmdError> {
    check_refs(args, step, &mut String::new())
}

fn check_refs(args: &Value, step: usize, path: &mut String) -> Result<(), CmdError> {
    match args {
        Value::Object(map) => {
            if map.len() == 1
                && let Some(Value::String(reference)) = map.get("$ref")
            {
                let (index, _) =
                    parse_ref(reference).map_err(|msg| CmdError::invalid(path.clone(), msg))?;
                if index >= step {
                    return Err(CmdError::invalid(path.clone(), not_yet(reference, index)));
                }
                return Ok(());
            }
            for (key, child) in map {
                let len = path.len();
                path.push('/');
                path.push_str(&escape(key));
                check_refs(child, step, path)?;
                path.truncate(len);
            }
        }
        Value::Array(items) => {
            for (i, child) in items.iter().enumerate() {
                let len = path.len();
                path.push('/');
                path.push_str(&i.to_string());
                check_refs(child, step, path)?;
                path.truncate(len);
            }
        }
        _ => {}
    }
    Ok(())
}

fn not_yet(reference: &str, index: usize) -> String {
    format!(
        "`{reference}` refers to step {index}, which has not run yet; only the outputs of \
         earlier steps can be used"
    )
}

fn resolve_in(
    value: &mut Value,
    outputs: &[Value],
    step: usize,
    path: &mut String,
) -> Result<(), CmdError> {
    match value {
        Value::Object(map) => {
            if map.len() == 1
                && let Some(Value::String(reference)) = map.get("$ref")
            {
                *value = lookup(reference, outputs, step)
                    .map_err(|msg| CmdError::invalid(path.clone(), msg))?;
                return Ok(());
            }
            for (key, child) in map.iter_mut() {
                let len = path.len();
                path.push('/');
                path.push_str(&escape(key));
                resolve_in(child, outputs, step, path)?;
                path.truncate(len);
            }
        }
        Value::Array(items) => {
            for (i, child) in items.iter_mut().enumerate() {
                let len = path.len();
                path.push('/');
                path.push_str(&i.to_string());
                resolve_in(child, outputs, step, path)?;
                path.truncate(len);
            }
        }
        _ => {}
    }
    Ok(())
}

/// One segment of a reference below `steps[N].output`.
#[derive(Debug, PartialEq, Eq)]
enum Segment {
    Key(String),
    Index(usize),
}

const REF_FORM: &str = "expected `steps[N].output`, then `.field` or `[index]` parts (e.g. \
                        `steps[0].output.left`), or `#/steps/N/output/...`";

/// Parses `steps[N].output.a[0].b` or `#/steps/N/output/a/0/b` into (N, segments).
fn parse_ref(reference: &str) -> Result<(usize, Vec<Segment>), String> {
    let bad = || format!("bad $ref `{reference}`: {REF_FORM}");
    if let Some(pointer) = reference
        .strip_prefix("#/")
        .or_else(|| reference.strip_prefix('/'))
    {
        let mut tokens = pointer
            .split('/')
            .map(|t| t.replace("~1", "/").replace("~0", "~"));
        let (Some(steps), Some(index), Some(output)) =
            (tokens.next(), tokens.next(), tokens.next())
        else {
            return Err(bad());
        };
        if steps != "steps" || output != "output" {
            return Err(bad());
        }
        let index = index.parse().map_err(|_| bad())?;
        let segments = tokens
            .map(|t| match t.parse::<usize>() {
                Ok(i) => Segment::Index(i),
                Err(_) => Segment::Key(t),
            })
            .collect();
        return Ok((index, segments));
    }
    let rest = reference.strip_prefix("steps[").ok_or_else(bad)?;
    let (index, rest) = rest.split_once(']').ok_or_else(bad)?;
    let index = index.parse().map_err(|_| bad())?;
    let mut rest = rest.strip_prefix(".output").ok_or_else(bad)?;
    let mut segments = Vec::new();
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('.') {
            let end = after.find(['.', '[']).unwrap_or(after.len());
            if end == 0 {
                return Err(bad());
            }
            segments.push(Segment::Key(after[..end].to_owned()));
            rest = &after[end..];
        } else if let Some(after) = rest.strip_prefix('[') {
            let (i, after) = after.split_once(']').ok_or_else(bad)?;
            segments.push(Segment::Index(i.parse().map_err(|_| bad())?));
            rest = after;
        } else {
            return Err(bad());
        }
    }
    Ok((index, segments))
}

fn lookup(reference: &str, outputs: &[Value], step: usize) -> Result<Value, String> {
    let (index, segments) = parse_ref(reference)?;
    if index >= step {
        return Err(not_yet(reference, index));
    }
    let mut current = outputs
        .get(index)
        .ok_or_else(|| format!("`{reference}`: step {index} has no output"))?;
    for segment in &segments {
        let next = match (segment, current) {
            (Segment::Key(key), Value::Object(map)) => map.get(key),
            (Segment::Index(i), Value::Array(items)) => items.get(*i),
            (Segment::Index(i), Value::Object(map)) => map.get(&i.to_string()),
            _ => None,
        };
        current = next.ok_or_else(|| {
            let keys = match current {
                Value::Object(map) => format!(
                    " (it has: {})",
                    map.keys().cloned().collect::<Vec<_>>().join(", ")
                ),
                _ => String::new(),
            };
            format!("`{reference}`: step {index}'s output has no such value{keys}")
        })?;
    }
    Ok(current.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_reference_forms() {
        assert_eq!(
            parse_ref("steps[0].output.left"),
            Ok((0, vec![Segment::Key("left".into())]))
        );
        assert_eq!(
            parse_ref("steps[12].output.media_ids[1]"),
            Ok((
                12,
                vec![Segment::Key("media_ids".into()), Segment::Index(1)]
            ))
        );
        assert_eq!(parse_ref("steps[3].output"), Ok((3, vec![])));
        assert_eq!(
            parse_ref("#/steps/1/output/a~1b/0"),
            Ok((1, vec![Segment::Key("a/b".into()), Segment::Index(0)]))
        );
        for bad in [
            "steps[x].output",
            "steps[0]",
            "steps[0].outputs",
            "step[0].output",
            "steps[0].output..a",
            "steps[0].output[",
            "#/steps/0",
            "#/stuff/0/output",
        ] {
            assert!(parse_ref(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn resolves_refs_deep_inside_args() {
        let outputs = vec![json!({ "left": "itm_a", "ids": ["x", "y"] })];
        let args = json!({
            "item": { "$ref": "steps[0].output.left" },
            "list": [{ "$ref": "steps[0].output.ids[1]" }],
            "keep": { "$ref": "not a ref", "other": 1 }
        });
        assert_eq!(
            resolve_refs(&args, &outputs, 1).unwrap(),
            json!({
                "item": "itm_a",
                "list": ["y"],
                "keep": { "$ref": "not a ref", "other": 1 }
            })
        );
        let err = resolve_refs(
            &json!({ "a": [{ "$ref": "steps[1].output" }] }),
            &outputs,
            1,
        )
        .unwrap_err();
        assert_eq!(err.code(), "invalid");
        assert!(
            matches!(&err, CmdError::Invalid { path, .. } if path == "/a/0"),
            "{err}"
        );
        let err = resolve_refs(
            &json!({ "a": { "$ref": "steps[0].output.nope" } }),
            &outputs,
            1,
        )
        .unwrap_err();
        assert!(err.to_string().contains("it has: left, ids"), "{err}");
    }

    #[test]
    fn labels_are_bounded() {
        let step = Step {
            name: "clip.split",
            kind: EntryKind::Command,
            args: json!({}),
        };
        assert_eq!(
            label(std::slice::from_ref(&step)),
            "cutweave_execute: clip.split"
        );
        let long = label(&vec![step; 50]);
        assert_eq!(long.chars().count(), MAX_LABEL_CHARS);
        assert!(long.ends_with('…'));
    }
}
