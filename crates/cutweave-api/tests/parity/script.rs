// SPDX-License-Identifier: GPL-3.0-or-later
//! The command-script format (`testdata/scripts/*.json`, documented in
//! `testdata/scripts/README.md`) and its runner.
//!
//! ```json
//! {
//!   "description": "What the script covers",
//!   "steps": [
//!     { "cmd": "project.new", "args": { "name": "Parity" } },
//!     { "cmd": "media.import", "args": { "paths": [{ "$media": "a.mp4" }] } },
//!     { "batch": [
//!         { "cmd": "clip.split", "args": { "item": { "$script": "steps[3].output.clips[0]" }, "at": "2s" } },
//!         { "cmd": "item.delete", "args": { "items": [{ "$ref": "steps[0].output.right" }] } }
//!       ] },
//!     { "cmd": "clip.split", "args": { "item": "itm_00000000000000000000000000000000", "at": 0 },
//!       "expect": { "error": "not_found" } }
//!   ]
//! }
//! ```
//!
//! - A step is one call (`cmd`, `args`) or a `batch` of calls that runs as one transaction.
//! - `{"$ref": "steps[N].output.<path>"}` (the `cutweave_execute` grammar) is the output of an
//!   earlier step of the enclosing request: of the script at the top level (the harness resolves
//!   it), of the batch inside a `batch` (sent as is: the surface resolves it).
//! - `{"$script": "steps[N].output.<path>"}` is always a script step's output (resolved by the
//!   harness, also inside a batch).
//! - `{"$media": "<name>"}` is the absolute path of a stub media file `<root>/media/<name>` the
//!   harness creates (the same path for every surface).
//! - `expect` is `{"error": "<code>", "failed_step"?: N}`; without it the step must succeed.

use std::path::Path;

use serde_json::{Map, Value, json};

/// One parsed script.
#[derive(Clone, Debug)]
pub struct Script {
    /// The file name without `.json`.
    pub name: String,
    pub steps: Vec<Step>,
}

/// One step of a script.
#[derive(Clone, Debug)]
pub struct Step {
    pub action: Action,
    pub expect: Expect,
}

/// What a step runs.
#[derive(Clone, Debug)]
pub enum Action {
    /// One call.
    Call(Call),
    /// Calls run as one transaction.
    Batch(Vec<Call>),
}

/// A command name and its args (references unresolved).
#[derive(Clone, Debug)]
pub struct Call {
    pub cmd: String,
    pub args: Value,
}

/// The outcome a step must have.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expect {
    Ok,
    Error {
        code: String,
        failed_step: Option<u64>,
    },
}

/// What a surface returned for one step: the outputs (one per call) or the failure.
pub type Outcome = Result<Vec<Value>, StepError>;

/// A failed step as a surface reported it.
#[derive(Clone, Debug)]
pub struct StepError {
    /// The engine's machine code (`not_found`, `invalid`, ...).
    pub code: String,
    /// The failing call of a batch (0-based), when the surface names it.
    pub failed_step: Option<u64>,
    /// The whole error as the surface reported it (for messages).
    pub raw: Value,
}

impl Script {
    /// Parses `path`.
    pub fn load(path: &Path) -> Script {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("a script file name")
            .to_owned();
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let value: Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("{}: not JSON: {e}", path.display()));
        Script::parse(&name, &value).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// Parses a script document.
    pub fn parse(name: &str, value: &Value) -> Result<Script, String> {
        let Value::Object(map) = value else {
            return Err("expected an object {description, steps}".into());
        };
        if let Some(key) = map
            .keys()
            .find(|k| !matches!(k.as_str(), "description" | "steps"))
        {
            return Err(format!("unknown field `{key}`"));
        }
        if !map.get("description").is_some_and(Value::is_string) {
            return Err("`description` must be a string".into());
        }
        let Some(Value::Array(raw)) = map.get("steps") else {
            return Err("`steps` must be an array".into());
        };
        if raw.is_empty() {
            return Err("`steps` is empty".into());
        }
        let steps = raw
            .iter()
            .enumerate()
            .map(|(i, s)| parse_step(s).map_err(|e| format!("steps[{i}]: {e}")))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Script {
            name: name.to_owned(),
            steps,
        })
    }

    /// The stub media file names the script references (`$media`), sorted and unique.
    pub fn media(&self) -> Vec<String> {
        let mut names = Vec::new();
        for step in &self.steps {
            for call in step.calls() {
                collect_media(&call.args, &mut names);
            }
        }
        names.sort();
        names.dedup();
        names
    }
}

