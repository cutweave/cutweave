// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2A.02: the document invariants of the timeline spec that `Project::validate` enforces
//! (INV-6 and INV-7 magnetic track, INV-8 aligned linked groups, INV-9 attachments) plus the
//! checks of the v2 additions (markers, in/out range, track heights).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use cutweave_model::{
    Clip, Duration, FrameRate, GroupId, IdGen, InOut, Invariant, ItemId, Marker, MarkerId,
    MediaAsset, MediaKind, ProbeInfo, Project, SampleRate, SeededGen, Sequence, SequenceFormat,
    Time, Track, TrackId, TrackKind,
};

const FRAME: i64 = 23_520_000; // 30 fps

fn at(frames: i64) -> Time {
    Time::from_flicks(frames * FRAME).unwrap()
}

fn len(frames: i64) -> Duration {
    Duration::from_flicks(frames * FRAME).unwrap()
}

/// A project with one video media and one audio media, one sequence with V2, V1 (video) and A1
/// (audio) tracks; ids are `1..` by role.
struct Doc {
    project: Project,
    seq: cutweave_model::SequenceId,
    v1: TrackId,
    v2: TrackId,
    a1: TrackId,
    video: cutweave_model::MediaId,
    audio: cutweave_model::MediaId,
    ids: SeededGen,
}

impl Doc {
    fn new() -> Doc {
        let mut ids = SeededGen::new(7);
        let mut project = Project::new("Invariants");
        let probe = |secs: i64| ProbeInfo {
            duration: Some(Duration::from_secs(secs).unwrap()),
            ..ProbeInfo::default()
        };
        let video = MediaAsset::file(ids.mint(), MediaKind::Video, "/v.mp4", probe(60));
        let audio = MediaAsset::file(ids.mint(), MediaKind::Audio, "/a.wav", probe(60));
        let mut seq = Sequence::new(
            ids.mint(),
            "Main",
            SequenceFormat::new(1920, 1080, FrameRate::FPS_30, SampleRate::HZ_48_000),
        );
        let (v2, v1, a1) = (ids.mint(), ids.mint(), ids.mint());
        seq.insert_track(0, Track::new(v2, TrackKind::Video, "V2"));
        seq.insert_track(1, Track::new(v1, TrackKind::Video, "V1"));
        seq.insert_track(2, Track::new(a1, TrackKind::Audio, "A1"));
        let doc = Doc {
            seq: seq.id,
            v1,
            v2,
            a1,
            video: video.id,
            audio: audio.id,
            ids,
            project: {
                project.media.insert(video.id, video);
                project.media.insert(audio.id, audio);
                project.sequences.insert(seq.id, seq);
                project
            },
        };
        assert_eq!(doc.project.violations(), vec![]);
        doc
    }

    fn clip(
        &mut self,
        track: TrackId,
        media: cutweave_model::MediaId,
        start: i64,
        dur: i64,
    ) -> ItemId {
        let id: ItemId = self.ids.mint();
        let clip = Clip::new(id, media, at(start), len(dur));
        let seq = self.project.sequences.get_mut(&self.seq).unwrap();
        seq.track_mut(&track).unwrap().items.insert(clip.into());
        id
    }

    fn edit_item(
        &mut self,
        track: TrackId,
        item: ItemId,
        f: impl FnOnce(&mut cutweave_model::Item),
    ) {
        let seq = self.project.sequences.get_mut(&self.seq).unwrap();
        seq.track_mut(&track)
            .unwrap()
            .items
            .update(&item, f)
            .unwrap();
    }

    fn track(&mut self, track: TrackId) -> &mut Track {
        self.project
            .sequences
            .get_mut(&self.seq)
            .unwrap()
            .track_mut(&track)
            .unwrap()
    }

    fn violations(&self) -> Vec<Invariant> {
        self.project.violations()
    }
}

// ----- INV-6 / INV-7 -------------------------------------------------------------------------

#[test]
fn a_gap_free_magnetic_video_track_is_valid() {
    let mut d = Doc::new();
    d.clip(d.v1, d.video, 0, 30);
    d.clip(d.v1, d.video, 30, 15);
    d.track(d.v1).magnetic = true;
    assert_eq!(d.violations(), vec![]);
    // An empty magnetic track is valid.
    d.track(d.v1).items = cutweave_model::TrackItems::new();
    assert_eq!(d.violations(), vec![]);
}

#[test]
fn inv6_two_magnetic_tracks_are_invalid() {
    let mut d = Doc::new();
    d.track(d.v1).magnetic = true;
    d.track(d.v2).magnetic = true;
    let v = d.violations();
    assert!(
        matches!(v.as_slice(), [Invariant::MagneticTrack { track, .. }] if *track == d.v1),
        "{v:?}"
    );
}

