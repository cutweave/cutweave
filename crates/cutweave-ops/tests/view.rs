// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1C.04: the `view` namespace through the registry: contract (schema, annotations, flags),
//! partial-merge and validation semantics, tolerant (de)serialization of `ui-state.json`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]

use cutweave_commands::{Access, CmdError, EntryKind, McpExposure, Registry, Scope};
use cutweave_model::{
    Clip, Duration, FrameRate, IdGen, ItemId, MediaAsset, MediaId, MediaKind, ProbeInfo, Project,
    SampleRate, SeededGen, Sequence, SequenceFormat, SequenceId, Time, Track, TrackId, TrackKind,
};
use cutweave_ops::view::{
    PreviewQuality, SelectionRef, UI_STATE_VERSION, UiState, ViewCtx, ViewPatch,
};
use proptest::prelude::*;
use serde_json::{Value, json};

fn secs(s: i64) -> Time {
    Time::from_secs(s).unwrap()
}

struct Ids {
    media: MediaId,
    sequence: SequenceId,
    track: TrackId,
    a: ItemId,
    b: ItemId,
}

fn fixture() -> (Project, Ids) {
    let mut ids = SeededGen::new(11);
    let mut project = Project::new("Fixture");
    let probe = ProbeInfo {
        duration: Some(Duration::from_secs(60).unwrap()),
        ..ProbeInfo::default()
    };
    let media = MediaAsset::file(ids.mint(), MediaKind::Video, "/media/a.mp4", probe);
    let format = SequenceFormat::new(1920, 1080, FrameRate::FPS_30, SampleRate::HZ_48_000);
    let mut sequence = Sequence::new(ids.mint(), "Main", format);
    let mut track = Track::new(ids.mint(), TrackKind::Video, "V1");
    let a = Clip::new(
        ids.mint(),
        media.id,
        secs(0),
        Duration::from_secs(4).unwrap(),
    );
    let b = Clip::new(
        ids.mint(),
        media.id,
        secs(4),
        Duration::from_secs(6).unwrap(),
    );
    let out = Ids {
        media: media.id,
        sequence: sequence.id,
        track: track.id,
        a: a.id,
        b: b.id,
    };
    track.items.insert(a.into());
    track.items.insert(b.into());
    sequence.insert_track(0, track);
    project.media.insert(media.id, media);
    project.sequences.insert(sequence.id, sequence);
    project.validate().unwrap();
    (project, out)
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    cutweave_ops::register(&mut registry).unwrap();
    registry
}

fn call(
    registry: &Registry,
    ctx: &mut ViewCtx,
    name: &str,
    params: Value,
) -> Result<UiState, CmdError> {
    let out = registry.invoke_engine_json(name, params, ctx, Access::STANDARD)?;
    Ok(serde_json::from_value(out).unwrap())
}

fn item(id: ItemId) -> Value {
    json!({"kind": "item", "id": id})
}

#[test]
fn registry_contract() {
    let registry = registry();
    for name in [
        "view.get",
        "view.set",
        "view.set_playhead",
        "view.select",
        "view.set_zoom",
    ] {
        let meta = &registry.get(name).expect(name).meta;
        assert_eq!(meta.kind, EntryKind::Engine, "{name}");
        assert!(
            meta.view && !meta.undoable && !meta.is_journaled(),
            "{name}"
        );
        assert_eq!(meta.mcp, McpExposure::Catalog, "{name}");
        assert_eq!(meta.scope, Scope::Any, "{name}");
        assert!(
            !meta.annotations.destructive && !meta.annotations.open_world,
            "{name}"
        );
        assert_eq!(meta.input_schema["type"], "object", "{name}");
        assert_eq!(meta.input_schema["additionalProperties"], false, "{name}");
    }
    let hints = |n: &str| {
        let a = registry.get(n).unwrap().meta.annotations;
        a.idempotent
    };
    assert!(hints("view.get") && hints("view.set") && hints("view.set_zoom"));
    assert!(!hints("view.select"), "toggle/add are not idempotent");
    assert_eq!(
        registry.get("view.set").unwrap().meta.mcp_tool_name(),
        "view_set"
    );
}

