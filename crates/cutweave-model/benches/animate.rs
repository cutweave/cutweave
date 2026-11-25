// SPDX-License-Identifier: GPL-3.0-or-later
//! Keyframe evaluation budget (WP-2C.02): 10 animated properties × 3 layers at one time
//! (< 20 µs), and the key lookup in a 50,000-key property (binary search, < 1 µs).
//!
//! `cargo bench -p cutweave-model --bench animate`

#![allow(missing_docs, clippy::expect_used)]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use cutweave_model::{
    AnimValue, Animatable, Easing, FLICKS_PER_SECOND, Interp, Keyframe, Keyframes, Time, Vec2,
};

const S: i64 = FLICKS_PER_SECOND;

fn time(flicks: i64) -> Time {
    Time::from_flicks(flicks).expect("a safe time")
}

/// A 10-key animation over 10 s cycling through linear, a bezier preset and a spring.
fn keys<T: AnimValue>(value: impl Fn(i64) -> T) -> Keyframes<T> {
    let interps = [
        Interp::Linear,
        Easing::EaseInOut.interp(),
        Easing::SpringWobbly.interp(),
    ];
    let list = (0..10)
        .map(|i| {
            Keyframe::new(
                time(i * S),
                value(i),
                interps[usize::try_from(i).unwrap_or(0) % 3].clone(),
            )
        })
        .collect();
    Keyframes::new(list).expect("valid keys")
}

/// One layer's animated properties: position, scale, rotation, anchor, opacity, four crop
/// edges and gain (10).
struct Layer {
    vec2: [Animatable<Vec2>; 3],
    scalar: [Animatable<f64>; 7],
}

impl Layer {
    fn new(seed: i64) -> Layer {
        #[allow(clippy::cast_precision_loss)]
        let f = |i: i64| ((i * 7 + seed) % 11) as f64 / 10.0;
        Layer {
            vec2: std::array::from_fn(|_| {
                Animatable::Keyframes(keys(|i| Vec2::new(f(i), f(i + 3))))
            }),
            scalar: std::array::from_fn(|_| Animatable::Keyframes(keys(f))),
        }
    }

    fn eval(&self, t: Time) -> f64 {
        let mut acc = 0.0;
        for p in &self.vec2 {
            let v = p.eval(t);
            acc += v.x + v.y;
        }
        for p in &self.scalar {
            acc += p.eval(t);
        }
        acc
    }
}

fn bench(c: &mut Criterion) {
    let layers = [Layer::new(1), Layer::new(2), Layer::new(3)];
    // Inside a spring segment, the most expensive kind.
    let t = time(5 * S + S / 3);
    c.bench_function("eval 10 props x 3 layers", |b| {
        b.iter(|| layers.iter().map(|l| l.eval(black_box(t))).sum::<f64>());
    });

    let dense = Animatable::Keyframes(
        Keyframes::new(
            (0..50_000_i64)
                .map(|i| Keyframe::new(time(i * (S / 30)), 0.5, Interp::Linear))
                .collect(),
        )
        .expect("valid keys"),
    );
    let Some(keys) = dense.keyframes() else {
        return;
    };
    let mut probe = 0_i64;
    c.bench_function("lookup in 50k keys", |b| {
        b.iter(|| {
            // A different place each time, so the search is not served from one cache line.
            probe = (probe + 7_919 * (S / 30) + 12_345) % (50_000 * (S / 30));
            keys.key_at_or_before(black_box(time(probe)))
        });
    });
    c.bench_function("eval in 50k keys", |b| {
        b.iter(|| {
            probe = (probe + 7_919 * (S / 30) + 12_345) % (50_000 * (S / 30));
            dense.eval(black_box(time(probe)))
        });
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
