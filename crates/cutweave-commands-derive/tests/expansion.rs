// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.02: what `#[command]`, `#[query]` and `#[wire]` generate, checked through the registry.

#![allow(clippy::unwrap_used)]

use cutweave_commands::{
    Access, CmdError, Command, EngineCommand, EntryKind, McpExposure, Query, QueryCtx, REDACTED,
    Registry, Scope, Transaction, Tx, command, query, wire,
};
use cutweave_model::{Project, SeededGen, SequenceId, Time, TimeInput};
use serde_json::json;

/// Sets an AI provider key (stored in the OS keychain).
#[command(
    name = "ai.set_key",
    summary = "Store an API key for a provider",
    undoable = false,
    mcp = "catalog",
    scope = "ui",
    annotations(destructive = false, idempotent = true)
)]
pub struct SetKey {
    /// Provider id.
    pub provider: String,
    /// The key itself.
    #[secret]
    pub key: String,
    /// Optional label.
    #[secret]
    #[serde(rename = "backupKey")]
    pub backup_key: Option<String>,
}

/// Empty result.
#[wire]
#[derive(Default)]
pub struct Done {}

impl Command for SetKey {
    type Output = Done;
    fn apply(&self, _: &mut Tx) -> Result<Done, CmdError> {
        Ok(Done {})
    }
}

/// Lists sequences.
#[query(name = "sequence.list", summary = "List sequences", mcp = "core")]
pub struct ListSequences {
    /// At most this many.
    pub limit: Option<u32>,
    /// Name filter (an explicit `default` is not duplicated by the macro).
    #[serde(default)]
    pub name: Option<String>,
}

/// One sequence.
#[wire]
pub struct SequenceSummary {
    /// Its ID.
    pub id: SequenceId,
    /// Its name.
    pub name: String,
}

/// The sequences.
#[wire]
pub struct SequenceList {
    /// In ID order.
    pub sequences: Vec<SequenceSummary>,
}

impl Query for ListSequences {
    type Output = SequenceList;
    fn run(&self, ctx: &QueryCtx) -> Result<SequenceList, CmdError> {
        let limit = self.limit.map_or(usize::MAX, |n| n as usize);
        Ok(SequenceList {
            sequences: ctx
                .doc()
                .sequences
                .values()
                .take(limit)
                .map(|s| SequenceSummary {
                    id: s.id,
                    name: s.name.clone(),
                })
                .collect(),
        })
    }
}

/// UI view state (stand-in for WP-1C.04).
#[derive(Debug, Default)]
pub struct ViewState {
    playhead: Time,
}

/// Moves the playhead.
#[command(
    name = "view.set_playhead",
    summary = "Move the playhead",
    view,
    mcp = "catalog"
)]
pub struct SetPlayhead {
    /// Where.
    pub at: TimeInput,
}

impl EngineCommand for SetPlayhead {
    type Context = ViewState;
    type Output = Done;
    fn run(&self, ctx: &mut ViewState) -> Result<Done, CmdError> {
        ctx.playhead = self
            .at
            .resolve(None)
            .map_err(|e| CmdError::invalid("/at", e.to_string()))?;
        Ok(Done {})
    }
}

/// History stand-in (WP-1B.04).
#[derive(Debug, Default)]
pub struct History {
    undone: u32,
}

/// Undoes the caller's last transaction.
#[command(
    name = "edit.undo",
    summary = "Undo your last change",
    engine,
    undoable = false,
    mcp = "core",
    annotations(destructive = false)
)]
pub struct Undo {}

impl EngineCommand for Undo {
    type Context = History;
    type Output = Done;
    fn run(&self, ctx: &mut History) -> Result<Done, CmdError> {
        ctx.undone += 1;
        Ok(Done {})
    }
}

/// Forgets the history (engine state nothing replays, so never journaled).
#[command(
    name = "edit.forget",
    summary = "Forget the history",
    engine,
    undoable = false,
    journaled = false,
    mcp = "hidden",
    annotations(destructive = false)
)]
pub struct Forget {}

impl EngineCommand for Forget {
    type Context = History;
    type Output = Done;
    fn run(&self, ctx: &mut History) -> Result<Done, CmdError> {
        ctx.undone = 0;
        Ok(Done {})
    }
}

/// A tagged wire enum.
#[wire]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    /// The start.
    Start,
    /// The end.
    End,
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    registry
        .register::<SetKey>()
        .unwrap()
        .register::<ListSequences>()
        .unwrap()
        .register::<SetPlayhead>()
        .unwrap()
        .register::<Undo>()
        .unwrap()
        .register::<Forget>()
        .unwrap();
    registry
}

#[test]
fn specs_follow_the_attributes() {
    let registry = registry();
    let key = &registry.get("ai.set_key").unwrap().meta;
    assert_eq!(
        (key.kind, key.scope, key.mcp, key.undoable),
        (
            EntryKind::Command,
            Scope::UiOnly,
            McpExposure::Catalog,
            false
        )
    );
    assert!(key.annotations.idempotent && !key.annotations.destructive);
    assert_eq!(key.secret_fields, ["key", "backupKey"]);
    assert_eq!(key.rust_type, "SetKey");

    let list = &registry.get("sequence.list").unwrap().meta;
    assert_eq!(list.kind, EntryKind::Query);
    assert!(list.annotations.read_only && list.annotations.idempotent);
    assert!(!list.annotations.destructive && !list.undoable);

    let view = &registry.get("view.set_playhead").unwrap().meta;
    assert_eq!(view.kind, EntryKind::Engine);
    assert!(view.view && !view.undoable && !view.is_journaled());

    let undo = &registry.get("edit.undo").unwrap().meta;
    assert_eq!(undo.kind, EntryKind::Engine);
    assert!(!undo.view && undo.is_journaled() && undo.journaled);

    let forget = &registry.get("edit.forget").unwrap().meta;
    assert_eq!(forget.kind, EntryKind::Engine);
    assert!(!forget.journaled && !forget.is_journaled());
}