#[test]
fn schema_snapshots() {
    let registry = registry();
    for name in ["view.get", "view.set", "view.select"] {
        let meta = &registry.get(name).unwrap().meta;
        insta::assert_json_snapshot!(
            name.replace('.', "_"),
            json!({
                "meta": serde_json::to_value(meta).unwrap(),
                "input": meta.input_schema,
                "output": meta.output_schema,
            })
        );
    }
}

#[test]
fn defaults_and_get() {
    let (doc, _) = fixture();
    let registry = registry();
    let mut ctx = ViewCtx::new(doc);
    let state = call(&registry, &mut ctx, "view.get", json!({})).unwrap();
    assert_eq!(state, UiState::initial());
    assert_eq!(ctx.rev(), 0);
    assert!(ctx.take_event().is_none());
}

#[test]
fn partial_merge_keeps_untouched_fields() {
    let (doc, ids) = fixture();
    let registry = registry();
    let mut ctx = ViewCtx::new(doc);
    call(
        &registry,
        &mut ctx,
        "view.set",
        json!({"patch": {"zoom": 250.0, "selection": [item(ids.a)]}}),
    )
    .unwrap();
    let s = call(
        &registry,
        &mut ctx,
        "view.set",
        json!({"patch": {"playhead": 705_600_000}}),
    )
    .unwrap();
    assert_eq!(s.playhead, secs(1));
    assert_eq!(s.zoom, 250.0);
    assert_eq!(s.selection, vec![SelectionRef::Item { id: ids.a }]);
    // An empty patch changes nothing and emits no event.
    let rev = ctx.rev();
    ctx.take_event();
    call(&registry, &mut ctx, "view.set", json!({"patch": {}})).unwrap();
    assert_eq!(ctx.rev(), rev);
    assert!(ctx.take_event().is_none());
    // Setting the same value again is not a change either.
    call(&registry, &mut ctx, "view.set_zoom", json!({"zoom": 250.0})).unwrap();
    assert_eq!(ctx.rev(), rev);
    // Opaque panel layout replaces wholesale.
    let s = call(
        &registry,
        &mut ctx,
        "view.set",
        json!({"patch": {"panel_layout": {"a": [1, 2]}, "preview_quality": "half"}}),
    )
    .unwrap();
    assert_eq!(s.panel_layout, json!({"a": [1, 2]}));
    assert_eq!(s.preview_quality, PreviewQuality::Half);
    let ev = ctx.take_event().unwrap();
    assert_eq!(ev.state, s);
    assert!(ctx.take_event().is_none(), "the event is delivered once");
}

#[test]
fn invalid_patch_is_atomic() {
    let (doc, _) = fixture();
    let registry = registry();
    let mut ctx = ViewCtx::new(doc);
    let before = ctx.state().clone();
    for patch in [
        json!({"playhead": 5, "zoom": -1.0}),
        json!({"playhead": 5, "zoom": 1e12}),
        json!({"playhead": -5}),
        json!({"playhead": 5, "master_gain": 9.0}),
        json!({"playhead": 5, "scroll": {"x": -1.0, "y": 0.0}}),
    ] {
        let err = call(&registry, &mut ctx, "view.set", json!({"patch": patch})).unwrap_err();
        assert!(matches!(err, CmdError::Invalid { .. }), "{err:?}");
        assert_eq!(ctx.state(), &before);
        assert_eq!(ctx.rev(), 0);
    }
    let err = call(
        &registry,
        &mut ctx,
        "view.set",
        json!({"patch": {"nope": 1}}),
    )
    .unwrap_err();
    assert!(matches!(err, CmdError::Invalid { .. }));
    match call(
        &registry,
        &mut ctx,
        "view.set",
        json!({"patch": {"zoom": 0.0}}),
    )
    .unwrap_err()
    {
        CmdError::Invalid { path, .. } => assert_eq!(path, "/patch/zoom"),
        e => panic!("{e:?}"),
    }
}

