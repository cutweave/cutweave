// SPDX-License-Identifier: GPL-3.0-or-later
//! `cargo xtask check-fixtures` (WP-2A.01): checks the timeline edit-semantics fixtures in
//! `docs/spec/fixtures/` against `docs/spec/fixture.schema.json` and the spec
//! `docs/spec/timeline-edit-semantics.md` (§9.2):
//!
//! 1. every fixture matches the schema (a JSON Schema subset; unknown keywords are errors), its
//!    `id` is its file name and is unique, and every cited `INV-n` is defined in the spec;
//! 2. `edit` fixtures: names resolve, `before` and `after` keep the document invariants
//!    (INV-1, 2, 4, 5, 6, 7, 8, 9), locked tracks do not change (INV-28), new entities are
//!    named `+…`, and a stated `report` is the one derived from `before` and `after` (INV-21);
//! 3. `snap` and `three_point` fixtures agree with the reference implementations of spec §6 and
//!    §4.18 below;
//! 4. every command × tag cell is covered by a fixture or explained in
//!    `docs/spec/fixture-coverage.json`, and there are at least [`MIN_EDIT_FIXTURES`].
//!
//! The fixtures are the executable half of the spec: WP-2A.02–2A.14 run them against the real
//! commands. This checker only proves they are consistent with the spec's global rules.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::Value;

const SPEC: &str = "docs/spec/timeline-edit-semantics.md";
const SCHEMA: &str = "docs/spec/fixture.schema.json";
const COVERAGE: &str = "docs/spec/fixture-coverage.json";
const FIXTURES: &str = "docs/spec/fixtures";

/// The fewest `edit` fixtures the spec accepts (WP-2A.01 acceptance criteria).
pub const MIN_EDIT_FIXTURES: usize = 40;

const FLICKS_PER_SECOND: i64 = 705_600_000;

/// How many fixtures of each kind passed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// `edit` fixtures.
    pub edit: usize,
    /// `snap` fixtures.
    pub snap: usize,
    /// `three_point` fixtures.
    pub three_point: usize,
}

/// Runs the check from the workspace root; prints a summary or every problem found.
pub fn run(root: &Path, args: &[String]) -> bool {
    if let Some(arg) = args.first() {
        eprintln!("check-fixtures: unexpected argument `{arg}`");
        return false;
    }
    match check(root) {
        Ok(s) => {
            println!(
                "check-fixtures: ok ({} edit, {} snap, {} three_point fixtures)",
                s.edit, s.snap, s.three_point
            );
            true
        }
        Err(errors) => {
            for e in &errors {
                eprintln!("check-fixtures: {e}");
            }
            eprintln!("check-fixtures: {} problem(s)", errors.len());
            false
        }
    }
}

fn read_json(root: &Path, rel: &str) -> Result<Value, String> {
    let text = fs::read_to_string(root.join(rel)).map_err(|e| format!("{rel}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{rel}: {e}"))
}

/// The schema, the coverage table and the spec text.
fn load(root: &Path) -> Result<(Value, Value, String), String> {
    let schema = read_json(root, SCHEMA)?;
    let coverage = read_json(root, COVERAGE)?;
    let spec = fs::read_to_string(root.join(SPEC)).map_err(|e| format!("{SPEC}: {e}"))?;
    Ok((schema, coverage, spec))
}

/// Checks every fixture; `Err` lists every problem.
pub fn check(root: &Path) -> Result<Summary, Vec<String>> {
    let mut errors = Vec::new();
    let (schema, coverage, spec) = load(root).map_err(|e| vec![e])?;
    let invs = defined_invs(&spec);
    let mut files: Vec<_> = match fs::read_dir(root.join(FIXTURES)) {
        Ok(dir) => dir
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect(),
        Err(e) => return Err(vec![format!("{FIXTURES}: {e}")]),
    };
    files.sort();
    let mut summary = Summary::default();
    let mut ids = BTreeSet::new();
    let mut covered = BTreeSet::new();
    for path in &files {
        let stem = path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        let mut errs = Vec::new();
        match fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string()))
        {
            Ok(fx) => {
                check_fixture(
                    &fx,
                    &stem,
                    &schema,
                    &invs,
                    &mut errs,
                    &mut summary,
                    &mut covered,
                );
            }
            Err(e) => errs.push(e),
        }
        if !ids.insert(stem.clone()) {
            errs.push("duplicate fixture id".into());
        }
        errors.extend(
            errs.into_iter()
                .map(|e| format!("{FIXTURES}/{stem}.json: {e}")),
        );
    }
    errors.extend(check_coverage(&coverage, &covered, &spec, summary.edit));
    if errors.is_empty() {
        Ok(summary)
    } else {
        Err(errors)
    }
}

