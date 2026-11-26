// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.05d (ADR-0035 items 3 and 4): `media.import_images_as_slideshow` and
//! `media.import_frame`, through the `MediaProbe`, `MediaSink` and `FrameGrabber` boundaries.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;

use common::{H, P, assert_code, assert_invalid, id};
use cutweave_commands::{Access, CmdError};
use cutweave_model::{Duration, ItemId, MediaId, MediaKind, Project, Time, TrackId};
use cutweave_ops::media::{AllowedRoots, FakeProbe, ImportPolicy};
use cutweave_ops::testing::{FakeFrames, fake_probe};
use serde_json::{Value, json};

const SLIDESHOW: &str = "media.import_images_as_slideshow";
const FRAME: &str = "media.import_frame";

fn pictures() -> FakeProbe {
    FakeProbe::new()
        .with("/p/img10.png", FakeProbe::image("/p/img10.png"))
        .with("/p/img2.png", FakeProbe::image("/p/img2.png"))
        .with("/p/Beach.png", FakeProbe::image("/p/Beach.png"))
        .with("/p/apple.png", FakeProbe::image("/p/apple.png"))
        // The same content as img2.png under another name.
        .with("/p/copy.png", FakeProbe::image("/p/img2.png"))
        .with("/p/clip.mp4", FakeProbe::video("/p/clip.mp4", 10, true))
}

fn slideshow(h: &H, params: Value) -> Result<Value, CmdError> {
    let mut ctx = h.import_ctx(pictures());
    h.registry
        .invoke_engine_json(SLIDESHOW, params, &mut ctx, Access::STANDARD)
}

/// The clips of `track` as `(file name, start, duration)`, in time order.
fn clips(doc: &Project, p: &P, track: TrackId) -> Vec<(String, Time, Duration)> {
    doc.sequences[&p.sequence].tracks[&track]
        .items
        .iter()
        .map(|item| {
            let media = &doc.media[&item.media().unwrap()];
            let path = media.path_abs.clone().unwrap();
            let name = path.rsplit('/').next().unwrap().to_owned();
            (name, item.start(), item.duration())
        })
        .collect()
}

fn secs(s: i64) -> Time {
    Time::from_secs(s).unwrap()
}

fn dur(s: i64) -> Duration {
    Duration::from_secs(s).unwrap()
}

