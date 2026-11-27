// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.05: `timeline.insert` and `timeline.get`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::many_single_char_names,
    clippy::needless_pass_by_value
)]

mod common;

use common::{H, P, assert_code, assert_invalid, group_id, id, item_id, secs};
use cutweave_model::{Item, MediaId, SequenceId, Time, TrackId, TrackKind};
use serde_json::{Value, json};

const FLICKS: i64 = 705_600_000;

fn insert(
    h: &mut H,
    p: P,
    track: TrackId,
    media: MediaId,
    extra: Value,
) -> Result<Value, cutweave_commands::CmdError> {
    let mut params = json!({"sequence": p.sequence, "track": track, "media": media, "at": 0});
    for (k, v) in extra.as_object().unwrap() {
        params[k] = v.clone();
    }
    h.run("timeline.insert", params)
}

fn clip_of(h: &H, p: P, item: &Value) -> cutweave_model::Clip {
    let doc = h.doc();
    let _ = p;
    doc.find_item(&item_id(item))
        .unwrap()
        .item
        .as_clip()
        .unwrap()
        .clone()
}

// ----- timeline.insert ----------------------------------------------------------------------

#[test]
fn insert_video_with_sound_creates_linked_video_and_audio_clips() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, true);
    let out = insert(&mut h, p, p.video, m, json!({"at": "2s"})).unwrap();
    let clips = out["clips"].as_array().unwrap();
    assert_eq!(clips.len(), 2);
    let group = group_id(&out["linked_group"]).expect("linked");
    let (v, a) = (clip_of(&h, p, &clips[0]), clip_of(&h, p, &clips[1]));
    assert_eq!(v.linked_group, Some(group));
    assert_eq!(a.linked_group, Some(group));
    assert_eq!((v.start, v.duration), (secs(2), a.duration));
    assert_eq!((v.start, v.source_in), (a.start, a.source_in));
    assert_eq!(
        v.duration.flicks(),
        10 * FLICKS,
        "defaults to the whole media"
    );
    let doc = h.doc();
    assert!(
        doc.sequences[&p.sequence].tracks[&p.video]
            .items
            .contains(&v.id)
    );
    assert!(
        doc.sequences[&p.sequence].tracks[&p.audio]
            .items
            .contains(&a.id)
    );
    assert_eq!(out["created_tracks"], json!([]));
}

#[test]
fn insert_video_without_sound_or_with_link_false_is_a_single_clip() {
    let (mut h, p) = H::project();
    let silent = h.add_video("/m/silent.mp4", 4, false);
    let noisy = h.add_video("/m/noisy.mp4", 4, true);
    let out = insert(&mut h, p, p.video, silent, json!({})).unwrap();
    assert_eq!(out["clips"].as_array().unwrap().len(), 1);
    assert!(out["linked_group"].is_null());
    let out = insert(
        &mut h,
        p,
        p.video,
        noisy,
        json!({"at": "4s", "link": false}),
    )
    .unwrap();
    assert_eq!(out["clips"].as_array().unwrap().len(), 1);
    assert!(clip_of(&h, p, &out["clips"][0]).linked_group.is_none());
}

#[test]
fn insert_honours_source_in_duration_and_frame_inputs() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, false);
    // 30 fps: frame 45 = 1.5 s; timecode 00:00:02:15 = 2.5 s.
    let out = insert(
        &mut h,
        p,
        p.video,
        m,
        json!({"at": "f:45", "source_in": "00:00:02:15", "duration": "2s"}),
    )
    .unwrap();
    let c = clip_of(&h, p, &out["clips"][0]);
    assert_eq!(c.start.flicks(), 3 * FLICKS / 2);
    assert_eq!(c.source_in.flicks(), 5 * FLICKS / 2);
    assert_eq!(c.duration.flicks(), 2 * FLICKS);
    // Without a duration: the rest of the media.
    let out = insert(
        &mut h,
        p,
        p.video,
        m,
        json!({"at": "20s", "source_in": "4s"}),
    )
    .unwrap();
    assert_eq!(
        clip_of(&h, p, &out["clips"][0]).duration.flicks(),
        6 * FLICKS
    );
}