fn check_fixture(
    fx: &Value,
    stem: &str,
    schema: &Value,
    invs: &BTreeSet<String>,
    errs: &mut Vec<String>,
    summary: &mut Summary,
    covered: &mut BTreeSet<(String, String)>,
) {
    let kind = fx["kind"].as_str().unwrap_or_default();
    let def = match kind {
        "edit" | "snap" | "three_point" => &schema["$defs"][kind],
        _ => schema,
    };
    validate(schema, def, fx, "", errs);
    if !errs.is_empty() {
        return;
    }
    if fx["id"].as_str() != Some(stem) {
        errs.push(format!("`id` must be the file name `{stem}`"));
    }
    for cite in strings(&fx["cites"]) {
        if !invs.contains(&cite) {
            errs.push(format!("cites {cite}, which the spec does not define"));
        }
    }
    match kind {
        "edit" => {
            summary.edit += 1;
            let command = fx["command"].as_str().unwrap_or_default().to_owned();
            for tag in strings(&fx["tags"]) {
                covered.insert((command.clone(), tag));
            }
            errs.extend(check_edit(fx));
        }
        "snap" => {
            summary.snap += 1;
            errs.extend(check_snap(fx));
        }
        _ => {
            summary.three_point += 1;
            errs.extend(check_three_point(fx));
        }
    }
}

