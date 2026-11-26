// SPDX-License-Identifier: GPL-3.0-or-later
//! Snapping tests (WP-2A.07): the spec's `snap` fixtures, hand-made cases for ties, disabled
//! kinds and zoom, and a brute-force reference over random indexes.

#![allow(
    clippy::cast_precision_loss,
    clippy::unnecessary_wraps,
    clippy::type_complexity
)]

use std::{fs, path::PathBuf};

use serde_json::Value;

use super::{SnapError, SnapIndex, SnapKind, SnapKinds, SnapOptions, SnapTarget, Snapped, snap};
use crate::{FLICKS_PER_SECOND, FrameRate, MAX_SAFE_FLICKS};

const S: i64 = FLICKS_PER_SECOND;
/// One frame at 25 fps.
const F25: i64 = S / 25;

fn rate25() -> FrameRate {
    FrameRate::new(25, 1).unwrap()
}

fn index(cands: &[(SnapKind, i64)]) -> SnapIndex {
    SnapIndex::new(rate25(), cands.iter().copied()).unwrap()
}

fn opts(pps: f64) -> SnapOptions {
    SnapOptions::new(pps)
}

fn at(cands: &[(SnapKind, i64)], r: i64, offsets: &[i64], o: &SnapOptions) -> Snapped {
    snap(&index(cands), r, offsets, o).unwrap()
}

fn target(kind: SnapKind, t: i64) -> Option<SnapTarget> {
    Some(SnapTarget { kind, t })
}

// ----- the spec fixtures -----------------------------------------------------------------------

fn fixtures() -> Vec<Value> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/spec/fixtures");
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.starts_with("snap-") && path.extension().is_some_and(|e| e == "json") {
            out.push(serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap());
        }
    }
    out
}