#[test]
fn insert_audio_media_on_an_audio_track_and_video_media_as_sound_only() {
    let (mut h, p) = H::project();
    let song = h.add_audio("/m/song.wav", 30);
    let video = h.add_video("/m/v.mp4", 10, true);
    let out = insert(&mut h, p, p.audio, song, json!({})).unwrap();
    assert_eq!(out["clips"].as_array().unwrap().len(), 1);
    let out = insert(&mut h, p, p.audio, video, json!({"at": "30s"})).unwrap();
    assert_eq!(out["clips"].as_array().unwrap().len(), 1, "sound only");
    assert!(out["linked_group"].is_null());
    assert_eq!(h.doc().item_count(), 2);
}

/// Frame times of the sequence the project was made with.
fn grid(h: &H, p: P) -> cutweave_model::FrameRate {
    h.doc().sequences[&p.sequence].format.frame_rate
}

#[test]
fn insert_snaps_at_and_duration_to_the_frame_grid_and_reports_them() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, false);
    let frame = FLICKS / 30;
    // 30 fps: frame 3 plus 0.4 of a frame -> frame 3 (down); 4.5 frames -> 5 (tie later).
    let out = insert(
        &mut h,
        p,
        p.video,
        m,
        json!({"at": 3 * frame + frame * 2 / 5, "duration": 9 * frame / 2}),
    )
    .unwrap();
    assert_eq!(out["at"], json!(3 * frame));
    assert_eq!(out["duration"], json!(5 * frame));
    let c = clip_of(&h, p, &out["clips"][0]);
    assert_eq!(
        (c.start.flicks(), c.duration.flicks()),
        (3 * frame, 5 * frame)
    );
    // source_in is a position in the media and keeps its off-grid value.
    let off = frame / 2 + 7;
    let out = insert(
        &mut h,
        p,
        p.video,
        m,
        json!({"at": "5s", "source_in": off, "duration": "1s"}),
    )
    .unwrap();
    assert_eq!(clip_of(&h, p, &out["clips"][0]).source_in.flicks(), off);
}

#[test]
fn insert_duration_is_at_least_one_frame_but_still_positive() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, false);
    let frame = FLICKS / 30;
    let out = insert(&mut h, p, p.video, m, json!({"duration": 3})).unwrap();
    assert_eq!(out["duration"], json!(frame));
    assert_invalid(
        insert(&mut h, p, p.video, m, json!({"duration": 0})),
        "/duration",
    );
    assert_invalid(
        insert(&mut h, p, p.video, m, json!({"duration": -5})),
        "/duration",
    );
    // `at` a few flicks below zero snaps to zero; a whole frame below zero fails.
    let out = insert(
        &mut h,
        p,
        p.video,
        m,
        json!({"at": -3, "duration": "1s", "overlap": "new_track"}),
    )
    .unwrap();
    assert_eq!(out["at"], json!(0));
    assert_invalid(insert(&mut h, p, p.video, m, json!({"at": -frame})), "/at");
}

#[test]
fn insert_on_grid_inputs_are_unchanged() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, false);
    let out = insert(
        &mut h,
        p,
        p.video,
        m,
        json!({"at": "f:45", "duration": "2s"}),
    )
    .unwrap();
    assert_eq!(out["at"], json!(FLICKS * 3 / 2));
    assert_eq!(out["duration"], json!(2 * FLICKS));
    // Defaults derived from the media are not snapped.
    let out = insert(
        &mut h,
        p,
        p.video,
        m,
        json!({"at": "20s", "source_in": "4s"}),
    )
    .unwrap();
    assert_eq!(out["duration"], json!(6 * FLICKS));
}