#[test]
fn selection_must_exist_and_is_deduplicated() {
    let (doc, ids) = fixture();
    let other = MediaId::from_u128(0x77);
    let registry = registry();
    let mut ctx = ViewCtx::new(doc);
    let ghost = json!({"kind": "media", "id": other});
    let err = call(
        &registry,
        &mut ctx,
        "view.select",
        json!({"ids": [item(ids.a), ghost]}),
    )
    .unwrap_err();
    assert!(matches!(err, CmdError::NotFound { .. }), "{err:?}");
    assert!(
        ctx.state().selection.is_empty(),
        "a failed select changes nothing"
    );

    let s = call(
        &registry,
        &mut ctx,
        "view.select",
        json!({"ids": [
            item(ids.a), item(ids.a), item(ids.b),
            {"kind": "track", "id": ids.track},
            {"kind": "media", "id": ids.media},
            {"kind": "sequence", "id": ids.sequence},
        ]}),
    )
    .unwrap();
    assert_eq!(s.selection.len(), 5);

    // Modes.
    let s = call(
        &registry,
        &mut ctx,
        "view.select",
        json!({"ids": [item(ids.b)], "mode": "remove"}),
    )
    .unwrap();
    assert_eq!(s.selection.len(), 4);
    let s = call(
        &registry,
        &mut ctx,
        "view.select",
        json!({"ids": [item(ids.b), item(ids.a)], "mode": "toggle"}),
    )
    .unwrap();
    assert!(s.selection.contains(&SelectionRef::Item { id: ids.b }));
    assert!(!s.selection.contains(&SelectionRef::Item { id: ids.a }));
    let s = call(
        &registry,
        &mut ctx,
        "view.select",
        json!({"ids": [item(ids.a)], "mode": "add"}),
    )
    .unwrap();
    assert_eq!(s.selection.last(), Some(&SelectionRef::Item { id: ids.a }));
    let s = call(&registry, &mut ctx, "view.select", json!({"ids": []})).unwrap();
    assert_eq!(s.selection, vec![]);
}

#[test]
fn set_doc_drops_stale_selection() {
    let (doc, ids) = fixture();
    let mut ctx = ViewCtx::new(doc.clone());
    let sel = ViewPatch {
        selection: Some(vec![
            SelectionRef::Item { id: ids.a },
            SelectionRef::Media { id: ids.media },
        ]),
        ..ViewPatch::default()
    };
    ctx.apply_patch(&sel).unwrap();
    ctx.take_event();
    assert!(!ctx.set_doc(&doc));
    let mut smaller = doc;
    smaller.media.remove(&ids.media);
    assert!(ctx.set_doc(&smaller));
    assert_eq!(
        ctx.state().selection,
        vec![SelectionRef::Item { id: ids.a }]
    );
    assert!(ctx.take_event().is_some());
}

#[test]
fn from_json_tolerates_damage_and_keeps_unknown_keys() {
    let (doc, ids) = fixture();
    let gone = ItemId::from_u128(0x1234);
    let file = json!({
        "version": UI_STATE_VERSION,
        "playhead": "oops",
        "zoom": 40.0,
        "scroll": {"x": 1.0},
        "master_gain": 0.5,
        "preview_quality": "quarter",
        "panel_layout": {"dock": []},
        "selection": [item(ids.a), item(gone), {"kind": "bogus"}, 7],
        "future_feature": {"x": [1, 2, 3]},
    });
    let (ctx, warnings) = ViewCtx::from_json(&file, doc.clone());
    let s = ctx.state();
    assert_eq!(
        s.playhead,
        Time::ZERO,
        "bad field falls back to its default"
    );
    assert_eq!(s.zoom, 40.0);
    assert_eq!(s.scroll, UiState::initial().scroll);
    assert_eq!(s.master_gain, 0.5);
    assert_eq!(s.preview_quality, PreviewQuality::Quarter);
    assert_eq!(
        s.selection,
        vec![SelectionRef::Item { id: ids.a }],
        "stale and malformed IDs dropped"
    );
    assert!(
        warnings.iter().any(|w| w.contains("playhead")),
        "{warnings:?}"
    );
    assert!(
        warnings.iter().any(|w| w.contains("scroll")),
        "{warnings:?}"
    );
    assert_eq!(ctx.to_json()["future_feature"], file["future_feature"]);

    for junk in [json!(null), json!([1]), json!("x"), json!(3)] {
        let (ctx, warnings) = ViewCtx::from_json(&junk, doc.clone());
        assert_eq!(ctx.state(), &UiState::initial());
        assert_eq!(warnings.len(), 1);
    }
    let (_, w) = ViewCtx::from_json(&json!({"version": 99}), doc);
    assert!(w[0].contains("newer"));
}