#[test]
fn inv6_an_audio_track_cannot_be_magnetic() {
    let mut d = Doc::new();
    d.track(d.a1).magnetic = true;
    let v = d.violations();
    assert!(
        matches!(v.as_slice(), [Invariant::MagneticTrack { track, reason, .. }]
            if *track == d.a1 && reason.contains("audio")),
        "{v:?}"
    );
}

#[test]
fn inv7_a_magnetic_track_must_start_at_zero_and_have_no_gap() {
    let mut d = Doc::new();
    let late = d.clip(d.v1, d.video, 5, 30);
    d.track(d.v1).magnetic = true;
    let v = d.violations();
    assert!(
        matches!(v.as_slice(), [Invariant::MagneticTrack { reason, .. }] if reason.contains(&late.to_string())),
        "{v:?}"
    );
    d.edit_item(d.v1, late, |i| i.set_start(at(0)));
    assert_eq!(d.violations(), vec![]);
    d.clip(d.v1, d.video, 31, 5);
    assert_eq!(d.violations().len(), 1);
}

// ----- INV-8 ---------------------------------------------------------------------------------

#[test]
fn inv8_aligned_groups_are_valid() {
    let mut d = Doc::new();
    let g: GroupId = d.ids.mint();
    let v = d.clip(d.v1, d.video, 10, 30);
    let a = d.clip(d.a1, d.audio, 10, 30);
    d.edit_item(d.v1, v, |i| i.set_linked_group(Some(g)));
    d.edit_item(d.a1, a, |i| i.set_linked_group(Some(g)));
    assert_eq!(d.violations(), vec![]);
}

#[test]
fn inv8_a_single_member_group_is_invalid() {
    let mut d = Doc::new();
    let g: GroupId = d.ids.mint();
    let v = d.clip(d.v1, d.video, 10, 30);
    d.edit_item(d.v1, v, |i| i.set_linked_group(Some(g)));
    assert_eq!(
        d.violations(),
        vec![Invariant::LinkedGroupTooSmall {
            group: g,
            members: 1
        }]
    );
}

#[test]
fn inv8_members_must_share_start_and_duration() {
    let mut d = Doc::new();
    let g: GroupId = d.ids.mint();
    let v = d.clip(d.v1, d.video, 10, 30);
    let a = d.clip(d.a1, d.audio, 10, 29);
    d.edit_item(d.v1, v, |i| i.set_linked_group(Some(g)));
    d.edit_item(d.a1, a, |i| i.set_linked_group(Some(g)));
    let violations = d.violations();
    assert!(
        matches!(violations.as_slice(), [Invariant::LinkedGroupMisaligned { group, .. }] if *group == g),
        "{violations:?}"
    );
    d.edit_item(d.a1, a, |i| i.set_duration(len(30)));
    d.edit_item(d.a1, a, |i| i.set_start(at(11)));
    assert_eq!(
        d.violations().len(),
        1,
        "a different start is misaligned too"
    );
}

// ----- INV-9 ---------------------------------------------------------------------------------

/// A parent on V1 `[10, 40)` and a free child on V2 starting at 20.
fn attached() -> (Doc, ItemId, ItemId) {
    let mut d = Doc::new();
    let parent = d.clip(d.v1, d.video, 10, 30);
    let child = d.clip(d.v2, d.video, 20, 5);
    d.edit_item(d.v2, child, |i| i.set_attached_to(Some(parent)));
    (d, parent, child)
}

fn attachment_reason(d: &Doc) -> String {
    match d.violations().as_slice() {
        [Invariant::Attachment { reason, .. }] => reason.clone(),
        other => panic!("expected one attachment violation, got {other:?}"),
    }
}

#[test]
fn inv9_a_child_inside_its_parent_is_valid() {
    let (d, _, _) = attached();
    assert_eq!(d.violations(), vec![]);
}

#[test]
fn inv9_the_child_must_start_inside_the_parents_range() {
    let (mut d, parent, child) = attached();
    d.edit_item(d.v2, child, |i| i.set_start(at(40)));
    assert!(attachment_reason(&d).contains("outside"));
    d.edit_item(d.v2, child, |i| i.set_start(at(9)));
    assert!(attachment_reason(&d).contains("outside"));
    d.edit_item(d.v2, child, |i| i.set_start(at(39)));
    assert_eq!(d.violations(), vec![]);
    d.edit_item(d.v2, child, |i| i.set_start(at(10)));
    assert_eq!(d.violations(), vec![], "the parent's own start is inside");
    let _ = parent;
}

#[test]
fn inv9_the_parent_must_exist_in_the_same_sequence() {
    let (mut d, parent, _) = attached();
    d.track(d.v1).items.remove(&parent);
    assert!(attachment_reason(&d).contains("same sequence"));
}