#[test]
fn insert_on_an_ntsc_sequence_lands_on_the_frame_grid() {
    let (h0, p) = H::project();
    let mut doc = h0.doc();
    doc.sequences
        .get_mut(&p.sequence)
        .unwrap()
        .format
        .frame_rate = cutweave_model::FrameRate::FPS_29_97;
    let mut h = H::with_doc(doc);
    let rate = grid(&h, p);
    let m = h.add_video("/m/v.mp4", 100, true);
    let off = rate.frame_to_time(100).unwrap().flicks() + 5;
    let len = rate.frame_to_time(40).unwrap().flicks() - 9;
    let out = insert(&mut h, p, p.video, m, json!({"at": off, "duration": len})).unwrap();
    let want_at = rate.frame_to_time(100).unwrap();
    let want_len = rate.frame_to_time(40).unwrap();
    assert_eq!(out["at"], json!(want_at.flicks()));
    assert_eq!(out["duration"], json!(want_len.flicks()));
    for c in out["clips"].as_array().unwrap() {
        let c = clip_of(&h, p, c);
        assert_eq!((c.start, c.duration.flicks()), (want_at, want_len.flicks()));
        assert_eq!(
            rate.snap_to_frame(c.start, cutweave_model::Rounding::Nearest)
                .unwrap(),
            c.start
        );
    }
    // An on-grid input is returned as given.
    let at = rate.frame_to_time(300).unwrap();
    let out = insert(
        &mut h,
        p,
        p.video,
        m,
        json!({"at": "f:300", "duration": "f:30"}),
    )
    .unwrap();
    assert_eq!(out["at"], json!(at.flicks()));
    assert_eq!(
        out["duration"],
        json!(rate.frame_to_time(30).unwrap().flicks())
    );
}

#[test]
fn insert_an_image_defaults_to_five_seconds() {
    let (mut h, p) = H::project();
    let img = h.add_image("/m/i.png");
    let out = insert(&mut h, p, p.video, img, json!({})).unwrap();
    let c = clip_of(&h, p, &out["clips"][0]);
    assert_eq!(c.duration.flicks(), 5 * FLICKS);
    let out = insert(
        &mut h,
        p,
        p.video,
        img,
        json!({"at": "5s", "duration": "f:30"}),
    )
    .unwrap();
    assert_eq!(clip_of(&h, p, &out["clips"][0]).duration.flicks(), FLICKS);
}

#[test]
fn a_still_gets_the_projects_still_duration_and_existing_clips_keep_theirs() {
    // ADR-0035: insert, change the setting, insert again.
    let mut h = H::empty();
    let out = h.ok(
        "project.new",
        json!({"name": "Stills", "still_duration": "8s"}),
    );
    let p = P {
        sequence: id(&out["sequence"]),
        video: id(&out["video_track"]),
        audio: id(&out["audio_track"]),
    };
    assert_eq!(h.doc().settings.still_duration().flicks(), 8 * FLICKS);
    let img = h.add_image("/m/i.png");
    let first = insert(&mut h, p, p.video, img, json!({})).unwrap();
    assert_eq!(
        clip_of(&h, p, &first["clips"][0]).duration.flicks(),
        8 * FLICKS
    );

    let mut doc = h.doc();
    doc.settings.still_duration = Some(cutweave_model::Duration::from_secs(2).unwrap());
    h.replace_doc(doc);
    let second = insert(&mut h, p, p.video, img, json!({"at": "20s"})).unwrap();
    assert_eq!(
        clip_of(&h, p, &second["clips"][0]).duration.flicks(),
        2 * FLICKS
    );
    assert_eq!(
        clip_of(&h, p, &first["clips"][0]).duration.flicks(),
        8 * FLICKS,
        "unchanged"
    );
    // A length shorter than a frame is one frame.
    let mut doc = h.doc();
    doc.settings.still_duration = Some(cutweave_model::Duration::from_flicks(1).unwrap());
    h.replace_doc(doc);
    let third = insert(&mut h, p, p.video, img, json!({"at": "40s"})).unwrap();
    assert_eq!(
        clip_of(&h, p, &third["clips"][0]).duration.flicks(),
        FLICKS / 30
    );
}