#[test]
fn a_slideshow_places_the_pictures_one_after_the_other_in_one_transaction() {
    let (h, p) = H::project();
    let before = h.commits();
    let out = slideshow(
        &h,
        json!({"files": ["/p/img10.png", "/p/img2.png", "/p/Beach.png"]}),
    )
    .unwrap();
    assert_eq!(h.commits(), before + 1, "one transaction, so one undo step");
    let doc = h.doc();
    assert_eq!(doc.media.len(), 3);
    // The default length is the project's still duration (5 s), the default place the main
    // video track from its end (empty: 0).
    assert_eq!(
        clips(&doc, &p, p.video),
        [
            ("img10.png".to_owned(), secs(0), dur(5)),
            ("img2.png".to_owned(), secs(5), dur(5)),
            ("Beach.png".to_owned(), secs(10), dur(5)),
        ]
    );
    assert_eq!(out["sequence"], json!(p.sequence));
    assert_eq!(out["track"], json!(p.video));
    assert_eq!(out["at"], json!(secs(0)));
    assert_eq!(out["per_image"], json!(dur(5)));
    assert_eq!(out["end"], json!(secs(15)));
    assert_eq!(out["failed"], json!([]));
    let placed: Vec<ItemId> = out["clips"].as_array().unwrap().iter().map(id).collect();
    assert_eq!(placed.len(), 3);
    let media: Vec<MediaId> = out["media_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(id)
        .collect();
    for (clip, media) in placed.iter().zip(&media) {
        let (_, item) = doc.sequences[&p.sequence].find_item(clip).unwrap();
        assert_eq!(item.media(), Some(*media));
        assert_eq!(doc.media[media].kind, MediaKind::Image);
    }
    assert_eq!(out["media"][1]["path"], "/p/img2.png");
    assert_eq!(out["media"][1]["duplicate"], false);
}

#[test]
fn per_image_sets_the_length_and_follows_the_project_default_when_absent() {
    let (mut h, p) = H::project();
    let out = slideshow(
        &h,
        json!({"files": ["/p/img2.png", "/p/apple.png"], "per_image": "2s"}),
    )
    .unwrap();
    assert_eq!(out["per_image"], json!(dur(2)));
    assert_eq!(
        clips(&h.doc(), &p, p.video),
        [
            ("img2.png".to_owned(), secs(0), dur(2)),
            ("apple.png".to_owned(), secs(2), dur(2)),
        ]
    );
    // A length between two frames snaps to whole frames (30 fps: 0.51 s is 15 frames).
    let out = slideshow(&h, json!({"files": ["/p/Beach.png"], "per_image": "0.51s"})).unwrap();
    assert_eq!(
        out["per_image"],
        json!(Duration::from_flicks(705_600_000 / 2).unwrap())
    );
    // A project with its own still duration: the next slideshow uses it.
    let mut doc = h.doc();
    doc.settings.still_duration = Some(dur(3));
    h.replace_doc(doc);
    let out = slideshow(&h, json!({"files": ["/p/img10.png"]})).unwrap();
    assert_eq!(out["per_image"], json!(dur(3)));
}

#[test]
fn the_order_is_given_by_name_or_by_name_with_numbers() {
    let files = json!([
        "/p/img10.png",
        "/p/img2.png",
        "/p/Beach.png",
        "/p/apple.png"
    ]);
    let names = |order: Value| -> Vec<String> {
        let (h, p) = H::project();
        let mut params = json!({"files": files, "per_image": "1s"});
        if !order.is_null() {
            params["order"] = order;
        }
        slideshow(&h, params).unwrap();
        clips(&h.doc(), &p, p.video)
            .into_iter()
            .map(|(name, _, _)| name)
            .collect()
    };
    assert_eq!(
        names(Value::Null),
        ["img10.png", "img2.png", "Beach.png", "apple.png"]
    );
    assert_eq!(
        names(json!("given")),
        ["img10.png", "img2.png", "Beach.png", "apple.png"]
    );
    assert_eq!(
        names(json!("name")),
        ["apple.png", "Beach.png", "img10.png", "img2.png"]
    );
    assert_eq!(
        names(json!("name_numeric")),
        ["apple.png", "Beach.png", "img2.png", "img10.png"]
    );
}

#[test]
fn pictures_already_in_the_project_are_reused_by_content() {
    let (mut h, p) = H::project();
    let known = h.add_image("/p/img2.png");
    let out = slideshow(
        &h,
        json!({"files": ["/p/copy.png", "/p/apple.png", "/p/img2.png"], "per_image": "1s"}),
    )
    .unwrap();
    let doc = h.doc();
    assert_eq!(doc.media.len(), 2, "one new asset: apple.png");
    assert_eq!(out["media_ids"][0], json!(known));
    assert_eq!(out["media_ids"][2], json!(known));
    assert_eq!(out["media"][0]["duplicate"], true);
    assert_eq!(out["media"][1]["duplicate"], false);
    assert_eq!(out["media"][2]["duplicate"], true);
    assert_eq!(
        clips(&doc, &p, p.video).len(),
        3,
        "three clips all the same"
    );
}

#[test]
fn files_that_are_not_pictures_are_left_out_and_reported() {
    let (h, p) = H::project();
    let out = slideshow(
        &h,
        json!({"files": ["/p/missing.png", "/p/img2.png", "/p/clip.mp4", "/p/apple.png"],
               "per_image": "1s"}),
    )
    .unwrap();
    assert_eq!(clips(&h.doc(), &p, p.video).len(), 2);
    let failed = out["failed"].as_array().unwrap();
    assert_eq!(failed.len(), 2);
    assert_eq!(failed[0]["path"], "/p/missing.png");
    assert_eq!(failed[0]["code"], "not_found");
    assert_eq!(failed[1]["path"], "/p/clip.mp4");
    assert_eq!(failed[1]["code"], "unsupported");
    assert_eq!(h.doc().media.len(), 2, "the video was not imported");

    // No picture at all: the call fails and changes nothing.
    let (h, _) = H::project();
    let before = h.commits();
    assert_invalid(
        slideshow(&h, json!({"files": ["/p/missing.png", "/p/clip.mp4"]})),
        "/files",
    );
    assert_eq!(h.commits(), before);
    assert!(h.doc().media.is_empty());
}

#[test]
fn the_pictures_go_after_what_is_on_the_track_or_where_asked() {
    let (mut h, p) = H::project();
    let video = h.add_video("/p/v.mp4", 10, false);
    h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": video, "at": 0}),
    );
    // Default: from the end of the track.
    let out = slideshow(
        &h,
        json!({"files": ["/p/img2.png", "/p/apple.png"], "per_image": "1s"}),
    )
    .unwrap();
    assert_eq!(out["at"], json!(secs(10)));
    assert_eq!(out["end"], json!(secs(12)));
    // At a time on the magnetic main track: what is after it moves later.
    h.ok(
        "track.set_magnetic",
        json!({"track": p.video, "magnetic": true}),
    );
    let out = slideshow(
        &h,
        json!({"files": ["/p/Beach.png", "/p/img10.png"], "per_image": "2s", "at": "10s",
               "sequence": p.sequence, "track": p.video}),
    )
    .unwrap();
    assert_eq!(out["at"], json!(secs(10)));
    let on_track = clips(&h.doc(), &p, p.video);
    let names: Vec<&str> = on_track.iter().map(|c| c.0.as_str()).collect();
    assert_eq!(
        names,
        ["v.mp4", "Beach.png", "img10.png", "img2.png", "apple.png"]
    );
    assert_eq!(on_track[3].1, secs(14), "the earlier pictures rippled");
    // The main track is also where pictures go when no track is named.
    let top: TrackId = id(&h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "video"}),
    )["track"]);
    let out = slideshow(&h, json!({"files": ["/p/copy.png"], "per_image": "1s"})).unwrap();
    assert_eq!(out["track"], json!(p.video));
    assert_eq!(out["at"], json!(secs(16)));
    assert!(clips(&h.doc(), &p, top).is_empty());
}