/// Runs one `snap` fixture through [`snap`].
fn run_fixture(fx: &Value) -> Snapped {
    let rate = FrameRate::new(u32::try_from(fx["fps"].as_i64().unwrap()).unwrap(), 1).unwrap();
    let cands = fx["candidates"].as_array().unwrap().iter().map(|c| {
        (
            SnapKind::from_name(c["kind"].as_str().unwrap()).unwrap(),
            c["t"].as_i64().unwrap(),
        )
    });
    let enabled = match fx.get("enabled") {
        Some(list) => list
            .as_array()
            .unwrap()
            .iter()
            .map(|k| SnapKind::from_name(k.as_str().unwrap()).unwrap())
            .collect(),
        None => SnapKinds::ALL,
    };
    let offsets: Vec<i64> = fx["offsets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o.as_i64().unwrap())
        .collect();
    let options = SnapOptions {
        px_per_second: fx["px_per_second"].as_f64().unwrap(),
        threshold_px: fx["threshold_px"].as_f64().unwrap(),
        enabled,
    };
    snap(
        &SnapIndex::new(rate, cands).unwrap(),
        fx["r"].as_i64().unwrap(),
        &offsets,
        &options,
    )
    .unwrap()
}

#[test]
fn every_spec_fixture_gives_the_expected_result() {
    let all = fixtures();
    assert!(all.len() >= 10, "found {} snap fixtures", all.len());
    for fx in &all {
        let got = run_fixture(fx);
        let want = &fx["expected"];
        assert_eq!(got.t, want["t"].as_i64().unwrap(), "{}", fx["id"]);
        let want_target = want["target"].as_object().map(|o| SnapTarget {
            kind: SnapKind::from_name(o["kind"].as_str().unwrap()).unwrap(),
            t: o["t"].as_i64().unwrap(),
        });
        assert_eq!(got.target, want_target, "{}", fx["id"]);
    }
}

// ----- hand-made cases -------------------------------------------------------------------------

#[test]
fn an_empty_index_gives_the_frame_snap() {
    let got = at(&[], F25 * 3 + 100, &[0], &opts(100.0));
    assert_eq!((got.t, got.target), (F25 * 3, None));
}

#[test]
fn the_frame_snap_rounds_to_nearest_and_ties_go_later() {
    let none = opts(100.0);
    assert_eq!(at(&[], F25 / 2, &[0], &none).t, F25);
    assert_eq!(at(&[], F25 / 2 - 1, &[0], &none).t, 0);
}

#[test]
fn a_candidate_snaps_the_probe_exactly() {
    let c = [(SnapKind::Playhead, 10 * F25)];
    let got = at(&c, 10 * F25 + 1_000_000, &[0], &opts(100.0));
    assert_eq!(
        (got.t, got.target),
        (10 * F25, target(SnapKind::Playhead, 10 * F25))
    );
}

#[test]
fn the_tolerance_is_inclusive() {
    // 8 px at 100 px/s is 56 448 000 flicks, an exact integer.
    let tol = 8 * S / 100;
    let c = [(SnapKind::Edge, 20 * F25)];
    assert!(at(&c, 20 * F25 + tol, &[0], &opts(100.0)).target.is_some());
    assert!(
        at(&c, 20 * F25 + tol + 1, &[0], &opts(100.0))
            .target
            .is_none()
    );
    assert!(at(&c, 20 * F25 - tol, &[0], &opts(100.0)).target.is_some());
}

#[test]
fn doubling_the_zoom_halves_the_tolerance() {
    let c = [(SnapKind::Edge, 20 * F25)];
    let r = 20 * F25 + 5 * S / 100; // 5 px at 100 px/s
    assert!(at(&c, r, &[0], &opts(100.0)).target.is_some());
    assert!(at(&c, r, &[0], &opts(200.0)).target.is_none());
    assert!(
        (opts(100.0).tolerance().unwrap() - 2.0 * opts(200.0).tolerance().unwrap()).abs() < 1e-6
    );
}

#[test]
fn a_larger_threshold_reaches_further() {
    let c = [(SnapKind::Edge, 20 * F25)];
    let r = 20 * F25 + 12 * S / 100;
    assert!(at(&c, r, &[0], &opts(100.0)).target.is_none());
    let wide = SnapOptions {
        threshold_px: 16.0,
        ..opts(100.0)
    };
    assert!(at(&c, r, &[0], &wide).target.is_some());
}

#[test]
fn a_zero_threshold_snaps_only_to_the_exact_time() {
    let zero = SnapOptions {
        threshold_px: 0.0,
        ..opts(100.0)
    };
    let c = [(SnapKind::Edge, 20 * F25)];
    assert!(at(&c, 20 * F25, &[0], &zero).target.is_some());
    assert!(at(&c, 20 * F25 + 1, &[0], &zero).target.is_none());
}

#[test]
fn a_disabled_kind_never_attracts() {
    let c = [(SnapKind::Marker, 10 * F25)];
    let off = SnapOptions {
        enabled: SnapKinds::ALL.without(SnapKind::Marker),
        ..opts(100.0)
    };
    let got = at(&c, 10 * F25 + 3, &[0], &off);
    assert_eq!((got.t, got.target), (10 * F25, None));
    // Without the marker, a time past the frame still rounds to the nearest frame.
    assert_eq!(at(&c, 10 * F25 + F25 / 2, &[0], &off).t, 11 * F25);
}

#[test]
fn with_every_kind_disabled_the_result_is_the_frame_snap() {
    let c = [(SnapKind::Playhead, 10 * F25)];
    let off = SnapOptions {
        enabled: SnapKinds::NONE,
        ..opts(100.0)
    };
    let got = at(&c, 10 * F25 + 1, &[0], &off);
    assert_eq!((got.t, got.target), (10 * F25, None));
}

#[test]
fn ties_go_to_the_higher_ranked_kind() {
    let r = 10 * F25 + F25 / 2; // midway between the two candidates
    let (lo, hi) = (10 * F25, 11 * F25);
    for (a, b, winner) in [
        (SnapKind::Edge, SnapKind::Playhead, SnapKind::Playhead),
        (SnapKind::Marker, SnapKind::Edge, SnapKind::Edge),
        (SnapKind::Beat, SnapKind::Marker, SnapKind::Marker),
        (SnapKind::InOut, SnapKind::Beat, SnapKind::Beat),
    ] {
        let wide = SnapOptions {
            threshold_px: 40.0,
            ..opts(100.0)
        };
        let got = at(&[(a, lo), (b, hi)], r, &[0], &wide);
        assert_eq!(got.target.map(|t| t.kind), Some(winner), "{a:?} vs {b:?}");
        // The kinds swap sides: the rank still decides.
        let got = at(&[(b, lo), (a, hi)], r, &[0], &wide);
        assert_eq!(got.target.map(|t| t.kind), Some(winner), "{b:?} vs {a:?}");
    }
}

#[test]
fn a_nearer_lower_ranked_candidate_beats_a_farther_playhead() {
    let c = [(SnapKind::Playhead, 12 * F25), (SnapKind::InOut, 10 * F25)];
    let got = at(&c, 10 * F25 + 1_000, &[0], &opts(100.0));
    assert_eq!(got.target.map(|t| t.kind), Some(SnapKind::InOut));
}

#[test]
fn at_equal_rank_and_distance_the_earlier_time_wins() {
    let r = 10 * F25 + F25 / 2;
    let wide = SnapOptions {
        threshold_px: 40.0,
        ..opts(100.0)
    };
    let c = [(SnapKind::Edge, 11 * F25), (SnapKind::Edge, 10 * F25)];
    // r is the midpoint only when F25 is even; 28 224 000 is.
    let got = at(&c, r, &[0], &wide);
    assert_eq!(got.target.map(|t| t.t), Some(10 * F25));
}

#[test]
fn a_clip_snaps_by_whichever_edge_is_nearer() {
    let dur = 4 * F25;
    let c = [(SnapKind::Edge, 30 * F25)];
    // The end (probe 1) is 1 ms from the edge at 30 frames; the start is far.
    let got = at(&c, 26 * F25 + 1_000_000, &[0, dur], &opts(100.0));
    assert_eq!((got.t, got.probe), (26 * F25, 1));
    assert_eq!(got.target, target(SnapKind::Edge, 30 * F25));
}

#[test]
fn equal_distance_prefers_the_earlier_probe() {
    let dur = 4 * F25;
    let c = [(SnapKind::Edge, 10 * F25), (SnapKind::Edge, 14 * F25)];
    // A clip exactly between: start 1 ms before the first edge, end 1 ms before the second.
    let got = at(&c, 10 * F25 - 1_000, &[0, dur], &opts(100.0));
    assert_eq!(got.probe, 0);
    assert_eq!(got.t, 10 * F25);
}

#[test]
fn a_result_before_the_timeline_start_is_not_eligible() {
    // The clip's end would land on the edge at 2 frames, putting its start before zero.
    let dur = 4 * F25;
    let c = [(SnapKind::Edge, 2 * F25)];
    let got = at(&c, 1_000, &[0, dur], &opts(100.0));
    assert_eq!(got.probe, 0);
    let c = [(SnapKind::Edge, 3 * F25)];
    let got = at(&c, 0, &[dur], &opts(100_000.0));
    assert_eq!((got.t, got.target), (0, None));
}

#[test]
fn the_timeline_start_is_an_edge_for_a_clip() {
    let c = [(SnapKind::Edge, 0)];
    let got = at(&c, 2_000_000, &[0], &opts(100.0));
    assert_eq!((got.t, got.target), (0, target(SnapKind::Edge, 0)));
}

#[test]
fn a_candidate_between_frames_is_moved_to_a_frame_first() {
    let c = [(SnapKind::Beat, 10 * F25 + F25 / 2 + 1)]; // rounds to the later frame
    let got = at(&c, 11 * F25 - 5_000, &[0], &opts(100.0));
    assert_eq!(got.target, target(SnapKind::Beat, 11 * F25));
    assert_eq!(got.t, 11 * F25);
}

#[test]
fn results_are_on_the_grid_at_an_inexact_rate() {
    let rate = FrameRate::new(30_000, 1001).unwrap();
    let frame = |n: i64| rate.frame_to_time(n).unwrap().flicks();
    let idx = SnapIndex::new(
        rate,
        [
            (SnapKind::Edge, frame(100) + 7),
            (SnapKind::Marker, frame(50)),
        ],
    )
    .unwrap();
    let dur = frame(7) - frame(0);
    for r in [frame(100) - 90_000, frame(40) + 3, frame(49) + 11_000_000] {
        let got = idx.snap(r, &[0, dur], &opts(500.0)).unwrap();
        let back = rate.snap_to_frame(
            crate::Time::from_flicks(got.t).unwrap(),
            crate::Rounding::Nearest,
        );
        assert_eq!(back.unwrap().flicks(), got.t);
    }
    // A target is moved to its frame.
    assert_eq!(
        idx.targets()
            .iter()
            .find(|c| c.kind == SnapKind::Edge)
            .unwrap()
            .t,
        frame(100)
    );
}

#[test]
fn bad_zoom_threshold_and_times_are_errors() {
    let idx = index(&[]);
    for pps in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(matches!(
            snap(&idx, 0, &[0], &opts(pps)),
            Err(SnapError::BadZoom(_))
        ));
    }
    for px in [-1.0, f64::NAN, f64::INFINITY] {
        let o = SnapOptions {
            threshold_px: px,
            ..opts(100.0)
        };
        assert!(matches!(
            snap(&idx, 0, &[0], &o),
            Err(SnapError::BadThreshold(_))
        ));
    }
    assert!(matches!(
        snap(&idx, MAX_SAFE_FLICKS + 1, &[0], &opts(100.0)),
        Err(SnapError::Time(_))
    ));
    assert!(SnapIndex::new(rate25(), [(SnapKind::Edge, i64::MIN)]).is_err());
}