#[test]
fn insert_unknown_sequence_track_or_media_is_not_found() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, false);
    let mut call = |params: Value| h.run("timeline.insert", params);
    assert_code(
        call(json!({"sequence": SequenceId::from_u128(1), "track": p.video, "media": m, "at": 0})),
        "not_found",
    );
    assert_code(
        call(json!({"sequence": p.sequence, "track": TrackId::from_u128(1), "media": m, "at": 0})),
        "not_found",
    );
    assert_code(
        call(
            json!({"sequence": p.sequence, "track": p.video, "media": MediaId::from_u128(1), "at": 0}),
        ),
        "not_found",
    );
}

#[test]
fn insert_rejects_a_track_of_another_sequence_with_its_path() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, false);
    let mut doc = h.doc();
    let mut other = cutweave_model::Sequence::new(
        SequenceId::from_u128(77),
        "Other",
        doc.settings.default_format.clone(),
    );
    let other_track = TrackId::from_u128(78);
    other.insert_track(
        0,
        cutweave_model::Track::new(other_track, TrackKind::Video, "X"),
    );
    doc.sequences.insert(other.id, other);
    let mut h = H::with_doc(doc);
    assert_invalid(
        h.run(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": other_track, "media": m, "at": 0}),
        ),
        "/track",
    );
}

#[test]
fn insert_overlap_is_invalid_at_at_by_default() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, false);
    insert(&mut h, p, p.video, m, json!({"at": "2s", "duration": "4s"})).unwrap();
    // Overlaps from the left, inside, and right; touching ends are fine.
    for at in ["0s", "3s", "5.5s"] {
        let r = insert(&mut h, p, p.video, m, json!({"at": at, "duration": "3s"}));
        assert_invalid(r, "/at");
    }
    insert(&mut h, p, p.video, m, json!({"at": "6s", "duration": "1s"})).unwrap();
    insert(&mut h, p, p.video, m, json!({"at": "1s", "duration": "1s"})).unwrap();
    assert_eq!(h.doc().item_count(), 3);
}

#[test]
fn insert_validates_times_media_bounds_and_track_kind() {
    let (mut h, p) = H::project();
    let v = h.add_video("/m/v.mp4", 10, false);
    let song = h.add_audio("/m/s.wav", 10);
    assert_invalid(insert(&mut h, p, p.video, v, json!({"at": "-1s"})), "/at");
    assert_invalid(
        insert(&mut h, p, p.video, v, json!({"at": "nonsense"})),
        "/at",
    );
    assert_invalid(
        insert(&mut h, p, p.video, v, json!({"source_in": "-1s"})),
        "/source_in",
    );
    assert_invalid(
        insert(&mut h, p, p.video, v, json!({"source_in": "10s"})),
        "/source_in",
    );
    assert_invalid(
        insert(&mut h, p, p.video, v, json!({"duration": 0})),
        "/duration",
    );
    assert_invalid(
        insert(&mut h, p, p.video, v, json!({"duration": "11s"})),
        "/duration",
    );
    assert_invalid(
        insert(
            &mut h,
            p,
            p.video,
            v,
            json!({"source_in": "5s", "duration": "6s"}),
        ),
        "/duration",
    );
    assert_invalid(insert(&mut h, p, p.video, song, json!({})), "/track");
    assert_eq!(
        h.commits(),
        3,
        "only project.new and the two registrations committed"
    );
}

#[test]
fn insert_without_a_known_media_length_needs_a_duration() {
    let (mut h, p) = H::project();
    let mut probed = cutweave_ops::media::FakeProbe::audio("/m/live.wav", 1);
    probed.probe.duration = None;
    let out = h.ok(
        "media.register",
        json!({"path": "/m/live.wav", "probe": probed.probe, "content_hash": "live"}),
    );
    let m: MediaId = id(&out["media_id"]);
    assert_invalid(insert(&mut h, p, p.audio, m, json!({})), "/duration");
    insert(&mut h, p, p.audio, m, json!({"duration": "3s"})).unwrap();
    let img = h.add_image("/m/i.png");
    assert_invalid(
        insert(&mut h, p, p.video, img, json!({"duration": "0s"})),
        "/duration",
    );
}