#[test]
fn a_refused_placement_leaves_no_asset_behind() {
    let (mut h, p) = H::project();
    let track: TrackId = id(&h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "video"}),
    )["track"]);
    let video = h.add_video("/p/v.mp4", 10, false);
    h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": track, "media": video, "at": "3s"}),
    );
    let (commits, media) = (h.commits(), h.doc().media.len());
    // The second picture would overlap the clip at 3 s on a track that does not ripple.
    assert_invalid(
        slideshow(
            &h,
            json!({"files": ["/p/img2.png", "/p/apple.png"], "per_image": "2s", "at": 0,
                   "track": track}),
        ),
        "/at",
    );
    assert_eq!(h.commits(), commits);
    assert_eq!(h.doc().media.len(), media, "the first picture was not kept");
    assert!(clips(&h.doc(), &p, track).len() == 1);
}

#[test]
fn bad_requests_are_refused_with_the_field_at_fault() {
    let (h, p) = H::project();
    let run = |params: Value| slideshow(&h, params);
    assert_invalid(run(json!({"files": []})), "/files");
    assert_invalid(run(json!({"files": ["p/img2.png"]})), "/files/0");
    assert_invalid(
        run(json!({"files": ["/p/img2.png", "/p/img2.png"]})),
        "/files/1",
    );
    assert_invalid(
        run(json!({"files": ["/p/img2.png"], "per_image": "0s"})),
        "/per_image",
    );
    assert_invalid(
        run(json!({"files": ["/p/img2.png"], "track": p.audio})),
        "/track",
    );
    assert_invalid(run(json!({"files": ["/p/img2.png"], "at": "-1s"})), "/at");
    assert_code(
        run(json!({"files": ["/p/img2.png"], "track": TrackId::from_u128(9)})),
        "not_found",
    );
    assert_code(
        run(json!({"files": ["/p/img2.png"],
                   "sequence": cutweave_model::SequenceId::from_u128(9)})),
        "not_found",
    );
    assert!(h.doc().media.is_empty(), "nothing was imported");
    assert_eq!(h.commits(), 1, "only project.new");

    // Without a sequence there is nowhere to put the pictures.
    let h = H::empty();
    assert_invalid(
        slideshow(&h, json!({"files": ["/p/img2.png"]})),
        "/sequence",
    );
}

#[test]
fn agents_may_only_import_pictures_from_allowed_folders() {
    let (h, _) = H::project();
    let mut ctx = h
        .import_ctx(pictures())
        .with_policy(ImportPolicy::restricted(AllowedRoots::new(["/elsewhere"])));
    let result = h.registry.invoke_engine_json(
        SLIDESHOW,
        json!({"files": ["/p/img2.png"]}),
        &mut ctx,
        Access::STANDARD,
    );
    assert_invalid(result, "/files/0");
    assert!(h.doc().media.is_empty());
}

// ----- media.import_frame --------------------------------------------------------------------

fn import_frame(h: &H, frames: &Arc<FakeFrames>, params: Value) -> Result<Value, CmdError> {
    let mut ctx = h
        .import_ctx(fake_probe())
        .with_frames(Arc::clone(frames) as Arc<dyn cutweave_ops::media::FrameGrabber>);
    h.registry
        .invoke_engine_json(FRAME, params, &mut ctx, Access::STANDARD)
}