#[test]
fn a_tiny_zoom_makes_a_huge_tolerance_without_overflow() {
    let c = [(SnapKind::Edge, 5 * F25)];
    let got = at(&c, MAX_SAFE_FLICKS / 2, &[0], &opts(1e-12));
    assert_eq!(got.target, target(SnapKind::Edge, 5 * F25));
}

#[test]
fn set_kind_replaces_one_kind_and_keeps_the_rest() {
    let mut idx = index(&[
        (SnapKind::Edge, 3 * F25),
        (SnapKind::Playhead, 5 * F25),
        (SnapKind::Edge, 9 * F25),
    ]);
    idx.set_kind(SnapKind::Edge, [7 * F25, F25]).unwrap();
    let list: Vec<_> = idx.targets().iter().map(|c| (c.kind, c.t)).collect();
    assert_eq!(
        list,
        [
            (SnapKind::Edge, F25),
            (SnapKind::Playhead, 5 * F25),
            (SnapKind::Edge, 7 * F25)
        ]
    );
    idx.set_kind(SnapKind::Edge, []).unwrap();
    assert_eq!(idx.len(), 1);
}

#[test]
fn a_failed_set_kind_leaves_the_index_unchanged() {
    let mut idx = index(&[(SnapKind::Edge, F25)]);
    assert!(idx.set_kind(SnapKind::Edge, [0, i64::MAX]).is_err());
    assert_eq!(
        idx.targets(),
        [SnapTarget {
            kind: SnapKind::Edge,
            t: F25
        }]
    );
}