#[test]
fn insert_on_a_locked_track_fails_with_locked() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, true);
    h.ok("track.set", json!({"track": p.video, "locked": true}));
    assert_code(insert(&mut h, p, p.video, m, json!({})), "locked");
    assert_code(
        h.validate(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": p.video, "media": m, "at": 0}),
        )
        .map(|()| json!(null)),
        "locked",
    );
    assert_eq!(h.doc().item_count(), 0);
}

#[test]
fn insert_skips_a_locked_audio_track_when_linking() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, true);
    h.ok("track.set", json!({"track": p.audio, "locked": true}));
    let out = insert(&mut h, p, p.video, m, json!({})).unwrap();
    assert_eq!(out["clips"].as_array().unwrap().len(), 1, "video only");
    assert!(out["linked_group"].is_null());
}

#[test]
fn insert_links_to_the_first_free_audio_track() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, true);
    let a2: TrackId = id(&h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "audio"}),
    )["track"]);
    let song = h.add_audio("/m/s.wav", 20);
    insert(&mut h, p, p.audio, song, json!({"at": 0})).unwrap(); // A1 busy 0..20
    let out = insert(&mut h, p, p.video, m, json!({"at": "5s"})).unwrap();
    let audio_clip = item_id(&out["clips"][1]);
    let doc = h.doc();
    assert!(
        doc.sequences[&p.sequence].tracks[&a2]
            .items
            .contains(&audio_clip),
        "A2 was free"
    );
}

#[test]
fn insert_new_track_policy_creates_a_track_above_for_overlays() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, false);
    insert(&mut h, p, p.video, m, json!({"duration": "5s"})).unwrap();
    let out = insert(
        &mut h,
        p,
        p.video,
        m,
        json!({"at": "1s", "duration": "2s", "overlap": "new_track"}),
    )
    .unwrap();
    let created: Vec<TrackId> = id(&out["created_tracks"]);
    assert_eq!(created.len(), 1);
    let doc = h.doc();
    let seq = &doc.sequences[&p.sequence];
    let order: Vec<_> = seq.track_order.iter().copied().collect();
    assert_eq!(
        order,
        [created[0], p.video, p.audio],
        "new track directly above the target"
    );
    assert_eq!(seq.tracks[&created[0]].kind, TrackKind::Video);
    assert_eq!(seq.tracks[&created[0]].name, "V2");
    assert!(
        seq.tracks[&created[0]]
            .items
            .contains(&item_id(&out["clips"][0]))
    );
}

#[test]
fn insert_new_track_policy_without_overlap_creates_nothing() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, false);
    let out = insert(&mut h, p, p.video, m, json!({"overlap": "new_track"})).unwrap();
    assert_eq!(out["created_tracks"], json!([]));
    assert_invalid(
        insert(&mut h, p, p.video, m, json!({"overlap": "sideways"})),
        "/overlap",
    );
}

#[test]
fn insert_new_track_policy_also_makes_a_busy_audio_partner_track() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, true);
    insert(&mut h, p, p.video, m, json!({})).unwrap();
    // Video track busy and audio track busy: both get a new track above.
    let out = insert(
        &mut h,
        p,
        p.video,
        m,
        json!({"at": "1s", "overlap": "new_track"}),
    )
    .unwrap();
    let created: Vec<TrackId> = id(&out["created_tracks"]);
    assert_eq!(created.len(), 2);
    let doc = h.doc();
    let kinds: Vec<_> = created
        .iter()
        .map(|t| doc.sequences[&p.sequence].tracks[t].kind)
        .collect();
    assert_eq!(kinds, [TrackKind::Video, TrackKind::Audio]);
    assert!(group_id(&out["linked_group"]).is_some());
}