#[test]
fn a_frame_becomes_a_picture_in_the_bin_named_after_its_source_and_time() {
    let (h, _) = H::project();
    let before = h.commits();
    let frames = Arc::new(FakeFrames::new(h.doc()));
    let out = import_frame(&h, &frames, json!({"source": "sequence", "at": "f:72"})).unwrap();
    assert_eq!(h.commits(), before + 1, "one transaction, so one undo step");
    let media_id: MediaId = id(&out["media_id"]);
    let doc = h.doc();
    let asset = &doc.media[&media_id];
    assert_eq!(asset.kind, MediaKind::Image);
    // The file is named after the new asset, so replaying the registration is deterministic.
    let path = asset.path_abs.clone().unwrap();
    assert_eq!(path, format!("/project/generated/frames/{media_id}.png"));
    assert_eq!(out["path"], json!(path));
    assert_eq!(out["duplicate"], false);
    assert_eq!(
        out["time"],
        json!(Time::from_flicks(705_600_000 / 30 * 72).unwrap())
    );
    assert_eq!(out["name"], "Sequence 1 00:00:02:12");
    assert_eq!(asset.name.as_deref(), Some("Sequence 1 00:00:02:12"));
    assert!(frames.discarded().is_empty());
}

#[test]
fn a_frame_takes_the_given_name_and_folder() {
    let (mut h, _) = H::project();
    let folder = h.ok("media.create_folder", json!({"name": "Stills"}))["folder_id"].clone();
    let frames = Arc::new(FakeFrames::new(h.doc()));
    let out = import_frame(
        &h,
        &frames,
        json!({"source": "sequence", "at": 0, "name": "  Poster  ", "folder": folder}),
    )
    .unwrap();
    let asset = &h.doc().media[&id::<MediaId>(&out["media_id"])];
    assert_eq!(asset.name.as_deref(), Some("Poster"));
    assert_eq!(out["name"], "Poster");
    assert_eq!(json!(asset.folder), folder);
}

#[test]
fn a_frame_of_a_video_asset_is_named_after_the_asset() {
    let (mut h, _) = H::project();
    let video = h.add_video("/p/Holiday.mp4", 10, true);
    let frames = Arc::new(FakeFrames::new(h.doc()));
    let out = import_frame(&h, &frames, json!({"source": {"media": video}, "at": "1s"})).unwrap();
    assert_eq!(out["name"], "Holiday.mp4 00:00:01:00");
    assert_eq!(out["time"], json!(secs(1)));
    assert_eq!(h.doc().media.len(), 2);
}

#[test]
fn the_same_picture_twice_is_one_asset_and_the_second_file_is_removed() {
    let (h, _) = H::project();
    let frames = Arc::new(FakeFrames::new(h.doc()));
    let first = import_frame(&h, &frames, json!({"source": "sequence", "at": 0})).unwrap();
    let commits = h.commits();
    // The store still sees the document without the first frame, so it "writes" the same
    // file again: the same content as the asset that now exists.
    let again = import_frame(
        &h,
        &frames,
        json!({"source": "sequence", "at": 0, "name": "Again"}),
    )
    .unwrap();
    assert_eq!(again["media_id"], first["media_id"]);
    assert_eq!(again["duplicate"], true);
    assert_eq!(
        again["name"], first["name"],
        "the existing asset keeps its name"
    );
    assert_eq!(again["path"], first["path"]);
    assert_eq!(h.doc().media.len(), 1);
    assert_eq!(h.commits(), commits, "nothing changed");
    assert_eq!(frames.discarded().len(), 1);
}

#[test]
fn import_frame_refuses_what_it_cannot_do_and_removes_the_file_it_wrote() {
    let (mut h, _) = H::project();
    let frames = Arc::new(FakeFrames::new(h.doc()));
    let run = |h: &H, params: Value| import_frame(h, &frames, params);
    assert_invalid(
        run(&h, json!({"source": "sequence", "at": 0, "name": "   "})),
        "/name",
    );
    assert_invalid(run(&h, json!({"source": "sequence", "at": "-1s"})), "/at");
    assert_code(
        run(
            &h,
            json!({"source": {"media": MediaId::from_u128(5)}, "at": 0}),
        ),
        "not_found",
    );
    assert!(
        frames.discarded().is_empty(),
        "nothing was written for those"
    );
    // An unknown bin folder is found only when registering: the file is removed again.
    let missing = cutweave_model::FolderId::from_u128(77);
    assert_code(
        run(
            &h,
            json!({"source": "sequence", "at": 0, "folder": missing}),
        ),
        "not_found",
    );
    assert_eq!(frames.discarded().len(), 1);
    assert!(h.doc().media.is_empty());
    // A still has no frame to take.
    let still = h.add_image("/p/logo.png");
    let frames = Arc::new(FakeFrames::new(h.doc()));
    assert_invalid(
        import_frame(&h, &frames, json!({"source": {"media": still}, "at": 0})),
        "/source/media",
    );

    // Without a renderer the command is unsupported.
    let mut ctx = h.import_ctx(fake_probe());
    let result = h.registry.invoke_engine_json(
        FRAME,
        json!({"source": "sequence", "at": 0}),
        &mut ctx,
        Access::STANDARD,
    );
    assert_code(result, "unsupported");
}