// The macro emits the opt-out on the params type itself.
const _: () = assert!(!<Forget as cutweave_commands::Described>::JOURNALED);

#[test]
fn schemas_carry_doc_comments_optionality_and_secrets() {
    let registry = registry();
    let key = &registry.get("ai.set_key").unwrap().meta.input_schema;
    assert_eq!(
        key["description"],
        "Sets an AI provider key (stored in the OS keychain)."
    );
    assert_eq!(key["properties"]["provider"]["description"], "Provider id.");
    assert_eq!(key["additionalProperties"], false);
    assert_eq!(key["required"], json!(["provider", "key"]));
    assert_eq!(key["properties"]["key"]["writeOnly"], true);
    assert_eq!(key["properties"]["backupKey"]["writeOnly"], true);
    let list = &registry.get("sequence.list").unwrap().meta;
    assert!(list.input_schema.get("required").is_none());
    assert_eq!(
        list.output_schema["properties"]["sequences"]["items"]["properties"]["name"]["description"],
        "Its name."
    );
}

#[test]
fn secrets_never_reach_debug_output_or_redacted_params() {
    let cmd = SetKey {
        provider: "openai".into(),
        key: "sk-live-0123456789".into(),
        backup_key: Some("sk-live-backup".into()),
    };
    let debug = format!("{cmd:?}");
    assert!(!debug.contains("sk-live"), "{debug}");
    assert!(
        debug.contains("openai") && debug.contains(REDACTED),
        "{debug}"
    );
    let registry = registry();
    let params = json!({"provider": "openai", "key": "sk-live-0123456789", "backupKey": "b"});
    let redacted = registry.get("ai.set_key").unwrap().redact(&params);
    assert_eq!(
        redacted,
        json!({"provider": "openai", "key": REDACTED, "backupKey": REDACTED})
    );
}

#[test]
fn generated_entries_run_through_the_registry() {
    let registry = registry();
    let doc = Project::new("P");
    let mut ids = SeededGen::new(3);
    let mut txn = Transaction::begin(doc.clone());
    // UI-only, unknown fields refused at their pointer, missing Option params read as None.
    let params = json!({"provider": "x", "key": "k"});
    assert_eq!(
        registry
            .invoke_json(
                "ai.set_key",
                params.clone(),
                &mut txn.tx(&mut ids, Access::STANDARD)
            )
            .unwrap_err()
            .code(),
        "forbidden"
    );
    assert_eq!(
        registry
            .invoke_json("ai.set_key", params, &mut txn.tx(&mut ids, Access::UI))
            .unwrap(),
        json!({})
    );
    let err = registry
        .invoke_json(
            "ai.set_key",
            json!({"provider": "x", "key": "k", "typo": 1}),
            &mut txn.tx(&mut ids, Access::UI),
        )
        .unwrap_err();
    assert!(
        matches!(&err, CmdError::Invalid { path, .. } if path == "/typo"),
        "{err:?}"
    );

    let ctx = QueryCtx::new(&doc, Access::STANDARD);
    assert_eq!(
        registry
            .query_json("sequence.list", json!({}), &ctx)
            .unwrap(),
        json!({"sequences": []})
    );

    let mut view = ViewState::default();
    registry
        .invoke_engine_json(
            "view.set_playhead",
            json!({"at": "1.5s"}),
            &mut view,
            Access::STANDARD,
        )
        .unwrap();
    assert_eq!(view.playhead.flicks(), 1_058_400_000);
    let mut history = History::default();
    registry
        .invoke_engine_json("edit.undo", json!({}), &mut history, Access::STANDARD)
        .unwrap();
    assert_eq!(history.undone, 1);
}

#[test]
fn wire_types_round_trip() {
    let out = SequenceList {
        sequences: vec![SequenceSummary {
            id: SequenceId::from_u128(1),
            name: "Main".into(),
        }],
    };
    let v = serde_json::to_value(&out).unwrap();
    assert_eq!(serde_json::from_value::<SequenceList>(v).unwrap(), out);
    assert!(serde_json::from_value::<SequenceList>(json!({"sequences": [], "x": 1})).is_err());
    assert_eq!(serde_json::to_value(Edge::End).unwrap(), json!("end"));
    assert_eq!(format!("{:?}", Edge::Start), "Start");
    assert_eq!(Done::default(), Done {});
}

#[test]
fn generated_types_export_to_typescript() {
    let registry = registry();
    let mut types = specta::Types::default();
    for entry in registry.list() {
        let _ = entry.ts_types(&mut types);
    }
    let ts = specta_typescript::Typescript::default()
        .export(&types, specta_serde::Format)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(ts.contains("export type SetKey"), "{ts}");
    assert!(ts.contains("backupKey?: string | null"), "{ts}");
    assert!(ts.contains("export type SequenceList"), "{ts}");
}