#[test]
fn insert_audio_stays_reject_when_the_partner_track_is_busy_and_policy_is_reject() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, true);
    let song = h.add_audio("/m/s.wav", 20);
    insert(&mut h, p, p.audio, song, json!({})).unwrap();
    let out = insert(&mut h, p, p.video, m, json!({})).unwrap();
    assert_eq!(
        out["clips"].as_array().unwrap().len(),
        1,
        "no free audio track: video only"
    );
}

#[test]
fn insert_rejects_unknown_params_and_bad_ids_with_paths() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, false);
    assert_invalid(
        h.run(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": p.video, "media": m, "at": 0, "mode": "x"}),
        ),
        "/mode",
    );
    assert_invalid(
        h.run(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": "nope", "media": m, "at": 0}),
        ),
        "/track",
    );
    assert_invalid(
        h.run("timeline.insert", json!({"sequence": p.sequence})),
        "",
    );
}

#[test]
fn insert_undo_redo_identity_with_linked_clips_and_new_tracks() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, true);
    insert(&mut h, p, p.video, m, json!({})).unwrap();
    insert(
        &mut h,
        p,
        p.video,
        m,
        json!({"at": "1s", "overlap": "new_track"}),
    )
    .unwrap();
    let c = h.last.as_ref().unwrap();
    assert_eq!(c.base.item_count(), 2);
    assert_eq!(c.doc.item_count(), 4);
    assert_eq!(c.base.sequences[&p.sequence].tracks.len(), 2);
    assert_eq!(c.doc.sequences[&p.sequence].tracks.len(), 4);
}

#[test]
fn insert_results_are_valid_documents_with_exact_clip_fields() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, true);
    let out = insert(
        &mut h,
        p,
        p.video,
        m,
        json!({"at": "1s", "source_in": "2s", "duration": "3s"}),
    )
    .unwrap();
    let doc = h.doc();
    doc.validate().unwrap();
    for c in out["clips"].as_array().unwrap() {
        let item = doc.find_item(&item_id(c)).unwrap().item;
        let Item::Clip(clip) = item else { panic!() };
        assert_eq!(clip.media, m);
        assert!(clip.enabled && clip.name.is_none());
        assert_eq!(clip.start, Time::from_flicks(FLICKS).unwrap());
    }
}

// ----- timeline.get -------------------------------------------------------------------------

fn fill(h: &mut H, p: P, clips: i64) {
    let m = h.add_video("/m/v.mp4", 3600, false);
    for i in 0..clips {
        insert(
            h,
            p,
            p.video,
            m,
            json!({"at": format!("{}s", i * 2), "duration": "1s"}),
        )
        .unwrap();
    }
}

#[test]
fn get_depth_zero_is_the_sequence_summary() {
    let (mut h, p) = H::project();
    fill(&mut h, p, 3);
    let out = h
        .query("timeline.get", json!({"sequence": p.sequence, "depth": 0}))
        .unwrap();
    assert_eq!(out["id"], json!(p.sequence));
    assert_eq!(out["track_count"], 2);
    assert_eq!(out["item_count"], 3);
    assert_eq!(out["duration"], 5 * FLICKS);
    assert!(out.get("tracks").is_none());
    assert_eq!(out["format"]["width"], 1920);
}

