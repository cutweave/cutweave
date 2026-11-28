// SPDX-License-Identifier: GPL-3.0-or-later
//! Export presets (WP-2G.01): the built-in pack validates and is snapshotted, every platform
//! preset names its source and date, safe areas exist, a mismatching aspect is a warning that
//! changes nothing, hardware falls back by `hw_preferred`, and the four commands work.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::{Arc, Mutex};

use common::{H, P, insert_av};
use cutweave_commands::{Access, CmdError, DocSnapshot, Rev};
use cutweave_jobs::JobId;
use cutweave_model::{Duration, FrameRate};
use cutweave_ops::export::presets::{
    self, Container, ExportPresetSpec, PRESET_SCHEMA_VERSION, PlanRate, PresetCtx, PresetStore,
    PresetWarningCode, plan_preset,
};
use cutweave_ops::export::{ExportCtx, ExportHw, ExportPlan, ExportStarter};
use serde_json::{Value, json};

const SEC: i64 = 705_600_000;

fn project() -> (H, P) {
    let (mut h, p) = H::project();
    let a = h.add_video("/media/a.mp4", 3, true);
    insert_av(&mut h, p, a, "0");
    (h, p)
}

fn snapshot(h: &H) -> Arc<DocSnapshot> {
    Arc::new(DocSnapshot {
        rev: Rev(1),
        committed_rev: Rev(1),
        doc: h.doc(),
    })
}

fn run(h: &H, store: &Arc<PresetStore>, name: &str, params: Value) -> Result<Value, CmdError> {
    let mut ctx = PresetCtx {
        store: Arc::clone(store),
        snapshot: Some(snapshot(h)),
    };
    h.registry
        .invoke_engine_json(name, params, &mut ctx, Access::STANDARD)
}

struct Recorder(Arc<Mutex<Vec<ExportPlan>>>);

impl ExportStarter for Recorder {
    fn start(&mut self, plan: ExportPlan) -> Result<JobId, CmdError> {
        self.0.lock().unwrap().push(plan);
        Ok(JobId::from_u128(9))
    }
}

fn export(
    h: &H,
    store: &Arc<PresetStore>,
    params: Value,
) -> (Result<Value, CmdError>, Vec<ExportPlan>) {
    let plans = Arc::new(Mutex::new(Vec::new()));
    let mut ctx = ExportCtx {
        snapshot: snapshot(h),
        allowed_roots: None,
        starter: Box::new(Recorder(Arc::clone(&plans))),
        presets: Arc::clone(store),
    };
    let out = h
        .registry
        .invoke_engine_json("export.start", params, &mut ctx, Access::STANDARD);
    let plans = plans.lock().unwrap().clone();
    (out, plans)
}

fn out_path() -> String {
    std::env::temp_dir()
        .join("cutweave-presets-test.mp4")
        .to_string_lossy()
        .into_owned()
}

fn mine(id: &str) -> Value {
    json!({
        "schema_version": 1, "id": id, "name": "Mine", "container": "mp4",
        "video": {"codec": "h264", "rate_control": {"mode": "crf", "crf": 22}, "hw_preferred": false},
        "audio": {"codec": "aac", "bitrate_kbps": 192, "sample_rate": 48000, "channels": 2},
        "size": {"kind": "fit", "width": 1280, "height": 720},
        "fps": {"kind": "follow_sequence"}
    })
}

#[test]
fn all_builtin_presets_validate_and_their_ids_are_snapshotted() {
    let b = presets::builtin();
    assert_eq!(b.problem, None);
    // The text is parsed and validated again, so a broken pack names the preset.
    let again = presets::parse_pack(presets::builtin_json()).expect("the pack is valid");
    assert_eq!(again.len(), b.presets.len());
    for p in &b.presets {
        assert_eq!(p.schema_version, PRESET_SCHEMA_VERSION);
        p.validate().unwrap_or_else(|e| panic!("{}: {e}", p.id));
    }
    let ids: Vec<&str> = b.presets.iter().map(|p| p.id.as_str()).collect();
    insta::assert_json_snapshot!("builtin_preset_ids", ids);
}

