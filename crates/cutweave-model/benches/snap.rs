// SPDX-License-Identifier: GPL-3.0-or-later
//! Snap lookup budget (WP-2A.07): one snap against 50,000 candidates (a clip probe pair, all
//! kinds) under 0.1 ms, and an incremental `set_kind` of 5,000 edges.
//!
//! `cargo bench -p cutweave-model --bench snap`

#![allow(missing_docs, clippy::expect_used)]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use cutweave_model::{FLICKS_PER_SECOND, FrameRate, SnapIndex, SnapKind, SnapOptions};

const S: i64 = FLICKS_PER_SECOND;

fn index() -> SnapIndex {
    let rate = FrameRate::new(25, 1).expect("25 fps");
    let kinds = [SnapKind::Edge, SnapKind::Marker, SnapKind::Beat];
    let cands = (0..50_000_i64).map(|i| (kinds[usize::try_from(i).unwrap_or(0) % 3], i * S / 5));
    SnapIndex::new(rate, cands).expect("an index")
}

fn snap(c: &mut Criterion) {
    let idx = index();
    let options = SnapOptions::new(100.0);
    let offsets = [0, 4 * S];
    c.bench_function("snap_50k_candidates", |b| {
        let mut r = 0_i64;
        b.iter(|| {
            r = (r + 7_919_001) % (9_000 * S);
            black_box(idx.snap(black_box(r), &offsets, &options).expect("snap"))
        });
    });
    c.bench_function("set_kind_5k_edges_into_50k", |b| {
        let mut idx = index();
        b.iter(|| {
            idx.set_kind(SnapKind::Edge, (0..5_000_i64).map(|i| i * S / 3))
                .expect("set_kind");
        });
    });
}

criterion_group!(benches, snap);
criterion_main!(benches);
