// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.01: registration checks the contract and rejects duplicates; invocation is typed,
//! scope-checked and field-precise.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{CountItems, Done, SetKey, SetPlayhead, SplitClip, ViewState, fixture, secs};
use cutweave_commands::{
    Access, Annotations, CmdError, Command, CommandSpec, Described, EntryKind, McpExposure,
    QueryCtx, REDACTED, Registry, RegistryError, ResourceKind, Rev, Scope, Transaction, Tx,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

fn registry() -> Registry {
    let mut registry = Registry::new();
    registry
        .add_command::<SplitClip>()
        .unwrap()
        .add_query::<CountItems>()
        .unwrap()
        .add_engine_command::<SetPlayhead>()
        .unwrap()
        .add_command::<SetKey>()
        .unwrap();
    registry
}

/// A command whose spec is set per test through a const generic index into `SPECS`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(deny_unknown_fields)]
struct Probe<const N: usize> {
    #[serde(default)]
    note: Option<String>,
}

const BASE: CommandSpec = CommandSpec {
    name: "clip.probe",
    summary: "Probe",
    kind: EntryKind::Command,
    undoable: true,
    view: false,
    scope: Scope::Any,
    mcp: McpExposure::Catalog,
    annotations: Annotations {
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: false,
    },
    secret_fields: &[],
    rust_type: "Probe",
};

const SPECS: &[CommandSpec] = &[
    BASE,
    CommandSpec {
        name: "Clip.probe",
        ..BASE
    },
    CommandSpec {
        name: "clip.probe2",
        ..BASE
    },
    CommandSpec {
        name: "clip-probe",
        ..BASE
    },
    CommandSpec {
        name: "widget.probe",
        ..BASE
    },
    CommandSpec {
        summary: "",
        ..BASE
    },
    CommandSpec {
        annotations: Annotations {
            read_only: true,
            ..BASE.annotations
        },
        ..BASE
    },
    CommandSpec {
        scope: Scope::UiOnly,
        mcp: McpExposure::Core,
        ..BASE
    },
    CommandSpec {
        name: "view.probe",
        ..BASE
    },
    CommandSpec {
        secret_fields: &["missing"],
        ..BASE
    },
    CommandSpec {
        kind: EntryKind::Query,
        ..BASE
    },
];

impl<const N: usize> Described for Probe<N> {
    const SPEC: CommandSpec = SPECS[N];
}

impl<const N: usize> Command for Probe<N> {
    type Output = Done;
    fn apply(&self, _: &mut Tx<'_>) -> Result<Done, CmdError> {
        Ok(Done {})
    }
}

#[test]
fn duplicate_names_are_rejected_at_startup() {
    let mut registry = registry();
    assert_eq!(
        registry.add_command::<SplitClip>().unwrap_err(),
        RegistryError::Duplicate {
            name: "clip.split".into()
        }
    );
    assert_eq!(registry.len(), 4);
}

#[test]
fn names_must_match_the_grammar_and_a_known_namespace() {
    let mut registry = Registry::new();
    registry.add_command::<Probe<0>>().unwrap();
    let invalid = |r: Result<&mut Registry, RegistryError>| match r {
        Err(RegistryError::InvalidName { name, reason }) => (name, reason),
        other => panic!("expected InvalidName, got {other:?}"),
    };
    assert_eq!(invalid(registry.add_command::<Probe<1>>()).0, "Clip.probe");
    assert_eq!(invalid(registry.add_command::<Probe<2>>()).0, "clip.probe2");
    assert_eq!(invalid(registry.add_command::<Probe<3>>()).0, "clip-probe");
    let (_, reason) = invalid(registry.add_command::<Probe<4>>());
    assert!(reason.contains("unknown namespace `widget`"), "{reason}");
    assert_eq!(registry.len(), 1);
}

#[test]
fn inconsistent_metadata_is_rejected() {
    let mut registry = Registry::new();
    assert!(matches!(
        registry.add_command::<Probe<5>>(),
        Err(RegistryError::InvalidSummary { .. })
    ));
    for result in [
        registry.add_command::<Probe<6>>().map(drop),
        registry.add_command::<Probe<7>>().map(drop),
        registry.add_command::<Probe<8>>().map(drop),
        registry.add_command::<Probe<10>>().map(drop),
    ] {
        assert!(
            matches!(result, Err(RegistryError::InconsistentFlags { .. })),
            "{result:?}"
        );
    }
    assert_eq!(
        registry.add_command::<Probe<9>>().unwrap_err(),
        RegistryError::UnknownSecretField {
            name: "clip.probe".into(),
            field: "missing".into()
        }
    );
    assert!(registry.is_empty());
}

/// A command whose output is not an object.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(deny_unknown_fields)]
struct Bare {}

impl Described for Bare {
    const SPEC: CommandSpec = CommandSpec {
        name: "clip.bare",
        ..BASE
    };
}

impl Command for Bare {
    type Output = u32;
    fn apply(&self, _: &mut Tx<'_>) -> Result<u32, CmdError> {
        Ok(1)
    }
}

#[test]
fn schemas_must_be_objects() {
    assert_eq!(
        Registry::new().add_command::<Bare>().unwrap_err(),
        RegistryError::NotAnObject {
            name: "clip.bare".into(),
            which: "output"
        }
    );
}

/// Opts out of the journal: allowed for a non-undoable engine command (`Unjournaled<0>`),
/// refused for an undoable one (`<1>`) and for a document command (`<2>`).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(deny_unknown_fields)]
struct Unjournaled<const N: usize> {}

const UNJOURNALED: &[CommandSpec] = &[
    CommandSpec {
        name: "edit.unjournaled",
        kind: EntryKind::Engine,
        undoable: false,
        ..BASE
    },
    CommandSpec {
        name: "edit.unjournaled",
        kind: EntryKind::Engine,
        ..BASE
    },
    CommandSpec {
        name: "clip.unjournaled",
        undoable: false,
        ..BASE
    },
];

impl<const N: usize> Described for Unjournaled<N> {
    const SPEC: CommandSpec = UNJOURNALED[N];
    const JOURNALED: bool = false;
}

impl<const N: usize> cutweave_commands::EngineCommand for Unjournaled<N> {
    type Context = ViewState;
    type Output = Done;
    fn run(&self, _: &mut ViewState) -> Result<Done, CmdError> {
        Ok(Done {})
    }
}

impl Command for Unjournaled<2> {
    type Output = Done;
    fn apply(&self, _: &mut Tx<'_>) -> Result<Done, CmdError> {
        Ok(Done {})
    }
}

#[test]
fn only_non_undoable_engine_commands_may_opt_out_of_the_journal() {
    let mut opted_out = Registry::new();
    opted_out.add_engine_command::<Unjournaled<0>>().unwrap();
    let meta = &opted_out.get("edit.unjournaled").unwrap().meta;
    assert!(!meta.journaled && !meta.is_journaled());
    assert_eq!(serde_json::to_value(meta).unwrap()["journaled"], false);
    for result in [
        Registry::new()
            .add_engine_command::<Unjournaled<1>>()
            .map(drop),
        Registry::new().add_command::<Unjournaled<2>>().map(drop),
    ] {
        assert!(
            matches!(&result, Err(RegistryError::InconsistentFlags { reason, .. }) if reason.contains("journal")),
            "{result:?}"
        );
    }
    // Everything else keeps the default, which is not serialized.
    let defaults = registry();
    let split = &defaults.get("clip.split").unwrap().meta;
    assert!(split.journaled);
    assert!(
        serde_json::to_value(split)
            .unwrap()
            .get("journaled")
            .is_none()
    );
}

#[test]
fn metadata_and_schemas() {
    let registry = registry();
    let names: Vec<&str> = registry
        .list()
        .map(cutweave_commands::Entry::name)
        .collect();
    assert_eq!(
        names,
        [
            "ai.set_key",
            "clip.split",
            "timeline.count",
            "view.set_playhead"
        ]
    );
    let split = registry.get("clip.split").unwrap();
    assert_eq!(split.kind(), EntryKind::Command);
    assert_eq!(split.meta.mcp_tool_name(), "clip_split");
    assert_eq!(split.meta.namespace(), "clip");
    assert!(split.meta.is_journaled());
    assert_eq!(
        registry.get_by_tool_name("clip_split").unwrap().name(),
        "clip.split"
    );
    assert!(registry.get_by_tool_name("clip").is_none());
    let input = &split.meta.input_schema;
    assert_eq!(input["type"], "object");
    assert_eq!(input["additionalProperties"], false);
    assert_eq!(input["required"], json!(["item", "at"]));
    assert_eq!(input["properties"]["item"]["pattern"], "^itm_[0-9a-f]{32}$");
    // Doc comments become descriptions; TimeInput accepts integers and strings.
    assert_eq!(
        input["properties"]["item"]["description"],
        "The clip to split."
    );
    assert!(input["properties"]["at"]["anyOf"].is_array());
    assert!(!input.to_string().contains("$ref"));
    assert_eq!(split.meta.output_schema["type"], "object");
    // MCP annotations.
    assert_eq!(
        serde_json::to_value(split.meta.annotations).unwrap(),
        json!({"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false})
    );
    let count = registry.get("timeline.count").unwrap();
    assert!(count.meta.annotations.read_only);
    assert!(!count.meta.is_journaled());
    // An `Option` param with `#[serde(default)]` is optional.
    assert!(
        count.meta.input_schema["required"]
            .as_array()
            .is_none_or(Vec::is_empty)
    );
    let view = registry.get("view.set_playhead").unwrap();
    assert!(view.meta.view && !view.meta.undoable && !view.meta.is_journaled());
    // Secret params are writeOnly in the schema.
    let key = registry.get("ai.set_key").unwrap();
    assert_eq!(
        key.meta.input_schema["properties"]["key"]["writeOnly"],
        true
    );
    assert_eq!(key.meta.scope, Scope::UiOnly);
    // The metadata is itself serializable (the catalog, WP-1N).
    let meta = serde_json::to_value(&split.meta).unwrap();
    assert_eq!(meta["name"], "clip.split");
    assert_eq!(meta["mcp"], "core");
    assert_eq!(meta["scope"], "any");
}

#[test]
fn invoke_decodes_validates_and_reports_exact_paths() {
    let registry = registry();
    let (doc, ids, mut gen_) = fixture();
    let mut txn = Transaction::begin(doc.clone());
    let mut tx = txn.tx(&mut gen_, Access::STANDARD);
    let call = |tx: &mut Tx<'_>, params| registry.invoke_json("clip.split", params, tx);

    assert_eq!(
        call(&mut tx, json!({"item": ids.a, "at": 0})).unwrap_err(),
        CmdError::invalid("/at", "must be strictly inside the item")
    );
    let err = call(&mut tx, json!({"item": ids.a, "at": "1s", "bogus": 1})).unwrap_err();
    assert!(
        matches!(&err, CmdError::Invalid { path, .. } if path == "/bogus"),
        "{err:?}"
    );
    assert!(err.message().contains("bogus"), "{err}");
    let err = call(
        &mut tx,
        json!({"item": "med_00000000000000000000000000000001", "at": 1}),
    )
    .unwrap_err();
    assert!(
        matches!(&err, CmdError::Invalid { path, .. } if path == "/item"),
        "{err:?}"
    );
    let err = call(&mut tx, json!({"item": ids.a})).unwrap_err();
    assert!(err.message().contains("missing field `at`"), "{err}");
    let err = call(
        &mut tx,
        json!({"item": ids.a, "at": 9_007_199_254_740_992_i64}),
    )
    .unwrap_err();
    assert!(
        matches!(&err, CmdError::Invalid { path, .. } if path == "/at"),
        "{err:?}"
    );
    let missing = cutweave_model::ItemId::from_u128(5);
    assert_eq!(
        call(&mut tx, json!({"item": missing, "at": 1})).unwrap_err(),
        CmdError::not_found(ResourceKind::Item, missing)
    );
    assert_eq!(
        registry
            .invoke_json("clip.nope", json!({}), &mut tx)
            .unwrap_err(),
        CmdError::not_found(ResourceKind::Command, "clip.nope")
    );
    // Nothing failed left a trace.
    assert!(txn.is_empty());

    // Timecodes resolve against the item's sequence rate (30 fps here).
    let mut tx = txn.tx(&mut gen_, Access::STANDARD);
    let out = call(&mut tx, json!({"item": ids.a, "at": "00:00:01:15"})).unwrap();
    assert_eq!(out["left"], json!(ids.a));
    let right: cutweave_model::ItemId = serde_json::from_value(out["right"].clone()).unwrap();
    assert_eq!(
        tx.item(right).unwrap().item.start().flicks(),
        705_600_000 + 15 * common::FRAME
    );
    // Queries run against the working document inside the transaction.
    assert_eq!(
        registry
            .invoke_json("timeline.count", json!(null), &mut tx)
            .unwrap(),
        json!({"count": 3})
    );
    assert_eq!(
        registry
            .query_json(
                "timeline.count",
                json!({}),
                &QueryCtx::new(&doc, Access::STANDARD).with_rev(Rev(4))
            )
            .unwrap(),
        json!({"count": 2})
    );
}

#[test]
fn dry_runs_validate_without_applying() {
    let registry = registry();
    let (doc, ids, _) = fixture();
    let ctx = QueryCtx::new(&doc, Access::STANDARD);
    assert_eq!(ctx.rev(), None);
    registry
        .validate_json("clip.split", json!({"item": ids.b, "at": "5s"}), &ctx)
        .unwrap();
    assert_eq!(
        registry
            .validate_json("clip.split", json!({"item": ids.b, "at": "4s"}), &ctx)
            .unwrap_err(),
        CmdError::invalid("/at", "must be strictly inside the item")
    );
    registry
        .validate_json("timeline.count", json!({}), &ctx)
        .unwrap();
    assert!(
        registry
            .validate_json("view.set_playhead", json!({"at": "x"}), &ctx)
            .is_err()
    );
}

#[test]
fn ui_only_scope_is_enforced_for_every_path() {
    let registry = registry();
    let (doc, _, mut gen_) = fixture();
    let params = json!({"provider": "openai", "key": "sk-test-not-a-real-key"});
    let mut txn = Transaction::begin(doc.clone());
    let err = registry
        .invoke_json(
            "ai.set_key",
            params.clone(),
            &mut txn.tx(&mut gen_, Access::STANDARD),
        )
        .unwrap_err();
    assert_eq!(err.code(), "forbidden");
    assert!(matches!(
        err,
        CmdError::Forbidden {
            scope: Scope::UiOnly,
            ..
        }
    ));
    let ctx = QueryCtx::new(&doc, Access::STANDARD);
    assert_eq!(
        registry
            .validate_json("ai.set_key", params.clone(), &ctx)
            .unwrap_err()
            .code(),
        "forbidden"
    );
    registry
        .invoke_json(
            "ai.set_key",
            params.clone(),
            &mut txn.tx(&mut gen_, Access::UI),
        )
        .unwrap();
    // Secrets are redacted in what may be journaled or logged.
    let entry = registry.get("ai.set_key").unwrap();
    assert_eq!(
        entry.redact(&params),
        json!({"provider": "openai", "key": REDACTED})
    );
    assert_eq!(entry.redact(&json!({"key": null})), json!({"key": null}));
    assert!(!format!("{:?}", entry.meta).contains("sk-test"));
}

#[test]
fn engine_commands_run_on_engine_state_only() {
    let registry = registry();
    let (doc, _, mut gen_) = fixture();
    let mut view = ViewState::default();
    let out = registry
        .invoke_engine_json(
            "view.set_playhead",
            json!({"at": "2s"}),
            &mut view,
            Access::STANDARD,
        )
        .unwrap();
    assert_eq!(out, json!({}));
    assert_eq!(view.playhead, secs(2));
    // Wrong context type.
    let mut wrong = 0_u32;
    assert_eq!(
        registry
            .invoke_engine_json(
                "view.set_playhead",
                json!({"at": 1}),
                &mut wrong,
                Access::STANDARD
            )
            .unwrap_err()
            .code(),
        "internal"
    );
    // Not part of document transactions, and document commands are not engine commands.
    let mut txn = Transaction::begin(doc.clone());
    assert_eq!(
        registry
            .invoke_json(
                "view.set_playhead",
                json!({"at": 1}),
                &mut txn.tx(&mut gen_, Access::STANDARD)
            )
            .unwrap_err()
            .code(),
        "unsupported"
    );
    assert_eq!(
        registry
            .invoke_engine_json("clip.split", json!({}), &mut view, Access::STANDARD)
            .unwrap_err()
            .code(),
        "unsupported"
    );
    assert_eq!(
        registry
            .query_json("clip.split", json!({}), &QueryCtx::new(&doc, Access::UI))
            .unwrap_err()
            .code(),
        "unsupported"
    );
}

#[test]
fn entries_export_typescript() {
    let registry = registry();
    let mut types = specta::Types::default();
    for entry in registry.list() {
        let _ = entry.ts_types(&mut types);
    }
    // The bus's own wire types export too (errors carry `message`; ops are JSON Patch).
    <CmdError as specta::Type>::definition(&mut types);
    <cutweave_commands::Patch as specta::Type>::definition(&mut types);
    <Rev as specta::Type>::definition(&mut types);
    let ts = specta_typescript::Typescript::default()
        .export(&types, specta_serde::Format)
        .unwrap_or_else(|e| panic!("entries must export to TypeScript: {e}"));
    assert!(ts.contains("export type SplitClip"), "{ts}");
    assert!(ts.contains("export type SplitOutput"), "{ts}");
    assert!(ts.contains("TimeInput"), "{ts}");
    assert!(ts.contains("export type Rev = number"), "{ts}");
    let error = ts
        .split("export type CmdError =")
        .nth(1)
        .unwrap_or_else(|| panic!("{ts}"));
    let error = error.split("export type").next().unwrap_or_default();
    assert!(
        error.contains("\"not_found\"") && error.contains("message: string"),
        "{error}"
    );
    assert!(ts.contains("\"replace\""), "{ts}");
}