#[test]
fn the_pack_covers_the_wbs_set() {
    let ids: Vec<String> = presets::builtin()
        .presets
        .iter()
        .map(|p| p.id.clone())
        .collect();
    for want in [
        "youtube-1080p",
        "youtube-4k",
        "youtube-shorts",
        "instagram-reels",
        "instagram-stories",
        "instagram-feed-4x5",
        "instagram-feed-1x1",
        "tiktok",
        "x-landscape",
        "x-portrait",
        "gif",
        "audio-wav",
        "audio-mp3",
        "audio-aac",
        "audio-flac",
        "audio-opus",
        "prores-422-hq",
        "archive-hevc",
        "h264-draft",
        "h264-mp4-1080p",
    ] {
        assert!(ids.iter().any(|i| i == want), "missing {want}");
    }
}

#[test]
fn platform_presets_name_their_source_date_and_are_h264_aac_mp4() {
    let safe = presets::known_safe_areas();
    let mut platforms = 0;
    for p in &presets::builtin().presets {
        let Some(pl) = &p.platform else { continue };
        platforms += 1;
        assert!(pl.spec_url.starts_with("https://"), "{}", p.id);
        assert_eq!(pl.checked.len(), 10, "{}", p.id);
        assert_eq!(p.container, Container::Mp4, "{}", p.id);
        p.engine_support()
            .unwrap_or_else(|e| panic!("{} is not H.264 + AAC MP4: {e}", p.id));
        if let Some(area) = &p.safe_area {
            assert!(safe.contains(area), "{}: unknown safe area `{area}`", p.id);
        }
    }
    assert_eq!(platforms, 10);
    // Portrait platform presets name a safe area (the zone to show in the preview).
    for id in [
        "youtube-shorts",
        "instagram-reels",
        "instagram-stories",
        "tiktok",
    ] {
        let p = presets::builtin()
            .presets
            .iter()
            .find(|p| p.id == id)
            .unwrap();
        assert_eq!(p.safe_area.as_deref(), Some(id));
    }
}

#[test]
fn safe_areas_exist_in_the_overlay_pack_once_it_lands() {
    // WP-2H.06 adds `overlays/safe-areas.json`; until then the pack manifest lists the ids.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let found = [
        "packages/editor/src/preview/overlays/safe-areas.json",
        "overlays/safe-areas.json",
    ]
    .iter()
    .map(|p| root.join(p))
    .find(|p| p.is_file());
    let Some(file) = found else { return };
    let text = std::fs::read_to_string(&file).unwrap();
    for area in presets::known_safe_areas() {
        assert!(
            text.contains(&format!("\"{area}\"")),
            "{} lacks the safe area `{area}`",
            file.display()
        );
    }
}

#[test]
fn a_broken_preset_is_rejected_with_a_pointer() {
    let mut p: ExportPresetSpec = serde_json::from_value(mine("ok")).unwrap();
    p.validate().unwrap();
    for (mutate, pointer) in [
        (json!({"id": "Not Valid"}), "/id"),
        (json!({"schema_version": 2}), "/schema_version"),
        (json!({"container": "gif"}), "/video/codec"),
        (
            json!({"size": {"kind": "fixed", "width": 1001, "height": 1080}}),
            "/size",
        ),
        (json!({"loop": 1}), "/loop"),
        (json!({"safe_area": "tiktok"}), "/safe_area"),
    ] {
        let mut v = mine("x");
        v.as_object_mut()
            .unwrap()
            .extend(mutate.as_object().unwrap().clone());
        match serde_json::from_value::<ExportPresetSpec>(v) {
            Ok(p) => {
                let e = p.validate().unwrap_err();
                assert!(
                    e.to_string().contains(pointer) || format!("{e:?}").contains(pointer),
                    "{pointer}: {e}"
                );
            }
            Err(e) => panic!("{pointer}: {e}"),
        }
    }
    p.video.as_mut().unwrap().rate_control = presets::RateControlSpec::Crf { crf: 60 };
    assert!(p.validate().is_err());
    // Unknown fields are refused.
    let mut v = mine("x");
    v["surprise"] = json!(1);
    assert!(serde_json::from_value::<ExportPresetSpec>(v).is_err());
}