fn arb_patch(ids: [ItemId; 2]) -> impl Strategy<Value = ViewPatch> {
    (
        proptest::option::of(0i64..1_000_000_000_000),
        proptest::option::of(0.001f64..100_000.0),
        proptest::option::of((0f64..1e6, 0f64..1e6)),
        proptest::option::of(proptest::collection::vec(0usize..2, 0..6)),
        proptest::option::of(any::<u8>()),
        proptest::option::of(0f64..4.0),
        proptest::option::of(0u8..3),
    )
        .prop_map(move |(p, z, sc, sel, layout, g, q)| ViewPatch {
            playhead: p.map(|f| Time::from_flicks(f).unwrap().into()),
            zoom: z,
            scroll: sc.map(|(x, y)| cutweave_ops::view::Scroll { x, y }),
            selection: sel.map(|v| {
                v.into_iter()
                    .map(|i| SelectionRef::Item { id: ids[i] })
                    .collect()
            }),
            panel_layout: layout.map(|n| json!({"n": n, "list": [n, "x"]})),
            preview_quality: q.map(|q| match q {
                0 => PreviewQuality::Full,
                1 => PreviewQuality::Half,
                _ => PreviewQuality::Quarter,
            }),
            master_gain: g,
        })
}

fn arb_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        any::<f64>().prop_map(|f| json!(f)),
        "[a-z_]{0,8}".prop_map(Value::from),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            proptest::collection::btree_map(
                prop_oneof![
                    Just("playhead".to_owned()),
                    Just("zoom".to_owned()),
                    Just("selection".to_owned()),
                    Just("scroll".to_owned()),
                    "[a-z_]{1,6}"
                ],
                inner,
                0..5
            )
            .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

proptest! {
    #[test]
    fn round_trip_through_json(patches in {
        let (_, ids) = fixture();
        proptest::collection::vec(arb_patch([ids.a, ids.b]), 0..8)
    }) {
        let (doc, _) = fixture();
        let mut ctx = ViewCtx::new(doc.clone());
        for p in &patches {
            ctx.apply_patch(p).unwrap();
        }
        let text = serde_json::to_string(&ctx.to_json()).unwrap();
        let (back, warnings) = ViewCtx::from_json(&serde_json::from_str(&text).unwrap(), doc);
        prop_assert!(warnings.is_empty(), "{:?}", warnings);
        prop_assert_eq!(back.state(), ctx.state());
        prop_assert_eq!(back.to_json(), ctx.to_json());
    }

    #[test]
    fn patch_equals_field_assignment(p in {
        let (_, ids) = fixture();
        arb_patch([ids.a, ids.b])
    }) {
        let (doc, _) = fixture();
        let mut ctx = ViewCtx::new(doc);
        let before = ctx.state().clone();
        ctx.apply_patch(&p).unwrap();
        let after = ctx.state();
        if p.zoom.is_none() { prop_assert_eq!(after.zoom, before.zoom); }
        if p.master_gain.is_none() { prop_assert_eq!(after.master_gain, before.master_gain); }
        if p.playhead.is_none() { prop_assert_eq!(after.playhead, before.playhead); }
        if p.selection.is_none() { prop_assert_eq!(&after.selection, &before.selection); }
        if p.panel_layout.is_none() { prop_assert_eq!(&after.panel_layout, &before.panel_layout); }
        if let Some(z) = p.zoom { prop_assert_eq!(after.zoom, z); }
    }

    #[test]
    fn arbitrary_files_never_panic_and_always_yield_valid_state(v in arb_json()) {
        let (doc, _) = fixture();
        let (ctx, _) = ViewCtx::from_json(&v, doc.clone());
        // Whatever came in, the result serializes and reloads without a single warning.
        let (again, warnings) = ViewCtx::from_json(&ctx.to_json(), doc);
        prop_assert!(warnings.is_empty(), "{:?}", warnings);
        prop_assert_eq!(again.state(), ctx.state());
    }
}