impl Step {
    /// The calls of the step.
    pub fn calls(&self) -> &[Call] {
        match &self.action {
            Action::Call(call) => std::slice::from_ref(call),
            Action::Batch(calls) => calls,
        }
    }

    /// Checks `outcome` against `expect`.
    pub fn check(&self, outcome: &Outcome) -> Result<(), String> {
        match (&self.expect, outcome) {
            (Expect::Ok, Ok(_)) => Ok(()),
            (Expect::Ok, Err(f)) => Err(format!("expected success, got {}", f.raw)),
            (Expect::Error { code, .. }, Ok(out)) => {
                Err(format!("expected `{code}`, got success: {}", json!(out)))
            }
            (Expect::Error { code, failed_step }, Err(f)) => {
                if f.code != *code {
                    return Err(format!("expected `{code}`, got `{}`: {}", f.code, f.raw));
                }
                if failed_step.is_some() && f.failed_step != *failed_step {
                    return Err(format!(
                        "expected failed_step {failed_step:?}, got {:?}: {}",
                        f.failed_step, f.raw
                    ));
                }
                Ok(())
            }
        }
    }
}

fn parse_step(value: &Value) -> Result<Step, String> {
    let Value::Object(map) = value else {
        return Err("expected an object".into());
    };
    if let Some(key) = map
        .keys()
        .find(|k| !matches!(k.as_str(), "cmd" | "args" | "batch" | "expect"))
    {
        return Err(format!("unknown field `{key}`"));
    }
    let action = match (map.get("cmd"), map.get("batch")) {
        (Some(_), None) => Action::Call(parse_call(value)?),
        (None, Some(Value::Array(calls))) if !calls.is_empty() => Action::Batch(
            calls
                .iter()
                .enumerate()
                .map(|(i, c)| parse_call(c).map_err(|e| format!("batch[{i}]: {e}")))
                .collect::<Result<_, _>>()?,
        ),
        _ => return Err("expected `cmd` (one call) or a non-empty `batch`".into()),
    };
    let expect = match map.get("expect") {
        None => Expect::Ok,
        Some(Value::Object(e)) => {
            if let Some(key) = e
                .keys()
                .find(|k| !matches!(k.as_str(), "error" | "failed_step"))
            {
                return Err(format!("expect: unknown field `{key}`"));
            }
            let Some(Value::String(code)) = e.get("error") else {
                return Err("expect.error must be an error code".into());
            };
            Expect::Error {
                code: code.clone(),
                failed_step: e.get("failed_step").and_then(Value::as_u64),
            }
        }
        Some(_) => return Err("expect must be {error, failed_step?}".into()),
    };
    Ok(Step { action, expect })
}

fn parse_call(value: &Value) -> Result<Call, String> {
    let Value::Object(map) = value else {
        return Err("expected {cmd, args}".into());
    };
    let Some(Value::String(cmd)) = map.get("cmd") else {
        return Err("`cmd` must be a command name".into());
    };
    let args = match map.get("args") {
        None => Value::Object(Map::new()),
        Some(args @ Value::Object(_)) => args.clone(),
        Some(_) => return Err("`args` must be an object".into()),
    };
    Ok(Call {
        cmd: cmd.clone(),
        args,
    })
}

fn collect_media(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let (1, Some(Value::String(name))) = (map.len(), map.get("$media")) {
                out.push(name.clone());
            } else {
                map.values().for_each(|v| collect_media(v, out));
            }
        }
        Value::Array(items) => items.iter().for_each(|v| collect_media(v, out)),
        _ => {}
    }
}

/// Where a reference may point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// A top-level step: `$ref` and `$script` both name script steps.
    Script,
    /// A call inside a batch: `$ref` names batch calls and is left for the surface.
    Batch,
}

/// Resolves the harness's references in `args`: `$media`, `$script`, and `$ref` at the top
/// level. `outputs[N]` is script step N's output (for a batch, the array of its calls'
/// outputs: `steps[N].output[1].right`; `None` for a step that failed).
pub fn resolve(
    args: &Value,
    scope: Scope,
    outputs: &[Option<Value>],
    media: &Path,
) -> Result<Value, String> {
    let mut out = args.clone();
    resolve_in(&mut out, scope, outputs, media)?;
    Ok(out)
}

