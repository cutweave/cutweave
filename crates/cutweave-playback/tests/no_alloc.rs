// SPDX-License-Identifier: GPL-3.0-or-later
//! The render loop makes no heap allocation per frame (AGENTS.md: real-time and render threads),
//! while playing into the SHM ring and while scrubbing, the sequence or a media item on its own
//! (ADR-0022). Uses the counting allocator of `cutweave-audio` (feature `testing`).

#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use cutweave_audio::ManualClock;
use cutweave_audio::rt_alloc::{CountingAlloc, RtSection};
use cutweave_model::{Duration, FrameRate, MediaId, SampleRate, Time};
use cutweave_playback::shm::{FramePublisher, PixelFormat, RingConfig};
use cutweave_playback::transport::{
    ManualAudio, PlaybackConfig, PlaybackParts, PublisherSink, RenderConfig, RenderLoop,
    SequenceTiming, SourceInfo, SyntheticSource, Tick, VirtualClock,
};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

const FRAME_30: i64 = 23_520_000;

struct Sim {
    render: RenderLoop,
    host: VirtualClock,
    manual: Arc<ManualClock>,
}

impl Sim {
    /// Runs `n` milliseconds of virtual time; returns the heap operations the render loop made.
    fn run_ms(&mut self, n: u32) -> u64 {
        let mut ops = 0;
        for _ in 0..n {
            let section = RtSection::enter();
            while self.render.tick() == Tick::Busy {}
            ops += section.exit();
            self.host.advance(1_000_000);
            if self.manual.clock().is_running() {
                self.manual.advance(Duration::from_flicks(705_600).unwrap());
            }
        }
        ops
    }
}

#[test]
fn playing_and_scrubbing_never_allocate_on_the_render_thread() {
    let host = VirtualClock::new(1_000_000_000);
    let manual = Arc::new(ManualClock::new(SampleRate::HZ_48_000));
    let publisher = FramePublisher::create(&RingConfig {
        prefault: false,
        ..RingConfig::new(160, 90, PixelFormat::Nv12)
    })
    .unwrap();
    let timing = SequenceTiming::new(FrameRate::FPS_30, Time::from_secs(60).unwrap());
    let PlaybackParts {
        transport,
        render,
        status: _status,
    } = PlaybackParts::new(
        Box::new(SyntheticSource::new(timing)),
        Box::new(ManualAudio::new(Arc::clone(&manual))),
        Box::new(PublisherSink::new(publisher)),
        PlaybackConfig {
            render: RenderConfig::default(),
            host: Arc::new(host.clone()),
            sources: None,
        },
    )
    .unwrap();
    let mut sim = Sim {
        render,
        host,
        manual,
    };

    transport.play();
    sim.run_ms(500); // warm-up
    let ops = sim.run_ms(3_000);
    assert_eq!(ops, 0, "heap operations during 3 s of playback");
    assert!(transport.stats().delivered >= 100);

    transport.pause();
    sim.run_ms(200);
    for i in 1..=50 {
        transport.seek(Time::from_flicks(i * 7 * FRAME_30 + 3).unwrap());
        let ops = sim.run_ms(150); // quarter frame, then the full refine
        assert_eq!(ops, 0, "heap operations during scrub {i}");
    }
    let stats = transport.stats();
    assert_eq!(stats.errors, 0);
    assert!(stats.delivered >= 100 + 100, "{stats:?}");

    // A media item on its own (ADR-0022): switching is a control call (it may allocate there);
    // playing and scrubbing the source allocate nothing on the render thread.
    let source = SourceInfo {
        media: MediaId::from_u128(7),
        duration: Duration::from_secs(20).unwrap(),
        fps: FrameRate::FPS_25,
    };
    transport.set_source(Some(source));
    transport.play();
    sim.run_ms(500); // warm-up
    let before = transport.stats().delivered;
    let ops = sim.run_ms(3_000);
    assert_eq!(ops, 0, "heap operations during 3 s of source playback");
    assert!(transport.stats().delivered >= before + 70);
    transport.pause();
    sim.run_ms(200);
    for i in 1..=20 {
        transport.seek(Time::from_flicks(i * 9 * 28_224_000 + 5).unwrap());
        let ops = sim.run_ms(150);
        assert_eq!(ops, 0, "heap operations during source scrub {i}");
    }
    // And back to the sequence, on its own playhead.
    transport.set_source(None);
    let ops = sim.run_ms(150);
    assert_eq!(ops, 0, "heap operations showing the sequence again");
    assert_eq!(transport.stats().errors, 0);
}