#[test]
fn get_depth_one_lists_track_summaries_in_display_order() {
    let (mut h, p) = H::project();
    fill(&mut h, p, 2);
    let out = h
        .query("timeline.get", json!({"sequence": p.sequence}))
        .unwrap();
    let tracks = out["tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 2);
    assert_eq!(tracks[0]["id"], json!(p.video));
    assert_eq!(tracks[0]["item_count"], 2);
    assert_eq!(tracks[0]["end"], 3 * FLICKS);
    assert_eq!(tracks[1]["kind"], "audio");
    assert!(tracks[0].get("items").is_none(), "depth 1 has no items");
}

#[test]
fn get_depth_one_of_a_100_clip_timeline_is_under_2_kb() {
    let (mut h, p) = H::project();
    fill(&mut h, p, 100);
    let out = h
        .query("timeline.get", json!({"sequence": p.sequence, "depth": 1}))
        .unwrap();
    let size = serde_json::to_vec(&out).unwrap().len();
    assert!(size < 2048, "depth 1 summary was {size} bytes");
    assert_eq!(out["item_count"], 100);
}

#[test]
fn get_depth_two_includes_items_in_start_order() {
    let (mut h, p) = H::project();
    fill(&mut h, p, 3);
    let out = h
        .query("timeline.get", json!({"sequence": p.sequence, "depth": 2}))
        .unwrap();
    let items = out["tracks"][0]["items"].as_array().unwrap();
    let starts: Vec<i64> = items.iter().map(|i| i["start"].as_i64().unwrap()).collect();
    assert_eq!(starts, [0, 2 * FLICKS, 4 * FLICKS]);
    assert_eq!(items[0]["kind"], "clip");
    assert!(items[0]["media"].is_string());
}

/// WP-2I.01: `limit` pages the items of a depth-2 read; `next_cursor` continues it, and every
/// item comes back exactly once, in track order then start order.
#[test]
fn get_pages_items_with_limit_and_cursor() {
    let (mut h, p) = H::project();
    fill(&mut h, p, 7);
    let all = h
        .query("timeline.get", json!({"sequence": p.sequence, "depth": 2}))
        .unwrap();
    assert!(all.get("next_cursor").is_none(), "no limit: everything");
    let ids = |out: &Value| -> Vec<Value> {
        out["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|t| {
                t["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|i| i["id"].clone())
            })
            .collect()
    };
    let every = ids(&all);
    assert_eq!(every.len(), 7);

    let mut seen = Vec::new();
    let mut cursor = Value::Null;
    let mut pages = 0;
    loop {
        let mut args = json!({"sequence": p.sequence, "depth": 2, "limit": 3});
        if !cursor.is_null() {
            args["cursor"] = cursor.clone();
        }
        let page = h.query("timeline.get", args).unwrap();
        let got = ids(&page);
        assert!(got.len() <= 3);
        assert_eq!(page["item_count"], 7, "counts describe the whole sequence");
        assert_eq!(page["tracks"][0]["item_count"], 7);
        seen.extend(got);
        pages += 1;
        match page.get("next_cursor") {
            Some(next) => cursor = next.clone(),
            None => break,
        }
        assert!(pages < 10);
    }
    assert_eq!(pages, 3);
    assert_eq!(seen, every);
}

/// WP-2I.01: `range` lists only the items that overlap `[from, to)`.
#[test]
fn get_range_lists_the_items_that_overlap_it() {
    let (mut h, p) = H::project();
    // Clips of 1 s at 0, 2, 4, 6 and 8 s.
    fill(&mut h, p, 5);
    let starts = |args: Value| -> Vec<i64> {
        let out = h.query("timeline.get", args).unwrap();
        out["tracks"][0]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["start"].as_i64().unwrap() / FLICKS)
            .collect()
    };
    let seq = json!(p.sequence);
    assert_eq!(
        starts(json!({"sequence": seq, "depth": 2, "range": {"from": "2.5s", "to": "6s"}})),
        [2, 4],
        "the clip at 2 s reaches into the range; the one at 6 s starts at its end"
    );
    assert_eq!(
        starts(json!({"sequence": seq, "depth": 2, "range": {"from": "6s"}})),
        [6, 8]
    );
    assert_eq!(
        starts(json!({"sequence": seq, "depth": 2, "range": {"to": "f:30"}})),
        [0]
    );
    // A range and a limit together: the cursor continues inside the range.
    let first = h
        .query(
            "timeline.get",
            json!({"sequence": seq, "depth": 2, "range": {"from": "2s"}, "limit": 2}),
        )
        .unwrap();
    assert_eq!(first["tracks"][0]["items"].as_array().unwrap().len(), 2);
    assert_eq!(
        starts(
            json!({"sequence": seq, "depth": 2, "range": {"from": "2s"}, "limit": 2,
                      "cursor": first["next_cursor"]})
        ),
        [6, 8]
    );
}

