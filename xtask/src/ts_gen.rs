// SPDX-License-Identifier: GPL-3.0-or-later
//! TypeScript output of `cargo xtask gen` (arch §5.3, WP-1N.02): `packages/protocol/src/generated/`.
//!
//! | File | Content |
//! |---|---|
//! | `types.ts` | every wire type (model, command inputs/outputs, bus types), exported by specta |
//! | `commands.ts` | `CommandMap`, `QueryMap`, `RpcMap` and `CmdErrorCode` |
//! | `events.ts` | `EventMap` (event name to payload type) |
//! | `schema-hash.ts` | the `SCHEMA_HASH` the engine reports in `rpc.hello` |
//!
//! Flicks are `number` (ADR-0015); IDs are branded strings (`type ItemId = string & { ... }`),
//! applied here to every `export type XxxId = string` because specta has no brand notion for
//! `#[specta(type = String)]` newtypes. Output is deterministic: specta sorts types by name and
//! the maps follow registry (name) order.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use cutweave_commands::{
    CmdError, CommandMeta, CommandPanicked, DocPatched, DocReset, EntryKind, HistoryChanged, Patch,
    Registry, Rev, TxActivity, TxEnded,
};
use cutweave_model::{
    Duration, EntityKind, FrameRate, GroupId, Invariant, ItemId, MediaId, Project, Rounding,
    SampleRate, SequenceId, Time, TimeInput, TrackId,
};
use serde_json::Value;
use specta::{Format as _, Type, Types, datatype::DataType};
use specta_typescript::{Typescript, primitives};

use crate::schema_gen::Files;

const DIR: &str = "packages/protocol/src/generated";
const HEADER: &str = "// @generated — do not edit; run cargo xtask gen\n";

/// Everything the TS side needs to name, beyond what the registry entries pull in.
fn register_support_types(types: &mut Types) {
    Time::definition(types);
    Duration::definition(types);
    TimeInput::definition(types);
    FrameRate::definition(types);
    SampleRate::definition(types);
    Rounding::definition(types);
    EntityKind::definition(types);
    MediaId::definition(types);
    SequenceId::definition(types);
    TrackId::definition(types);
    ItemId::definition(types);
    GroupId::definition(types);
    Project::definition(types);
    Invariant::definition(types);
    CmdError::definition(types);
    Patch::definition(types);
    Rev::definition(types);
}

/// The payload type of a document event, when it is a specta type (`None` for engine events,
/// whose payload structs live in `cutweave-engine`).
fn doc_event_payload(name: &str, types: &mut Types) -> Option<DataType> {
    Some(match name {
        "DocPatched" => DocPatched::definition(types),
        "DocReset" => DocReset::definition(types),
        "CommandPanicked" => CommandPanicked::definition(types),
        "HistoryChanged" => HistoryChanged::definition(types),
        "TxActivity" => TxActivity::definition(types),
        "TxEnded" => TxEnded::definition(types),
        _ => return None,
    })
}