fn strings(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a
            .iter()
            .filter_map(|x| x.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

/// The `INV-n` ids the spec defines (written `**INV-n**`).
fn defined_invs(spec: &str) -> BTreeSet<String> {
    spec.split("**INV-")
        .skip(1)
        .filter_map(|rest| {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            (!digits.is_empty() && rest[digits.len()..].starts_with("**"))
                .then(|| format!("INV-{digits}"))
        })
        .collect()
}

fn check_coverage(
    coverage: &Value,
    covered: &BTreeSet<(String, String)>,
    spec: &str,
    edits: usize,
) -> Vec<String> {
    let mut errors = Vec::new();
    let commands = strings(&coverage["commands"]);
    let tags = strings(&coverage["tags"]);
    let na = coverage["not_applicable"].as_object();
    for (command, tag) in covered {
        if !commands.contains(command) {
            errors.push(format!(
                "{COVERAGE}: fixture command `{command}` is not listed"
            ));
        }
        if !tags.contains(tag) {
            errors.push(format!("{COVERAGE}: fixture tag `{tag}` is not listed"));
        }
    }
    for command in &commands {
        if !spec.contains(&format!("`{command}`")) {
            errors.push(format!(
                "{COVERAGE}: `{command}` does not appear in the spec"
            ));
        }
        for tag in &tags {
            let reason = na
                .and_then(|m| m.get(command))
                .and_then(|m| m.get(tag))
                .and_then(Value::as_str);
            let has = covered.contains(&(command.clone(), tag.clone()));
            match (has, reason) {
                (true, Some(_)) => errors.push(format!(
                    "{COVERAGE}: `{command}` × {tag} has fixtures; remove its not_applicable entry"
                )),
                (false, None) => errors.push(format!(
                    "no `edit` fixture covers `{command}` × {tag} (add one, or a reason in {COVERAGE})"
                )),
                (false, Some(r)) if r.trim().is_empty() => {
                    errors.push(format!("{COVERAGE}: `{command}` × {tag} needs a reason"));
                }
                _ => {}
            }
        }
    }
    if let Some(map) = na {
        for (command, cells) in map {
            for tag in cells.as_object().into_iter().flat_map(|c| c.keys()) {
                if !commands.contains(command) || !tags.contains(tag) {
                    errors.push(format!("{COVERAGE}: unknown cell `{command}` × {tag}"));
                }
            }
        }
    }
    if edits < MIN_EDIT_FIXTURES {
        errors.push(format!(
            "only {edits} edit fixtures; the spec needs at least {MIN_EDIT_FIXTURES}"
        ));
    }
    errors
}

// ----- JSON Schema subset --------------------------------------------------------------------

/// Validates `v` against `schema` (with `#/$defs/…` references resolved in `root`), appending
/// one message per problem. Supports the keywords `fixture.schema.json` uses; any other keyword
/// is reported, so the schema cannot silently outgrow the checker.
fn validate(root: &Value, schema: &Value, v: &Value, at: &str, errors: &mut Vec<String>) {
    let Some(obj) = schema.as_object() else {
        if schema != &Value::Bool(true) {
            errors.push(format!("{at}: unsupported schema {schema}"));
        }
        return;
    };
    let here = if at.is_empty() { "/" } else { at };
    for (key, s) in obj {
        match key.as_str() {
            "$schema" | "$id" | "title" | "description" | "$comment" | "$defs" => {}
            "$ref" => match s.as_str().and_then(|r| r.strip_prefix("#/$defs/")) {
                Some(name) if root["$defs"].get(name).is_some() => {
                    validate(root, &root["$defs"][name], v, at, errors);
                }
                _ => errors.push(format!("{here}: unresolvable $ref {s}")),
            },
            "type" => {
                if !strings(s).iter().any(|t| type_ok(t, v)) {
                    errors.push(format!("{here}: expected {s}, got {}", type_name(v)));
                }
            }
            "const" => {
                if v != s {
                    errors.push(format!("{here}: expected {s}"));
                }
            }
            "enum" => {
                if !s.as_array().is_some_and(|a| a.contains(v)) {
                    errors.push(format!("{here}: {v} is not one of {s}"));
                }
            }
            "properties" => {
                if let (Some(props), Some(o)) = (s.as_object(), v.as_object()) {
                    for (name, sub) in props {
                        if let Some(x) = o.get(name) {
                            validate(root, sub, x, &format!("{at}/{name}"), errors);
                        }
                    }
                }
            }
            "required" => {
                if let Some(o) = v.as_object() {
                    for name in strings(s) {
                        if !o.contains_key(&name) {
                            errors.push(format!("{here}: missing `{name}`"));
                        }
                    }
                }
            }
            "additionalProperties" => additional(root, obj, s, v, at, errors),
            "items" => {
                if let Some(a) = v.as_array() {
                    for (i, x) in a.iter().enumerate() {
                        validate(root, s, x, &format!("{at}/{i}"), errors);
                    }
                }
            }
            "minItems" => {
                if let (Some(n), Some(a)) = (s.as_u64(), v.as_array())
                    && u64::try_from(a.len()).unwrap_or(u64::MAX) < n
                {
                    errors.push(format!("{here}: needs at least {n} entries"));
                }
            }
            "minimum" => {
                if let (Some(m), Some(x)) = (s.as_f64(), v.as_f64())
                    && x < m
                {
                    errors.push(format!("{here}: {v} is below the minimum {s}"));
                }
            }
            "minLength" => {
                if let (Some(n), Some(x)) = (s.as_u64(), v.as_str())
                    && u64::try_from(x.chars().count()).unwrap_or(u64::MAX) < n
                {
                    errors.push(format!("{here}: shorter than {n} characters"));
                }
            }
            "oneOf" | "anyOf" => alternatives(root, key, s, v, at, errors),
            other => errors.push(format!("schema keyword `{other}` is not supported")),
        }
    }
}

fn additional(
    root: &Value,
    obj: &serde_json::Map<String, Value>,
    s: &Value,
    v: &Value,
    at: &str,
    errors: &mut Vec<String>,
) {
    let Some(o) = v.as_object() else { return };
    let props = obj.get("properties").and_then(Value::as_object);
    for (name, x) in o {
        if props.is_some_and(|p| p.contains_key(name)) {
            continue;
        }
        match s {
            Value::Bool(false) => errors.push(format!("{at}/{name}: unexpected property")),
            Value::Bool(true) => {}
            sub => validate(root, sub, x, &format!("{at}/{name}"), errors),
        }
    }
}

fn alternatives(root: &Value, key: &str, s: &Value, v: &Value, at: &str, errors: &mut Vec<String>) {
    let here = if at.is_empty() { "/" } else { at };
    let options = s.as_array().map_or(&[][..], Vec::as_slice);
    let results: Vec<Vec<String>> = options
        .iter()
        .map(|sub| {
            let mut e = Vec::new();
            validate(root, sub, v, at, &mut e);
            e
        })
        .collect();
    let passing = results.iter().filter(|e| e.is_empty()).count();
    let ok = if key == "oneOf" {
        passing == 1
    } else {
        passing >= 1
    };
    if !ok {
        let why: Vec<String> = results.into_iter().flatten().take(4).collect();
        errors.push(format!(
            "{here}: matches {passing} of the {} `{key}` alternatives ({})",
            options.len(),
            why.join("; ")
        ));
    }
}

fn type_ok(name: &str, v: &Value) -> bool {
    match name {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        _ => false,
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

// ----- edit fixtures -------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
struct Item {
    id: String,
    media: String,
    start: i64,
    dur: i64,
    source: i64,
    group: Option<String>,
    parent: Option<String>,
    enabled: bool,
}

impl Item {
    fn end(&self) -> i64 {
        self.start + self.dur
    }
}

#[derive(Clone, Debug)]
struct Track {
    name: String,
    kind: String,
    magnetic: bool,
    locked: bool,
    items: Vec<Item>,
}

#[derive(Clone, Debug)]
struct Media {
    kind: String,
    frames: Option<i64>,
    audio: bool,
}

fn opt_str(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

fn parse_tracks(v: &Value) -> Vec<Track> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|t| Track {
            name: t["track"].as_str().unwrap_or_default().to_owned(),
            kind: t["kind"].as_str().unwrap_or_default().to_owned(),
            magnetic: t["magnetic"].as_bool().unwrap_or(false),
            locked: t["locked"].as_bool().unwrap_or(false),
            items: t["items"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|i| Item {
                    id: i["id"].as_str().unwrap_or_default().to_owned(),
                    media: i["media"].as_str().unwrap_or_default().to_owned(),
                    start: i["start"].as_i64().unwrap_or_default(),
                    dur: i["dur"].as_i64().unwrap_or_default(),
                    source: i["in"].as_i64().unwrap_or_default(),
                    group: opt_str(&i["group"]),
                    parent: opt_str(&i["parent"]),
                    enabled: i["enabled"].as_bool().unwrap_or(true),
                })
                .collect(),
        })
        .collect()
}

fn parse_media(v: &Value) -> BTreeMap<String, Media> {
    v.as_object()
        .into_iter()
        .flatten()
        .map(|(name, m)| {
            let media = Media {
                kind: m["kind"].as_str().unwrap_or_default().to_owned(),
                frames: m["frames"].as_i64(),
                audio: m["audio"].as_bool().unwrap_or(false),
            };
            (name.clone(), media)
        })
        .collect()
}

fn check_edit(fx: &Value) -> Vec<String> {
    let media = parse_media(&fx["media"]);
    let before = parse_tracks(&fx["before"]);
    let mut errors: Vec<String> = doc_errors(&before, &media)
        .into_iter()
        .map(|e| format!("before: {e}"))
        .collect();
    for t in &before {
        if t.name.starts_with('+') || t.items.iter().any(|i| i.id.starts_with('+')) {
            errors.push(format!("before: track `{}` uses a `+` name", t.name));
        }
    }
    let error_code = fx["error"]["code"].as_str();
    if error_code != Some("not_found") {
        errors.extend(param_errors(&fx["params"], &before, &media));
    }
    if let Some(code) = error_code {
        if code == "locked" {
            let id = fx["error"]["id"].as_str().unwrap_or_default();
            if !before.iter().any(|t| t.locked && t.name == id) {
                errors.push(format!(
                    "error.id `{id}` must name a locked track of `before`"
                ));
            }
        }
        if fx.get("report").is_some() {
            errors.push("a failing fixture has no report".into());
        }
        return errors;
    }
    let after = parse_tracks(&fx["after"]);
    errors.extend(
        doc_errors(&after, &media)
            .into_iter()
            .map(|e| format!("after: {e}")),
    );
    let command = fx["command"].as_str().unwrap_or_default();
    errors.extend(transition_errors(command, &before, &after));
    if let Some(report) = fx.get("report") {
        errors.extend(report_errors(report, &before, &after));
    }
    errors
}

fn param_errors(params: &Value, tracks: &[Track], media: &BTreeMap<String, Media>) -> Vec<String> {
    let mut errors = Vec::new();
    let items: BTreeSet<&str> = tracks
        .iter()
        .flat_map(|t| t.items.iter().map(|i| i.id.as_str()))
        .collect();
    let names: BTreeSet<&str> = tracks.iter().map(|t| t.name.as_str()).collect();
    for (key, v) in params.as_object().into_iter().flatten() {
        for name in strings(v) {
            let resolves = match key.as_str() {
                "item" | "items" | "left" | "right" | "parent" => items.contains(name.as_str()),
                "track" | "to_track" | "audio_track" | "tracks" => names.contains(name.as_str()),
                "media" => media.contains_key(&name),
                "sequence" => name == "S",
                _ => true,
            };
            if !resolves {
                errors.push(format!("params.{key}: `{name}` does not resolve"));
            }
        }
    }
    errors
}

/// The document invariants of spec §2 that a fixture document can show.
fn doc_errors(tracks: &[Track], media: &BTreeMap<String, Media>) -> Vec<String> {
    let mut errors = Vec::new();
    let mut seen_tracks = BTreeSet::new();
    let mut by_id: BTreeMap<&str, (&Track, &Item)> = BTreeMap::new();
    for t in tracks {
        if !seen_tracks.insert(t.name.as_str()) {
            errors.push(format!("track `{}` appears twice", t.name));
        }
        for i in &t.items {
            if by_id.insert(i.id.as_str(), (t, i)).is_some() {
                errors.push(format!("item `{}` appears twice", i.id));
            }
            errors.extend(item_errors(t, i, media));
        }
        let mut sorted: Vec<&Item> = t.items.iter().collect();
        sorted.sort_by_key(|i| (i.start, i.id.clone()));
        for w in sorted.windows(2) {
            if w[1].start < w[0].end() {
                errors.push(format!(
                    "INV-1: `{}` and `{}` overlap on `{}`",
                    w[0].id, w[1].id, t.name
                ));
            }
        }
        if t.magnetic {
            if t.kind != "video" {
                errors.push(format!("INV-6: magnetic track `{}` is not video", t.name));
            }
            let mut end = 0;
            for i in &sorted {
                if i.start != end {
                    errors.push(format!(
                        "INV-7: gap or overlap before `{}` on magnetic `{}`",
                        i.id, t.name
                    ));
                }
                end = i.end();
            }
        }
    }
    if tracks.iter().filter(|t| t.magnetic).count() > 1 {
        errors.push("INV-6: more than one magnetic track".into());
    }
    errors.extend(group_errors(&by_id));
    errors.extend(attach_errors(&by_id));
    errors
}

fn item_errors(t: &Track, i: &Item, media: &BTreeMap<String, Media>) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(m) = media.get(&i.media) else {
        return vec![format!("item `{}` uses unknown media `{}`", i.id, i.media)];
    };
    let accepts = match t.kind.as_str() {
        "video" => m.kind == "video" || m.kind == "image",
        "audio" => m.kind == "audio" || (m.kind == "video" && m.audio),
        _ => false,
    };
    if !accepts {
        errors.push(format!(
            "INV-5: `{}` ({} media) cannot sit on {} track `{}`",
            i.id, m.kind, t.kind, t.name
        ));
    }
    if let Some(frames) = m.frames.filter(|_| m.kind != "image")
        && i.source + i.dur > frames
    {
        errors.push(format!(
            "INV-4: `{}` reads past the end of `{}` ({} + {} > {frames})",
            i.id, i.media, i.source, i.dur
        ));
    }
    errors
}

fn group_errors(by_id: &BTreeMap<&str, (&Track, &Item)>) -> Vec<String> {
    let mut groups: BTreeMap<&str, Vec<&Item>> = BTreeMap::new();
    for (_, i) in by_id.values() {
        if let Some(g) = &i.group {
            groups.entry(g.as_str()).or_default().push(i);
        }
    }
    let mut errors = Vec::new();
    for (g, members) in groups {
        if members.len() < 2 {
            errors.push(format!("INV-8: group `{g}` has a single member"));
        }
        if members
            .iter()
            .any(|m| m.start != members[0].start || m.dur != members[0].dur)
        {
            errors.push(format!("INV-8: group `{g}` is not aligned"));
        }
    }
    errors
}

fn attach_errors(by_id: &BTreeMap<&str, (&Track, &Item)>) -> Vec<String> {
    let mut errors = Vec::new();
    for (track, child) in by_id.values() {
        let Some(p) = &child.parent else { continue };
        let c = &child.id;
        let Some((ptrack, parent)) = by_id.get(p.as_str()) else {
            errors.push(format!("INV-9: `{c}` is attached to unknown `{p}`"));
            continue;
        };
        if parent.parent.is_some() {
            errors.push(format!("INV-9: parent `{p}` is itself attached"));
        }
        if by_id.values().any(|(_, x)| x.parent.as_ref() == Some(c)) {
            errors.push(format!("INV-9: child `{c}` is also a parent"));
        }
        if ptrack.name == track.name {
            errors.push(format!("INV-9: `{c}` is on its parent's track"));
        }
        if child.group.is_some() && child.group == parent.group {
            errors.push(format!("INV-9: `{c}` is in its parent's group"));
        }
        if child.start < parent.start || child.start >= parent.end() {
            errors.push(format!("INV-9: `{c}` starts outside its parent `{p}`"));
        }
        if let Some(g) = &child.group {
            for (_, m) in by_id.values() {
                if m.group.as_ref() == Some(g) && m.parent.as_ref() != Some(p) {
                    errors.push(format!(
                        "INV-9: `{}` is linked to `{c}` but not attached to `{p}`",
                        m.id
                    ));
                }
            }
        }
    }
    errors
}

fn items_of(tracks: &[Track]) -> BTreeMap<&str, (&str, &Item)> {
    tracks
        .iter()
        .flat_map(|t| {
            t.items
                .iter()
                .map(move |i| (i.id.as_str(), (t.name.as_str(), i)))
        })
        .collect()
}

fn groups_of(tracks: &[Track]) -> BTreeSet<&str> {
    tracks
        .iter()
        .flat_map(|t| t.items.iter().filter_map(|i| i.group.as_deref()))
        .collect()
}

fn transition_errors(command: &str, before: &[Track], after: &[Track]) -> Vec<String> {
    let mut errors = Vec::new();
    let after_names: Vec<&str> = after.iter().map(|t| t.name.as_str()).collect();
    let kept: Vec<&str> = after_names
        .iter()
        .copied()
        .filter(|n| !n.starts_with('+'))
        .collect();
    let before_names: Vec<&str> = before.iter().map(|t| t.name.as_str()).collect();
    if kept != before_names {
        errors.push(format!(
            "after keeps tracks {kept:?}; edits never remove or reorder tracks ({before_names:?})"
        ));
    }
    for t in before {
        let Some(a) = after.iter().find(|a| a.name == t.name) else {
            continue;
        };
        if a.kind != t.kind || a.locked != t.locked {
            errors.push(format!("track `{}` changed kind or lock", t.name));
        }
        if a.magnetic != t.magnetic && command != "track.set_magnetic" {
            errors.push(format!("track `{}` changed its magnetic flag", t.name));
        }
        if t.locked {
            let mut x = t.items.clone();
            let mut y = a.items.clone();
            x.sort_by(|p, q| p.id.cmp(&q.id));
            y.sort_by(|p, q| p.id.cmp(&q.id));
            if x != y {
                errors.push(format!("INV-28: locked track `{}` changed", t.name));
            }
        }
    }
    let (b, a) = (items_of(before), items_of(after));
    for (id, (_, item)) in &a {
        match b.get(id) {
            None if !id.starts_with('+') => {
                errors.push(format!("new item `{id}` must be named `+…`"));
            }
            Some((_, old)) if old.media != item.media && command != "clip.replace" => {
                errors.push(format!("`{id}` changed media; only clip.replace does"));
            }
            _ => {}
        }
    }
    let old_groups = groups_of(before);
    for g in groups_of(after) {
        if !old_groups.contains(g) && !g.starts_with('+') {
            errors.push(format!("new group `{g}` must be named `+…`"));
        }
    }
    errors
}

// ----- the EditReport of spec INV-21 ---------------------------------------------------------

type Relink = (String, Option<String>, Option<String>);

#[derive(Debug, Default, PartialEq, Eq)]
struct Report {
    created: BTreeSet<String>,
    removed: BTreeSet<String>,
    moved: BTreeSet<(String, String, i64)>,
    trimmed: BTreeSet<(String, i64, i64, i64)>,
    relinked: BTreeSet<Relink>,
    replaced: BTreeSet<(String, String)>,
    toggled: BTreeSet<(String, bool)>,
    created_tracks: BTreeSet<String>,
}

/// The report INV-21 derives from `before` and `after`, given the split right halves.
fn derive_report(before: &[Track], after: &[Track], rights: &BTreeSet<String>) -> Report {
    let (b, a) = (items_of(before), items_of(after));
    let mut r = Report::default();
    for (id, (track, item)) in &a {
        let Some((old_track, old)) = b.get(id) else {
            if !rights.contains(*id) {
                r.created.insert((*id).to_owned());
            }
            continue;
        };
        let id = (*id).to_owned();
        let extent = old.dur != item.dur || old.source != item.source;
        if extent {
            r.trimmed
                .insert((id.clone(), item.start, item.dur, item.source));
        } else if old_track != track || old.start != item.start {
            r.moved
                .insert((id.clone(), (*track).to_owned(), item.start));
        }
        if old.group != item.group || old.parent != item.parent {
            r.relinked
                .insert((id.clone(), item.group.clone(), item.parent.clone()));
        }
        if old.enabled != item.enabled {
            r.toggled.insert((id.clone(), item.enabled));
        }
        if old.media != item.media {
            r.replaced.insert((id, item.media.clone()));
        }
    }
    r.removed = b
        .keys()
        .filter(|id| !a.contains_key(*id))
        .map(|id| (*id).to_owned())
        .collect();
    let known_tracks: BTreeSet<&str> = before.iter().map(|t| t.name.as_str()).collect();
    r.created_tracks = after
        .iter()
        .filter(|t| !known_tracks.contains(t.name.as_str()))
        .map(|t| t.name.clone())
        .collect();
    r
}

fn stated_report(v: &Value) -> Report {
    let list = |key: &str| v[key].as_array().cloned().unwrap_or_default();
    let s = |x: &Value, key: &str| x[key].as_str().unwrap_or_default().to_owned();
    let n = |x: &Value, key: &str| x[key].as_i64().unwrap_or_default();
    Report {
        created: strings(&v["created"]).into_iter().collect(),
        removed: strings(&v["removed"]).into_iter().collect(),
        moved: list("moved")
            .iter()
            .map(|x| (s(x, "item"), s(x, "track"), n(x, "start")))
            .collect(),
        trimmed: list("trimmed")
            .iter()
            .map(|x| (s(x, "item"), n(x, "start"), n(x, "dur"), n(x, "in")))
            .collect(),
        relinked: list("relinked")
            .iter()
            .map(|x| (s(x, "item"), opt_str(&x["group"]), opt_str(&x["parent"])))
            .collect(),
        replaced: list("replaced")
            .iter()
            .map(|x| (s(x, "item"), s(x, "media")))
            .collect(),
        toggled: list("toggled")
            .iter()
            .map(|x| (s(x, "item"), x["enabled"].as_bool().unwrap_or(true)))
            .collect(),
        created_tracks: strings(&v["created_tracks"]).into_iter().collect(),
    }
}

fn report_errors(v: &Value, before: &[Track], after: &[Track]) -> Vec<String> {
    let mut errors = Vec::new();
    let (b, a) = (items_of(before), items_of(after));
    let mut rights = BTreeSet::new();
    for split in v["split"].as_array().into_iter().flatten() {
        let left = split["left"].as_str().unwrap_or_default();
        let right = split["right"].as_str().unwrap_or_default();
        if !b.contains_key(left) {
            errors.push(format!("report.split: left `{left}` is not in `before`"));
        }
        if b.contains_key(right) || !a.contains_key(right) {
            errors.push(format!(
                "report.split: right `{right}` must be new in `after`"
            ));
        }
        rights.insert(right.to_owned());
    }
    let derived = derive_report(before, after, &rights);
    let stated = stated_report(v);
    if derived != stated {
        errors.push(format!(
            "INV-21: the stated report differs from before/after: stated {stated:?}, derived {derived:?}"
        ));
    }
    errors
}

// ----- snapping (spec §6) --------------------------------------------------------------------

fn frame_flicks(fps: i64) -> Result<i64, String> {
    if fps > 0 && FLICKS_PER_SECOND % fps == 0 {
        Ok(FLICKS_PER_SECOND / fps)
    } else {
        Err(format!("fps {fps} is not exact in flicks"))
    }
}

/// Nearest frame boundary, ties later (P1 `Rounding::Nearest`).
fn snap_to_frame(t: i64, period: i64) -> i64 {
    let (t, p) = (i128::from(t), i128::from(period));
    let frame = (2 * t + p).div_euclid(2 * p);
    i64::try_from(frame * p).unwrap_or(i64::MAX)
}

fn snap_rank(kind: &str) -> u8 {
    match kind {
        "playhead" => 0,
        "edge" => 1,
        "marker" => 2,
        "beat" => 3,
        _ => 4,
    }
}

/// The reference snap of spec §6: `(t, target)`.
#[allow(clippy::cast_precision_loss)] // flicks distances compared with an f64 tolerance, as the spec says
fn snap_reference(fx: &Value) -> Result<(i64, Option<(String, i64)>), String> {
    let period = frame_flicks(fx["fps"].as_i64().unwrap_or_default())?;
    let pps = fx["px_per_second"].as_f64().unwrap_or(1.0);
    let tolerance =
        fx["threshold_px"].as_f64().unwrap_or_default() * FLICKS_PER_SECOND as f64 / pps;
    let r = fx["r"].as_i64().unwrap_or_default();
    let enabled = fx.get("enabled").map(strings);
    let mut best: Option<((i64, u8, usize, i64), String)> = None;
    for (i, o) in fx["offsets"].as_array().into_iter().flatten().enumerate() {
        let o = o.as_i64().unwrap_or_default();
        for c in fx["candidates"].as_array().into_iter().flatten() {
            let kind = c["kind"].as_str().unwrap_or_default();
            if enabled
                .as_ref()
                .is_some_and(|e| !e.iter().any(|k| k == kind))
            {
                continue;
            }
            let t = snap_to_frame(c["t"].as_i64().unwrap_or_default(), period);
            let d = (t - (r + o)).abs();
            if d as f64 > tolerance || t - o < 0 {
                continue;
            }
            let key = (d, snap_rank(kind), i, t);
            if best.as_ref().is_none_or(|(k, _)| key < *k) {
                best = Some((key, kind.to_owned()));
            }
        }
    }
    let offsets = fx["offsets"].as_array().cloned().unwrap_or_default();
    Ok(match best {
        Some(((_, _, i, c), kind)) => {
            let o = offsets.get(i).and_then(Value::as_i64).unwrap_or_default();
            (c - o, Some((kind, c)))
        }
        None => (snap_to_frame(r, period), None),
    })
}

fn check_snap(fx: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    for o in fx["offsets"].as_array().into_iter().flatten() {
        if let (Ok(p), Some(o)) = (
            frame_flicks(fx["fps"].as_i64().unwrap_or_default()),
            o.as_i64(),
        ) && o % p != 0
        {
            errors.push(format!("offset {o} is not a whole number of frames"));
        }
    }
    match snap_reference(fx) {
        Ok((t, target)) => {
            let expected = &fx["expected"];
            let want_t = expected["t"].as_i64().unwrap_or_default();
            let want_target = expected["target"].as_object().map(|o| {
                (
                    o.get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    o.get("t").and_then(Value::as_i64).unwrap_or_default(),
                )
            });
            if (t, &target) != (want_t, &want_target) {
                errors.push(format!(
                    "expected ({want_t}, {want_target:?}), the reference snap gives ({t}, {target:?})"
                ));
            }
        }
        Err(e) => errors.push(e),
    }
    errors
}

// ----- three-point placement (spec §4.18) ----------------------------------------------------

/// `(at, source_in, duration, shortened)` or the error code.
fn three_point_reference(fx: &Value) -> Result<(i64, i64, i64, bool), &'static str> {
    let frames = |k: &str| fx[k].as_i64();
    let fps = fx["fps"].as_i64().unwrap_or_default();
    let media = &fx["media"];
    let length = media["frames"]
        .as_i64()
        .filter(|_| media["kind"] != "image");
    let (src_in, src_out) = (frames("source_in"), frames("source_out"));
    if let (Some(i), Some(o)) = (src_in, src_out)
        && o <= i
    {
        return Err("invalid");
    }
    let timeline = match (frames("timeline_in"), frames("timeline_out")) {
        (Some(i), Some(o)) if o > i => Some((i, o)),
        (None, None) => None,
        _ => return Err("invalid"),
    };
    let source_in = src_in.unwrap_or(0);
    let to_end = |s: i64| length.map_or(5 * fps, |len| len - s);
    let at = timeline.map_or(frames("playhead").unwrap_or_default(), |(i, _)| i);
    let mut duration = if src_in.is_some() || src_out.is_some() {
        src_out.map_or_else(|| to_end(source_in), |o| o - source_in)
    } else if let Some((i, o)) = timeline {
        o - i
    } else {
        to_end(source_in)
    };
    let mut shortened = false;
    if let Some(len) = length
        && source_in + duration > len
    {
        duration = len - source_in;
        shortened = true;
    }
    if duration < 1 {
        return Err("invalid");
    }
    Ok((at, source_in, duration, shortened))
}

fn check_three_point(fx: &Value) -> Vec<String> {
    let got = three_point_reference(fx);
    let want = if let Some(code) = fx["error"]["code"].as_str() {
        Err(code)
    } else {
        let e = &fx["expected"];
        let n = |k: &str| e[k].as_i64().unwrap_or_default();
        Ok((
            n("at"),
            n("source_in"),
            n("duration"),
            e["shortened"].as_bool().unwrap_or(false),
        ))
    };
    if got == want {
        Vec::new()
    } else {
        vec![format!(
            "expected {want:?}, the reference placement gives {got:?}"
        )]
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn committed_fixtures_pass() {
        match check(&crate::workspace_root()) {
            Ok(s) => assert!(s.edit >= MIN_EDIT_FIXTURES, "{s:?}"),
            Err(errors) => panic!("check-fixtures failed:\n{}", errors.join("\n")),
        }
    }

    #[test]
    fn invs_are_read_from_bold_ids_only() {
        let invs = defined_invs("- **INV-3** a rule. See INV-4 and **INV-12** and **INV-x**.");
        assert_eq!(invs, ["INV-12", "INV-3"].map(String::from).into());
    }

    #[test]
    fn the_schema_subset_reports_problems() {
        let root = json!({
            "$defs": { "n": { "type": "integer", "minimum": 1 } },
            "type": "object",
            "required": ["a"],
            "additionalProperties": false,
            "properties": { "a": { "$ref": "#/$defs/n" }, "b": { "enum": ["x"] } },
            "oneOf": [{ "required": ["a"] }, { "required": ["b"] }]
        });
        let mut e = Vec::new();
        validate(&root, &root, &json!({"a": 2}), "", &mut e);
        assert!(e.is_empty(), "{e:?}");
        validate(&root, &root, &json!({"a": 0, "b": "y", "c": 1}), "", &mut e);
        assert_eq!(e.len(), 4, "{e:?}"); // minimum, enum, unexpected, oneOf (both match)
        let mut e = Vec::new();
        validate(&root, &json!({"pattern": "x"}), &json!("x"), "", &mut e);
        assert_eq!(e, ["schema keyword `pattern` is not supported"]);
    }

    fn track(name: &str, magnetic: bool, items: &[(&str, i64, i64)]) -> Track {
        Track {
            name: name.into(),
            kind: "video".into(),
            magnetic,
            locked: false,
            items: items
                .iter()
                .map(|&(id, start, dur)| Item {
                    id: id.into(),
                    media: "m".into(),
                    start,
                    dur,
                    source: 0,
                    group: None,
                    parent: None,
                    enabled: true,
                })
                .collect(),
        }
    }

    #[test]
    fn document_checks_catch_overlaps_and_gaps() {
        let media = BTreeMap::from([(
            "m".to_owned(),
            Media {
                kind: "video".into(),
                frames: Some(100),
                audio: false,
            },
        )]);
        let ok = [track("V1", true, &[("A", 0, 10), ("B", 10, 10)])];
        assert_eq!(doc_errors(&ok, &media), Vec::<String>::new());
        let gap = [track("V1", true, &[("A", 0, 10), ("B", 15, 10)])];
        assert!(doc_errors(&gap, &media)[0].starts_with("INV-7"));
        let overlap = [track("V2", false, &[("A", 0, 10), ("B", 5, 10)])];
        assert!(doc_errors(&overlap, &media)[0].starts_with("INV-1"));
        let past_end = [track("V2", false, &[("A", 0, 105)])];
        assert!(doc_errors(&past_end, &media)[0].starts_with("INV-4"));
    }

    #[test]
    fn the_report_is_derived_from_the_diff() {
        let before = [track("V1", true, &[("A", 0, 10), ("B", 10, 10)])];
        let after = [track("V1", true, &[("B", 0, 10)])];
        let r = derive_report(&before, &after, &BTreeSet::new());
        assert_eq!(r.removed, ["A".to_owned()].into());
        assert_eq!(r.moved, [("B".to_owned(), "V1".to_owned(), 0)].into());
        assert_eq!((r.trimmed.len(), r.created.len()), (0, 0));
    }

    #[test]
    fn snapping_prefers_the_nearest_then_the_rank() {
        let f = 28_224_000; // one frame at 25 fps
        let fx = |r: i64| {
            json!({
                "fps": 25, "px_per_second": 100, "threshold_px": 8, "offsets": [0], "r": r,
                "candidates": [{"kind": "edge", "t": 10 * f}, {"kind": "playhead", "t": 12 * f}]
            })
        };
        assert_eq!(
            snap_reference(&fx(11 * f)),
            Ok((12 * f, Some(("playhead".into(), 12 * f))))
        );
        assert_eq!(
            snap_reference(&fx(10 * f + 1)),
            Ok((10 * f, Some(("edge".into(), 10 * f))))
        );
        assert_eq!(snap_reference(&fx(20 * f + f / 2)), Ok((21 * f, None)));
    }

    #[test]
    fn three_point_timeline_in_wins() {
        let fx = json!({
            "fps": 25, "media": {"kind": "video", "frames": 250}, "playhead": 7,
            "timeline_in": 100, "timeline_out": 110, "source_in": 50, "source_out": 80
        });
        assert_eq!(three_point_reference(&fx), Ok((100, 50, 30, false)));
    }
}