#[test]
fn get_paging_params_are_validated_with_paths() {
    let (mut h, p) = H::project();
    fill(&mut h, p, 2);
    let seq = json!(p.sequence);
    for (args, path) in [
        (json!({"sequence": seq, "depth": 2, "limit": 0}), "/limit"),
        (
            json!({"sequence": seq, "depth": 2, "limit": 1001}),
            "/limit",
        ),
        (json!({"sequence": seq, "limit": 5}), "/limit"),
        (
            json!({"sequence": seq, "depth": 2, "range": {"from": "4s", "to": "2s"}}),
            "/range/to",
        ),
        (
            json!({"sequence": seq, "depth": 1, "range": {"from": "4s"}}),
            "/range",
        ),
        (
            json!({"sequence": seq, "depth": 2, "limit": 1,
                   "cursor": cutweave_model::ItemId::from_u128(77)}),
            "/cursor",
        ),
    ] {
        assert_invalid(h.query("timeline.get", args), path);
    }
}

/// WP-2I.01: an item's own name is listed (not the media's, which `media.list` has).
#[test]
fn get_lists_an_items_own_name() {
    let (mut h, p) = H::project();
    fill(&mut h, p, 2);
    let out = h
        .query("timeline.get", json!({"sequence": p.sequence, "depth": 2}))
        .unwrap();
    let first = out["tracks"][0]["items"][0].clone();
    assert!(first.get("name").is_none(), "unnamed: no field");
    h.run(
        "item.set_props",
        json!({"item": first["id"], "props": {"name": "Intro"}}),
    )
    .unwrap();
    let out = h
        .query("timeline.get", json!({"sequence": p.sequence, "depth": 2}))
        .unwrap();
    assert_eq!(out["tracks"][0]["items"][0]["name"], "Intro");
}

/// WP-2I.01: the default read of a 400-item sequence is an overview under 4k tokens
/// (`ceil(bytes / 4)`, the tool-list estimate), and so is each page of 50 items.
#[test]
fn get_default_of_a_400_item_sequence_is_under_4k_tokens() {
    let (mut h, p) = H::project();
    fill(&mut h, p, 400);
    let tokens = |v: &Value| serde_json::to_vec(v).unwrap().len().div_ceil(4);
    let overview = h
        .query("timeline.get", json!({"sequence": p.sequence}))
        .unwrap();
    assert_eq!(overview["item_count"], 400);
    assert!(tokens(&overview) < 4000, "{} tokens", tokens(&overview));
    let page = h
        .query(
            "timeline.get",
            json!({"sequence": p.sequence, "depth": 2, "limit": 50}),
        )
        .unwrap();
    assert_eq!(page["tracks"][0]["items"].as_array().unwrap().len(), 50);
    assert!(page.get("next_cursor").is_some());
    assert!(tokens(&page) < 4000, "{} tokens", tokens(&page));
}

#[test]
fn get_fields_limit_the_sections() {
    let (h, p) = H::project();
    let out = h
        .query(
            "timeline.get",
            json!({"sequence": p.sequence, "fields": ["name"]}),
        )
        .unwrap();
    assert!(out.get("name").is_some());
    for gone in ["format", "duration", "tracks"] {
        assert!(out.get(gone).is_none(), "{gone}");
    }
    assert_eq!(out["track_count"], 2, "counts are always there");
}

#[test]
fn get_validates_with_field_paths_and_unknown_sequence_is_not_found() {
    let (h, p) = H::project();
    assert_code(
        h.query(
            "timeline.get",
            json!({"sequence": SequenceId::from_u128(9)}),
        ),
        "not_found",
    );
    assert_invalid(
        h.query("timeline.get", json!({"sequence": p.sequence, "depth": 3})),
        "/depth",
    );
    assert_invalid(
        h.query(
            "timeline.get",
            json!({"sequence": p.sequence, "fields": ["name", "bogus"]}),
        ),
        "/fields/1",
    );
    assert_invalid(
        h.query("timeline.get", json!({"sequence": "x"})),
        "/sequence",
    );
}