#[test]
fn kinds_and_sets_round_trip() {
    for (i, k) in SnapKind::ALL.into_iter().enumerate() {
        assert_eq!(usize::from(k.code()), i);
        assert_eq!(SnapKind::from_code(u32::from(k.code())), Some(k));
        assert_eq!(SnapKind::from_name(k.name()), Some(k));
        assert!(SnapKinds::ALL.contains(k));
        assert!(!SnapKinds::NONE.contains(k));
        assert_eq!(
            SnapKinds::from_bits(SnapKinds::only(k).bits()),
            SnapKinds::only(k)
        );
    }
    assert_eq!(SnapKind::from_code(5), None);
    assert_eq!(SnapKind::from_name("nope"), None);
    assert_eq!(SnapKinds::from_bits(0xFF), SnapKinds::ALL);
    assert_eq!(SnapKinds::ALL.without(SnapKind::Beat).bits(), 0b1_0111);
}

// ----- brute force over random indexes ---------------------------------------------------------

fn lcg(seed: &mut u64) -> i64 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    i64::try_from(*seed >> 33).unwrap()
}

/// The spec's algorithm written the slow, obvious way.
fn reference(
    cands: &[SnapTarget],
    r: i64,
    offsets: &[i64],
    o: &SnapOptions,
) -> (i64, Option<SnapTarget>) {
    let tol = o.tolerance().unwrap();
    let mut best: Option<((i64, u8, usize, i64), SnapTarget, i64)> = None;
    for (i, &off) in offsets.iter().enumerate() {
        for c in cands {
            let d = (c.t - (r + off)).abs();
            if !o.enabled.contains(c.kind) || d as f64 > tol || c.t - off < 0 {
                continue;
            }
            let key = (d, c.kind.code(), i, c.t);
            if best.as_ref().is_none_or(|(k, ..)| key < *k) {
                best = Some((key, *c, off));
            }
        }
    }
    match best {
        Some((_, c, off)) => (c.t - off, Some(c)),
        None => ((2 * r + F25).div_euclid(2 * F25) * F25, None),
    }
}