/// Rewrites `export type FooId = string;` to a branded string.
fn brand_ids(ts: &str) -> String {
    let mut out = String::with_capacity(ts.len() + 512);
    for line in ts.lines() {
        let branded = line
            .strip_prefix("export type ")
            .and_then(|rest| rest.strip_suffix(" = string;"))
            .filter(|name| name.ends_with("Id") && name.chars().all(|c| c.is_ascii_alphanumeric()));
        if let Some(name) = branded {
            let _ = writeln!(
                out,
                "export type {name} = string & {{ readonly __brand: \"{name}\" }};"
            );
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Names of every `export type` in `ts`.
fn exported_names(ts: &str) -> BTreeSet<String> {
    ts.lines()
        .filter_map(|l| l.strip_prefix("export type "))
        .filter_map(|rest| rest.split([' ', '<', '=']).next())
        .map(str::to_owned)
        .collect()
}

/// `import type { ... } from "./types.ts";` for the exported names that `body` mentions.
fn imports(body: &str, exported: &BTreeSet<String>) -> String {
    let code: Vec<&str> = body
        .lines()
        .filter(|l| !l.trim_start().starts_with("/**") && !l.trim_start().starts_with("//"))
        .collect();
    let code = code.join(
        "
",
    );
    let used: BTreeSet<&str> = code
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| exported.contains(*w))
        .collect();
    if used.is_empty() {
        return String::new();
    }
    let list: Vec<_> = used.into_iter().collect();
    format!(
        "import type {{ {} }} from \"./types.ts\";\n\n",
        list.join(", ")
    )
}

/// A JSON/TS string literal (command names contain dots, so keys are always quoted).
fn quote(name: &str) -> String {
    format!("{name:?}")
}

/// Builds `packages/protocol/src/generated/*` in memory.
///
/// # Errors
/// A type specta cannot export.
pub fn generate(
    registry: &Registry,
    events: &Value,
    error_codes: &[String],
    schema_hash: &str,
) -> Result<Files, String> {
    let ts = Typescript::default();
    let mut types = Types::default();
    register_support_types(&mut types);

    // Commands, queries and engine commands in registry (name) order.
    let mut entries: Vec<(&CommandMeta, DataType, DataType)> = Vec::new();
    for entry in registry.list() {
        let (input, output) = entry.ts_types(&mut types);
        entries.push((&entry.meta, input, output));
    }
    let mut payloads: Vec<(String, String, Option<DataType>)> = Vec::new();
    for event in events["events"]
        .as_array()
        .ok_or("events.json has no events")?
    {
        let name = event["name"]
            .as_str()
            .ok_or("event without name")?
            .to_owned();
        let payload = event["payload"]
            .as_str()
            .ok_or("event without payload")?
            .to_owned();
        let dt = doc_event_payload(&payload, &mut types);
        payloads.push((name, payload, dt));
    }

    // Types file (serde mapping applied to the graph, as the high-level exporter does).
    let types_ts = ts
        .clone()
        .header(HEADER)
        .export(&types, specta_serde::Format)
        .map_err(|e| format!("specta export: {e}"))?;
    let types_ts = brand_ids(&types_ts);
    let exported = exported_names(&types_ts);

    // Top-level datatypes get the same serde mapping, then render as references.
    let mapped = specta_serde::Format
        .map_types(&types)
        .map_err(|e| format!("specta format: {e}"))?;
    let render = |dt: &DataType| -> Result<String, String> {
        let dt = specta_serde::Format
            .map_type(&types, dt)
            .map_err(|e| format!("specta format: {e}"))?;
        primitives::reference(&ts, mapped.as_ref(), dt.as_ref())
            .map_err(|e| format!("specta reference: {e}"))
    };

    let mut commands = String::new();
    let mut queries = String::new();
    for (meta, input, output) in &entries {
        let target = if meta.kind == EntryKind::Query {
            &mut queries
        } else {
            &mut commands
        };
        let _ = writeln!(
            target,
            "  {}: {{ input: {}; output: {} }};",
            quote(meta.name),
            render(input)?,
            render(output)?
        );
    }
    let mut body = String::new();
    body.push_str("/** Commands and engine commands: name to input and output. */\n");
    let _ = writeln!(body, "export type CommandMap = {{\n{commands}}};\n");
    body.push_str("/** Read-only queries: name to input and output. */\n");
    let _ = writeln!(body, "export type QueryMap = {{\n{queries}}};\n");
    body.push_str(
        "/** Everything callable over RPC. */\nexport type RpcMap = CommandMap & QueryMap;\n\n",
    );
    body.push_str(
        "/** Machine-readable error codes (`CmdError.code`). */\nexport type CmdErrorCode =\n",
    );
    for code in error_codes {
        let _ = writeln!(body, "  | {}", quote(code));
    }
    body.push_str("  ;\n");
    let commands_ts = format!("{HEADER}{}{body}", imports(&body, &exported));

    let mut events_body = String::new();
    events_body.push_str(
        "/** Event name to payload. Engine events are `unknown` until their payload types are exported. */\nexport type EventMap = {\n",
    );
    for (name, payload, dt) in &payloads {
        if let Some(dt) = dt {
            let _ = writeln!(events_body, "  {}: {};", quote(name), render(dt)?);
        } else {
            let _ = writeln!(
                events_body,
                "  // TODO: `{payload}` is not exported by specta yet (engine event).\n  {}: unknown;",
                quote(name)
            );
        }
    }
    events_body.push_str("};\n");
    let events_ts = format!("{HEADER}{}{events_body}", imports(&events_body, &exported));

    let hash_ts = format!(
        "{HEADER}/** sha256 of `schema/commands.json`; compare with the engine's `rpc.hello`. */\nexport const SCHEMA_HASH = \"{schema_hash}\";\n"
    );

    let mut files = Files::new();
    files.insert(format!("{DIR}/types.ts"), types_ts.into_bytes());
    files.insert(format!("{DIR}/commands.ts"), commands_ts.into_bytes());
    files.insert(format!("{DIR}/events.ts"), events_ts.into_bytes());
    files.insert(format!("{DIR}/schema-hash.ts"), hash_ts.into_bytes());
    Ok(files)
}