#[test]
fn tiktok_on_a_16_9_sequence_warns_aspect_mismatch_and_changes_nothing() {
    let (h, p) = project();
    let store = Arc::new(PresetStore::default());
    let before = serde_json::to_value(h.doc()).unwrap();
    let out = run(
        &h,
        &store,
        "export.resolve_preset",
        json!({"preset": "tiktok"}),
    )
    .unwrap();
    assert_eq!(out["sequence"], json!(p.sequence));
    let codes: Vec<&Value> = out["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| &w["code"])
        .collect();
    assert_eq!(codes, [&json!("AspectMismatch")]);
    // The sequence's shape is kept, not stretched: 1080 x 608 fits into 1080 x 1920.
    assert_eq!(
        (out["width"].clone(), out["height"].clone()),
        (json!(1080), json!(608))
    );
    assert_eq!(serde_json::to_value(h.doc()).unwrap(), before);
}

#[test]
fn a_matching_aspect_gives_the_preset_size_and_a_short_clip_has_no_warnings() {
    let (h, _) = project();
    let store = Arc::new(PresetStore::default());
    let out = run(
        &h,
        &store,
        "export.resolve_preset",
        json!({"preset": "youtube-4k"}),
    )
    .unwrap();
    assert_eq!(
        (out["width"].clone(), out["height"].clone()),
        (json!(3840), json!(2160))
    );
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(out["supported"], json!(true));
    assert!(out["estimated_kib"].as_u64().unwrap() > 0);
}

#[test]
fn limits_are_warnings_not_errors() {
    let (h, _) = project();
    let doc = h.doc();
    let seq = doc.sequences.values().next().unwrap();
    let mut spec = presets::builtin()
        .presets
        .iter()
        .find(|p| p.id == "youtube-shorts")
        .unwrap()
        .clone();
    spec.limits.max_duration_secs = Some(2);
    spec.limits.max_size_mb = Some(1);
    let plan = plan_preset(&spec, seq, Duration::from_secs(5).unwrap(), None).unwrap();
    let codes: Vec<_> = plan.warnings.iter().map(|w| w.code).collect();
    assert!(codes.contains(&PresetWarningCode::DurationOverLimit));
    // 5 s at 8.1 Mbit/s is about 5 MB.
    assert!(codes.contains(&PresetWarningCode::SizeOverLimit));
}

#[test]
fn the_frame_rate_follows_the_sequence_up_to_the_platform_maximum() {
    let (h, _) = project();
    let doc = h.doc();
    let mut seq = doc.sequences.values().next().unwrap().clone();
    let seq = &mut seq;
    let spec = presets::builtin()
        .presets
        .iter()
        .find(|p| p.id == "tiktok")
        .unwrap()
        .clone();
    let len = Duration::from_secs(3).unwrap();
    seq.format.frame_rate = FrameRate::FPS_24;
    let plan = plan_preset(&spec, &*seq, len, None).unwrap();
    assert_eq!(plan.frame_rate, FrameRate::FPS_24);
    assert!(
        !plan
            .warnings
            .iter()
            .any(|w| w.code == PresetWarningCode::FrameRateCapped)
    );
    seq.format.frame_rate = FrameRate::FPS_60;
    let plan = plan_preset(&spec, &*seq, len, None).unwrap();
    assert_eq!(plan.frame_rate, FrameRate::FPS_30);
    assert!(
        plan.warnings
            .iter()
            .any(|w| w.code == PresetWarningCode::FrameRateCapped)
    );
    seq.format.frame_rate = FrameRate::FPS_59_94;
    assert_eq!(
        plan_preset(&spec, &*seq, len, None).unwrap().frame_rate,
        FrameRate::FPS_29_97
    );
}

#[test]
fn export_start_with_a_preset_id_plans_size_rate_and_hardware() {
    let (h, _) = project();
    let store = Arc::new(PresetStore::default());
    let (out, plans) = export(
        &h,
        &store,
        json!({"path": out_path(), "preset": "youtube-1080p", "overwrite": true}),
    );
    let out = out.unwrap();
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(out["out"], json!(3 * SEC));
    let plan = &plans[0];
    assert_eq!((plan.width, plan.height), (1920, 1080));
    assert_eq!(plan.rate, Some(PlanRate::BitrateKbps(8000)));
    // hw_preferred: true -> hardware when it works, software otherwise.
    assert_eq!(plan.hw, ExportHw::Auto);
    // An explicit `hw` wins, and a quality override replaces the preset's rate control.
    let (_, plans) = export(
        &h,
        &store,
        json!({"path": out_path(), "overwrite": true, "hw": "off", "preset": {"custom": {"preset": "youtube-1080p",
                "quality": "low", "width": 1280, "height": 720}}}),
    );
    let plan = &plans[0];
    assert_eq!(plan.hw, ExportHw::Off);
    assert_eq!(plan.rate, None);
    assert_eq!((plan.width, plan.height), (1280, 720));
}

#[test]
fn hw_preferred_false_means_software() {
    let (h, _) = project();
    let store = Arc::new(PresetStore::default());
    run(
        &h,
        &store,
        "export.save_preset",
        json!({"preset": mine("soft")}),
    )
    .unwrap();
    let (out, plans) = export(
        &h,
        &store,
        json!({"path": out_path(), "preset": "soft", "overwrite": true}),
    );
    out.unwrap();
    assert_eq!(plans[0].hw, ExportHw::Off);
    assert_eq!(plans[0].rate, Some(PlanRate::Crf(22)));
}

#[test]
fn presets_this_build_cannot_encode_fail_with_unsupported_and_list_as_such() {
    let (h, _) = project();
    let store = Arc::new(PresetStore::default());
    // GIF and audio-only formats wait for WP-2G.02b.
    for id in ["gif", "audio-wav", "audio-mp3"] {
        let (out, plans) = export(
            &h,
            &store,
            json!({"path": out_path(), "preset": id, "overwrite": true}),
        );
        assert_eq!(out.unwrap_err().code(), "unsupported", "{id}");
        assert!(plans.is_empty());
    }
    let list = run(&h, &store, "export.list_presets", json!({})).unwrap();
    let entry = list["presets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["preset"]["id"] == "gif")
        .unwrap();
    assert_eq!(entry["supported"], json!(false));
    assert!(
        entry["unsupported_reason"]
            .as_str()
            .unwrap()
            .contains("WP-2G.02b")
    );
    // HEVC, AV1 and ProRes are exportable now.
    for id in ["archive-hevc", "av1-small", "prores-422-hq"] {
        let entry = list["presets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["preset"]["id"] == id)
            .unwrap();
        assert_eq!(entry["supported"], json!(true), "{id}");
    }
    // Unknown ids, also as the base of a custom preset, are invalid.
    let (out, _) = export(
        &h,
        &store,
        json!({"path": out_path(), "preset": "nope", "overwrite": true}),
    );
    assert_eq!(out.unwrap_err().code(), "invalid");
    let (out, _) = export(
        &h,
        &store,
        json!({"path": out_path(), "preset": {"custom": {"preset": "nope"}}, "overwrite": true}),
    );
    assert_eq!(out.unwrap_err().code(), "invalid");
}

#[test]
fn the_legacy_presets_still_work() {
    let (h, _) = project();
    let store = Arc::new(PresetStore::default());
    let (out, plans) = export(
        &h,
        &store,
        json!({"path": out_path(), "preset": "h264-mp4-1080p", "overwrite": true}),
    );
    out.unwrap();
    assert_eq!((plans[0].width, plans[0].height), (1920, 1080));
    let (out, _) = export(
        &h,
        &store,
        json!({"path": out_path(), "preset": {"custom": {"width": 640}}, "overwrite": true}),
    );
    out.unwrap();
}

#[test]
fn listing_filters_by_platform() {
    let (h, _) = project();
    let store = Arc::new(PresetStore::default());
    let list = run(
        &h,
        &store,
        "export.list_presets",
        json!({"platform": "instagram-reels"}),
    )
    .unwrap();
    let ids: Vec<&str> = list["presets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["preset"]["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["instagram-reels"]);
}

#[test]
fn user_presets_save_list_replace_delete_and_persist() {
    let (h, _) = project();
    let dir = std::env::temp_dir().join(format!("cutweave-presets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Arc::new(PresetStore::open(Some(dir.clone())));
    let saved = run(
        &h,
        &store,
        "export.save_preset",
        json!({"preset": mine("mine-a")}),
    )
    .unwrap();
    assert_eq!(saved, json!({"id": "mine-a", "replaced": false}));
    let saved = run(
        &h,
        &store,
        "export.save_preset",
        json!({"preset": mine("mine-a")}),
    )
    .unwrap();
    assert_eq!(saved["replaced"], json!(true));
    // Built-in ids are protected both ways; invalid presets are refused.
    let e = run(
        &h,
        &store,
        "export.save_preset",
        json!({"preset": mine("tiktok")}),
    )
    .unwrap_err();
    assert_eq!(e.code(), "invalid");
    let e = run(&h, &store, "export.delete_preset", json!({"id": "tiktok"})).unwrap_err();
    assert_eq!(e.code(), "invalid");
    let e = run(&h, &store, "export.delete_preset", json!({"id": "nothing"})).unwrap_err();
    assert_eq!(e.code(), "invalid");
    let e = run(
        &h,
        &store,
        "export.save_preset",
        json!({"preset": mine("Bad Id")}),
    )
    .unwrap_err();
    assert_eq!(e.code(), "invalid");
    // A second store on the same folder reads the saved preset back.
    let reopened = PresetStore::open(Some(dir.clone()));
    assert_eq!(reopened.problems(), &[] as &[String]);
    assert!(reopened.get("mine-a").is_some());
    let entries = run(&h, &store, "export.list_presets", json!({})).unwrap();
    let last = entries["presets"].as_array().unwrap().last().unwrap();
    assert_eq!(
        (last["source"].clone(), last["preset"]["id"].clone()),
        (json!("user"), json!("mine-a"))
    );
    run(&h, &store, "export.delete_preset", json!({"id": "mine-a"})).unwrap();
    assert!(PresetStore::open(Some(dir.clone())).get("mine-a").is_none());
    // A damaged file is reported, not fatal.
    std::fs::write(dir.join(presets::USER_PRESETS_FILE), "{not json").unwrap();
    assert_eq!(PresetStore::open(Some(dir.clone())).problems().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

fn out_path_ext(ext: &str) -> String {
    std::env::temp_dir()
        .join(format!("cutweave-presets-test.{ext}"))
        .to_string_lossy()
        .into_owned()
}

#[test]
fn codec_presets_plan_their_container_codec_profile_bit_depth_and_audio() {
    use cutweave_ops::export::PlanOutput;
    use cutweave_ops::export::presets::{AudioCodec, VideoCodec};
    let (h, _) = project();
    let store = Arc::new(PresetStore::default());
    let plan = |id: &str, ext: &str| {
        let (out, plans) = export(
            &h,
            &store,
            json!({"path": out_path_ext(ext), "preset": id, "overwrite": true}),
        );
        (out, plans)
    };
    // The classic export is MP4, 8-bit H.264, AAC.
    let (out, plans) = plan("h264-mp4-1080p", "mp4");
    out.unwrap();
    assert_eq!(plans[0].output.video_codec, VideoCodec::H264);
    assert_eq!(plans[0].output.bit_depth, 8);
    assert_eq!(
        plans[0].output.audio_bitrate_kbps, None,
        "the default export keeps the encoder's AAC rate"
    );
    // HEVC archive: 10-bit Main 10 in MP4, software.
    let (out, plans) = plan("archive-hevc", "mp4");
    out.unwrap();
    assert_eq!(
        plans[0].output,
        PlanOutput {
            container: Container::Mp4,
            video_codec: VideoCodec::Hevc,
            profile: Some("main10".into()),
            bit_depth: 10,
            alpha: false,
            audio_codec: AudioCodec::Aac,
            audio_bitrate_kbps: Some(320),
            advanced: cutweave_ops::export::PlanAdvanced::default(),
        }
    );
    assert_eq!(
        plans[0].hw,
        ExportHw::Off,
        "archive presets are quality-first"
    );
    assert_eq!(plans[0].rate, Some(PlanRate::Crf(16)));
    // AV1.
    let (out, plans) = plan("av1-small", "mp4");
    out.unwrap();
    assert_eq!(plans[0].output.video_codec, VideoCodec::Av1);
    assert_eq!(plans[0].output.bit_depth, 8);
    // ProRes goes into .mov with PCM audio; the .mp4 name is refused.
    let (out, plans) = plan("prores-422-hq", "mov");
    out.unwrap();
    let o = &plans[0].output;
    assert_eq!(
        (o.container, o.video_codec, o.bit_depth, o.audio_codec),
        (Container::Mov, VideoCodec::Prores, 10, AudioCodec::Pcm)
    );
    assert_eq!(o.profile.as_deref(), Some("422hq"));
    assert_eq!(o.extension(), "mov");
    let (out, _) = plan("prores-422-hq", "mp4");
    let err = out.unwrap_err();
    assert_eq!(err.code(), "invalid");
    assert!(err.to_string().contains(".mov"), "{err}");
    // And the MP4 presets refuse .mov.
    let (out, _) = plan("archive-hevc", "mov");
    assert_eq!(out.unwrap_err().code(), "invalid");
}

#[test]
fn a_user_preset_can_name_prores_4444_with_alpha_and_hevc_profiles_are_checked() {
    let (h, _) = project();
    let store = Arc::new(PresetStore::default());
    let prores = |profile: &str, pix: &str| {
        json!({
            "schema_version": 1, "id": "p4", "name": "P4", "container": "mov",
            "video": {"codec": "prores", "rate_control": {"mode": "profile"},
                      "profile": profile, "pix_fmt": pix, "hw_preferred": false},
            "audio": {"codec": "pcm", "sample_rate": 48000, "channels": 2},
            "size": {"kind": "follow_sequence"}, "fps": {"kind": "follow_sequence"}
        })
    };
    let save = |preset: Value| run(&h, &store, "export.save_preset", json!({"preset": preset}));
    save(prores("4444", "yuva444p10le")).unwrap();
    let (out, plans) = export(
        &h,
        &store,
        json!({"path": out_path_ext("mov"), "preset": "p4", "overwrite": true}),
    );
    out.unwrap();
    assert!(plans[0].output.alpha, "yuva keeps the alpha channel");
    // A profile and pixel format that do not belong together are `Invalid` at their pointer.
    let err = save(prores("422hq", "yuv444p10le")).unwrap_err();
    assert_eq!(err.code(), "invalid");
    let err = save(prores("nope", "yuv422p10le")).unwrap_err();
    assert!(err.to_string().contains("ProRes profiles"), "{err}");
    let mut hevc = mine("hv");
    hevc["video"] = json!({"codec": "hevc", "rate_control": {"mode": "crf", "crf": 20},
        "profile": "main", "pix_fmt": "yuv420p10le", "hw_preferred": false});
    let err = save(hevc).unwrap_err();
    assert_eq!(err.code(), "invalid");
    assert!(err.to_string().contains("main10"), "{err}");
}

#[test]
fn export_list_encoders_asks_the_engine_and_lists_the_four_codecs() {
    struct Found;
    impl ExportStarter for Found {
        fn start(&mut self, _plan: ExportPlan) -> Result<JobId, CmdError> {
            Ok(JobId::from_u128(1))
        }
        fn encoders(
            &mut self,
        ) -> Result<Vec<cutweave_ops::export::encoders::EncoderEntry>, CmdError> {
            use cutweave_ops::export::encoders::EncoderEntry;
            Ok(vec![
                EncoderEntry {
                    codec: "hevc".into(),
                    implementation: "libx265".into(),
                    hardware: false,
                    device: None,
                },
                EncoderEntry {
                    codec: "hevc".into(),
                    implementation: "hevc_nvenc".into(),
                    hardware: true,
                    device: Some("cuda".into()),
                },
            ])
        }
    }
    let (h, _) = project();
    let mut ctx = ExportCtx {
        snapshot: snapshot(&h),
        allowed_roots: None,
        starter: Box::new(Found),
        presets: Arc::default(),
    };
    let out = h
        .registry
        .invoke_engine_json(
            "export.list_encoders",
            json!({}),
            &mut ctx,
            Access::STANDARD,
        )
        .unwrap();
    let codecs = out["codecs"].as_array().unwrap();
    let names: Vec<&str> = codecs
        .iter()
        .map(|c| c["codec"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["h264", "hevc", "av1", "prores"]);
    assert_eq!(codecs[1]["software"], json!("libx265"));
    assert_eq!(
        codecs[1]["hardware"][0]["implementation"],
        json!("hevc_nvenc")
    );
    assert_eq!(codecs[1]["hardware"][0]["device"], json!("cuda"));
    assert_eq!(codecs[2]["available"], json!(false));
    // An engine without a media backend says so.
    let mut none = ExportCtx {
        snapshot: snapshot(&h),
        allowed_roots: None,
        starter: Box::new(Recorder(Arc::default())),
        presets: Arc::default(),
    };
    let err = h
        .registry
        .invoke_engine_json(
            "export.list_encoders",
            json!({}),
            &mut none,
            Access::STANDARD,
        )
        .unwrap_err();
    assert_eq!(err.code(), "unsupported");
}

// Issue #238: a transparent background only shows in a file that keeps alpha; any other export
// says so (a warning, the export runs), in the pre-check and in what the export answers.

fn warning_codes(out: &Value) -> Vec<String> {
    out["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["code"].as_str().unwrap().to_owned())
        .collect()
}

fn prores_preset(id: &str, profile: &str, pix: &str) -> Value {
    json!({
        "schema_version": 1, "id": id, "name": id, "container": "mov",
        "video": {"codec": "prores", "rate_control": {"mode": "profile"},
                  "profile": profile, "pix_fmt": pix, "hw_preferred": false},
        "audio": {"codec": "pcm", "sample_rate": 48000, "channels": 2},
        "size": {"kind": "follow_sequence"}, "fps": {"kind": "follow_sequence"}
    })
}

#[test]
fn a_transparent_background_warns_when_the_file_keeps_no_alpha() {
    let (mut h, p) = project();
    let store = Arc::new(PresetStore::default());
    let resolve = |h: &H, preset: &str| {
        run(
            h,
            &store,
            "export.resolve_preset",
            json!({"preset": preset}),
        )
        .unwrap()
    };
    // A color background: nothing to say.
    assert_eq!(resolve(&h, "youtube-4k")["warnings"], json!([]));
    h.ok(
        "sequence.set_background",
        json!({"sequence": p.sequence, "kind": "transparent"}),
    );
    // H.264 has no alpha channel: the pre-check and the export both say what the file shows.
    let out = resolve(&h, "youtube-4k");
    assert_eq!(warning_codes(&out), ["TransparentBackground"]);
    let message = out["warnings"][0]["message"].as_str().unwrap();
    assert!(message.contains("black"), "{message}");
    let (out, plans) = export(
        &h,
        &store,
        json!({"path": out_path(), "preset": "youtube-4k", "overwrite": true}),
    );
    assert_eq!(warning_codes(&out.unwrap()), ["TransparentBackground"]);
    assert_eq!(plans.len(), 1, "a warning: the export runs");
    // So does the export that names no preset (the default H.264 MP4).
    let (out, _) = export(&h, &store, json!({"path": out_path(), "overwrite": true}));
    assert_eq!(warning_codes(&out.unwrap()), ["TransparentBackground"]);
    // No built-in preset keeps alpha today: every one with a picture that this build can
    // export warns (GIF is not exported yet, and says that instead).
    let mut warned = 0;
    for preset in &presets::builtin().presets {
        let out = resolve(&h, &preset.id);
        if preset.video.is_none() || out["supported"] == json!(false) {
            continue;
        }
        assert!(
            warning_codes(&out).contains(&"TransparentBackground".to_owned()),
            "{}",
            preset.id
        );
        warned += 1;
    }
    assert!(warned > 10, "{warned}");
}

#[test]
fn a_transparent_background_goes_to_an_alpha_file_without_a_warning() {
    let (mut h, p) = project();
    let store = Arc::new(PresetStore::default());
    h.ok(
        "sequence.set_background",
        json!({"sequence": p.sequence, "kind": "transparent"}),
    );
    let save = |preset: Value| {
        run(&h, &store, "export.save_preset", json!({"preset": preset})).unwrap();
    };
    // ProRes 4444 with an alpha pixel format keeps it.
    save(prores_preset("with-alpha", "4444", "yuva444p10le"));
    let out = run(
        &h,
        &store,
        "export.resolve_preset",
        json!({"preset": "with-alpha"}),
    )
    .unwrap();
    assert_eq!(out["warnings"], json!([]));
    let (out, plans) = export(
        &h,
        &store,
        json!({"path": out_path_ext("mov"), "preset": "with-alpha", "overwrite": true}),
    );
    assert_eq!(out.unwrap()["warnings"], json!([]));
    assert!(plans[0].output.alpha);
    // ProRes 4444 without the alpha plane does not, until the request asks for it.
    save(prores_preset("no-alpha", "4444", "yuv444p10le"));
    let out = run(
        &h,
        &store,
        "export.resolve_preset",
        json!({"preset": "no-alpha"}),
    )
    .unwrap();
    assert_eq!(warning_codes(&out), ["TransparentBackground"]);
    let (out, _) = export(
        &h,
        &store,
        json!({"path": out_path_ext("mov"), "preset": "no-alpha", "overwrite": true,
               "advanced": {"video": {"pix_fmt": "yuva444p10le"}}}),
    );
    assert_eq!(out.unwrap()["warnings"], json!([]));
}