#[test]
fn it_matches_a_brute_force_reference_and_stays_on_the_grid() {
    let mut seed = 0x2A07;
    for round in 0..300 {
        let n = usize::try_from(lcg(&mut seed) % 60).unwrap();
        let raw: Vec<(SnapKind, i64)> = (0..n)
            .map(|_| {
                let kind = SnapKind::from_code(u32::try_from(lcg(&mut seed) % 5).unwrap()).unwrap();
                (kind, lcg(&mut seed) % (400 * F25))
            })
            .collect();
        let idx = SnapIndex::new(rate25(), raw).unwrap();
        let offsets: Vec<i64> = (0..=lcg(&mut seed) % 4)
            .map(|_| (lcg(&mut seed) % 60) * F25)
            .collect();
        let enabled = SnapKinds::from_bits(u32::try_from(lcg(&mut seed) % 32).unwrap());
        let options = SnapOptions {
            px_per_second: [10.0, 100.0, 1000.0, 20_000.0][usize::try_from(round % 4).unwrap()],
            threshold_px: 8.0,
            enabled,
        };
        for _ in 0..20 {
            let r = lcg(&mut seed) % (420 * F25);
            let got = idx.snap(r, &offsets, &options).unwrap();
            let (t, target) = reference(idx.targets(), r, &offsets, &options);
            assert_eq!((got.t, got.target), (t, target), "round {round}, r {r}");
            assert_eq!(got.t % F25, 0, "off the frame grid");
            if let Some(c) = got.target {
                assert_eq!(got.t + offsets[got.probe], c.t, "INV-322");
                assert!(((got.t - r).abs() as f64) <= options.tolerance().unwrap() + F25 as f64);
            }
        }
    }
}

#[test]
fn set_kind_gives_the_same_index_as_a_fresh_build() {
    let mut seed = 0x51;
    for _ in 0..50 {
        let mut all: Vec<(SnapKind, i64)> = (0..40)
            .map(|_| {
                let k = SnapKind::from_code(u32::try_from(lcg(&mut seed) % 5).unwrap()).unwrap();
                (k, lcg(&mut seed) % (100 * F25))
            })
            .collect();
        let mut idx = SnapIndex::new(rate25(), all.iter().copied()).unwrap();
        let fresh: Vec<i64> = (0..15).map(|_| lcg(&mut seed) % (100 * F25)).collect();
        idx.set_kind(SnapKind::Marker, fresh.iter().copied())
            .unwrap();
        all.retain(|(k, _)| *k != SnapKind::Marker);
        all.extend(fresh.iter().map(|t| (SnapKind::Marker, *t)));
        let rebuilt = SnapIndex::new(rate25(), all).unwrap();
        assert_eq!(idx.targets(), rebuilt.targets());
    }
}

#[test]
fn the_snap_method_is_the_function() {
    let idx = index(&[(SnapKind::Edge, 4 * F25)]);
    let o = opts(100.0);
    assert_eq!(
        idx.snap(4 * F25 + 9, &[0], &o).unwrap(),
        snap(&idx, 4 * F25 + 9, &[0], &o).unwrap()
    );
}

#[test]
fn many_candidates_at_one_time_pick_the_highest_rank() {
    let c: Vec<_> = SnapKind::ALL
        .into_iter()
        .rev()
        .map(|k| (k, 6 * F25))
        .collect();
    let got = at(&c, 6 * F25 + 5, &[0], &opts(100.0));
    assert_eq!(got.target.map(|t| t.kind), Some(SnapKind::Playhead));
}

#[test]
fn a_multi_item_move_snaps_by_any_moving_edge() {
    // Edges of the moving items at 0, 2, 5 and 9 frames from the primary's start.
    let offsets = [0, 2 * F25, 5 * F25, 9 * F25];
    let c = [(SnapKind::Marker, 30 * F25), (SnapKind::Edge, 100 * F25)];
    let got = at(&c, 25 * F25 + 2_000, &offsets, &opts(100.0));
    assert_eq!((got.t, got.probe), (25 * F25, 2));
}