#[test]
fn inv9_attachments_are_one_level_deep() {
    let (mut d, parent, child) = attached();
    let grandparent = d.clip(d.v2, d.video, 50, 5);
    // The parent is attached too.
    d.edit_item(d.v1, parent, |i| i.set_attached_to(Some(grandparent)));
    let reasons: Vec<String> = d
        .violations()
        .into_iter()
        .filter_map(|v| match v {
            Invariant::Attachment { reason, .. } => Some(reason),
            _ => None,
        })
        .collect();
    assert!(
        reasons.iter().any(|r| r.contains("itself attached")),
        "{reasons:?}"
    );
    assert!(
        reasons.iter().any(|r| r.contains("itself a parent")),
        "{reasons:?}"
    );
    let _ = child;
}

#[test]
fn inv9_the_child_is_on_another_track_and_not_linked_to_its_parent() {
    let (mut d, parent, child) = attached();
    // Same track: put the child on the parent's track beyond the parent.
    d.track(d.v2).items.remove(&child);
    let mut moved = Clip::new(child, d.video, at(50), len(5));
    moved.attached_to = Some(parent);
    d.track(d.v1).items.insert(moved.into());
    let reasons: Vec<String> = d
        .violations()
        .into_iter()
        .filter_map(|v| match v {
            Invariant::Attachment { reason, .. } => Some(reason),
            _ => None,
        })
        .collect();
    assert!(
        reasons.iter().any(|r| r.contains("parent's track")),
        "{reasons:?}"
    );

    // Linked to the parent: one group, aligned (so only INV-9 speaks).
    let (mut d, parent, child) = attached();
    let g: GroupId = d.ids.mint();
    d.edit_item(d.v1, parent, |i| i.set_linked_group(Some(g)));
    d.edit_item(d.v2, child, |i| {
        i.set_linked_group(Some(g));
        i.set_start(at(10));
        i.set_duration(len(30));
    });
    let reasons: Vec<String> = d
        .violations()
        .into_iter()
        .filter_map(|v| match v {
            Invariant::Attachment { reason, .. } => Some(reason),
            _ => None,
        })
        .collect();
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("linked to its own parent")),
        "{reasons:?}"
    );
}

#[test]
fn inv9_linked_partners_of_a_child_follow_the_same_parent() {
    let (mut d, parent, child) = attached();
    let g: GroupId = d.ids.mint();
    let partner = d.clip(d.a1, d.audio, 20, 5);
    d.edit_item(d.v2, child, |i| i.set_linked_group(Some(g)));
    d.edit_item(d.a1, partner, |i| i.set_linked_group(Some(g)));
    assert!(attachment_reason(&d).contains("not attached to the same parent"));
    d.edit_item(d.a1, partner, |i| i.set_attached_to(Some(parent)));
    assert_eq!(d.violations(), vec![]);
}

// ----- markers, in/out, heights ----------------------------------------------------------------

#[test]
fn markers_and_in_out_are_checked() {
    let mut d = Doc::new();
    let seq = d.project.sequences.get_mut(&d.seq).unwrap();
    let id = MarkerId::from_u128(1);
    let mut m = Marker::new(id, at(3));
    m.duration = Some(len(10));
    seq.markers.insert(id, m.clone());
    seq.in_out = Some(InOut::new(at(2), at(9)));
    assert_eq!(d.violations(), vec![]);

    let seq = d.project.sequences.get_mut(&d.seq).unwrap();
    seq.markers.get_mut(&id).unwrap().duration = Some(Duration::ZERO);
    seq.markers.insert(
        MarkerId::from_u128(2),
        Marker::new(MarkerId::from_u128(3), at(0)),
    );
    seq.in_out = Some(InOut::new(at(9), at(9)));
    let paths: Vec<String> = d
        .violations()
        .into_iter()
        .map(|v| match v {
            Invariant::InvalidValue { path, .. } => path,
            Invariant::KeyMismatch { path, .. } => path,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(paths.len(), 3, "{paths:?}");
    assert!(paths.iter().any(|p| p.ends_with("/in_out")));
    assert!(paths.iter().any(|p| p.ends_with("/markers")));
}

#[test]
fn item_markers_are_checked_too() {
    let mut d = Doc::new();
    let item = d.clip(d.v1, d.video, 0, 30);
    d.edit_item(d.v1, item, |i| {
        let mut m = Marker::new(MarkerId::from_u128(9), Time::ZERO);
        m.duration = Some(Duration::ZERO);
        i.markers_mut().insert(m.id, m);
    });
    assert!(matches!(
        d.violations().as_slice(),
        [Invariant::InvalidValue { path, .. }] if path.contains("/markers/")
    ));
}

#[test]
fn track_heights_are_bounded() {
    let mut d = Doc::new();
    d.track(d.v1).height = Some(64);
    assert_eq!(d.violations(), vec![]);
    for bad in [0, 7, 513] {
        d.track(d.v1).height = Some(bad);
        assert!(
            matches!(d.violations().as_slice(), [Invariant::InvalidValue { path, .. }] if path.ends_with("/height")),
            "{bad}"
        );
    }
}