fn resolve_in(
    value: &mut Value,
    scope: Scope,
    outputs: &[Option<Value>],
    media: &Path,
) -> Result<(), String> {
    match value {
        Value::Object(map) => {
            if map.len() == 1 {
                let (key, target) = map.iter().next().expect("one entry");
                if let Value::String(target) = target {
                    let resolved = match key.as_str() {
                        "$media" => Some(json!(media.join(target).to_string_lossy())),
                        "$script" => Some(lookup(target, outputs)?),
                        "$ref" if scope == Scope::Script => Some(lookup(target, outputs)?),
                        _ => None,
                    };
                    if let Some(resolved) = resolved {
                        *value = resolved;
                        return Ok(());
                    }
                }
            }
            for child in map.values_mut() {
                resolve_in(child, scope, outputs, media)?;
            }
        }
        Value::Array(items) => {
            for child in items {
                resolve_in(child, scope, outputs, media)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// `steps[N].output` followed by `.key` / `[index]` parts.
pub fn parse_reference(reference: &str) -> Result<(usize, Vec<String>), String> {
    let bad = || format!("bad reference `{reference}`: expected steps[N].output.<path>");
    let rest = reference.strip_prefix("steps[").ok_or_else(bad)?;
    let (index, rest) = rest.split_once(']').ok_or_else(bad)?;
    let index: usize = index.parse().map_err(|_| bad())?;
    let mut rest = rest.strip_prefix(".output").ok_or_else(bad)?;
    let mut path = Vec::new();
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('.') {
            let end = after.find(['.', '[']).unwrap_or(after.len());
            if end == 0 {
                return Err(bad());
            }
            path.push(after[..end].to_owned());
            rest = &after[end..];
        } else if let Some(after) = rest.strip_prefix('[') {
            let (i, after) = after.split_once(']').ok_or_else(bad)?;
            i.parse::<usize>().map_err(|_| bad())?;
            path.push(i.to_owned());
            rest = after;
        } else {
            return Err(bad());
        }
    }
    Ok((index, path))
}

/// The value `reference` names in `outputs`.
pub fn lookup(reference: &str, outputs: &[Option<Value>]) -> Result<Value, String> {
    let (index, path) = parse_reference(reference)?;
    let Some(Some(output)) = outputs.get(index) else {
        // Also a reference to the step itself or a later one.
        return Err(format!(
            "`{reference}`: step {index} has not run or did not succeed"
        ));
    };
    let mut current = output;
    for part in &path {
        current = match current {
            Value::Object(map) => map.get(part),
            Value::Array(items) => part.parse::<usize>().ok().and_then(|i| items.get(i)),
            _ => None,
        }
        .ok_or_else(|| format!("`{reference}`: no `{part}` in {current}"))?;
    }
    Ok(current.clone())
}

/// Resolves the batch-scoped `$ref`s of `args` against the outputs of the batch's earlier calls
/// (what the raw JSON-RPC client does itself: JSON-RPC has no batch references).
pub fn resolve_batch_refs(args: &Value, earlier: &[Value]) -> Result<Value, String> {
    let outputs: Vec<Option<Value>> = earlier.iter().cloned().map(Some).collect();
    let mut out = args.clone();
    resolve_refs_in(&mut out, &outputs)?;
    Ok(out)
}

fn resolve_refs_in(value: &mut Value, outputs: &[Option<Value>]) -> Result<(), String> {
    match value {
        Value::Object(map) => {
            if let (1, Some(Value::String(target))) = (map.len(), map.get("$ref")) {
                *value = lookup(target, outputs)?;
                return Ok(());
            }
            for child in map.values_mut() {
                resolve_refs_in(child, outputs)?;
            }
        }
        Value::Array(items) => {
            for child in items {
                resolve_refs_in(child, outputs)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[test]
fn script_references_parse_and_resolve() {
    assert_eq!(
        parse_reference("steps[2].output.clips[0]").unwrap(),
        (2, vec!["clips".to_owned(), "0".to_owned()])
    );
    assert!(parse_reference("steps[x].output").is_err());
    let outputs = vec![Some(json!({ "clips": ["itm_a"] })), None];
    assert_eq!(
        lookup("steps[0].output.clips[0]", &outputs).unwrap(),
        "itm_a"
    );
    assert!(lookup("steps[1].output", &outputs).is_err());
    let media = Path::new("/m");
    let args = json!({ "a": { "$ref": "steps[0].output.clips" }, "b": { "$media": "x.mp4" } });
    let top = resolve(&args, Scope::Script, &outputs, media).unwrap();
    assert_eq!(top["a"], json!(["itm_a"]));
    assert!(top["b"].as_str().unwrap().ends_with("x.mp4"));
    let inner = resolve(&args, Scope::Batch, &outputs, media).unwrap();
    assert_eq!(inner["a"], json!({ "$ref": "steps[0].output.clips" }));
}
