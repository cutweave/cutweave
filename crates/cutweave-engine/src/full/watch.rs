// SPDX-License-Identifier: GPL-3.0-or-later
//! [`DocWatcher`]: keeps playback in step with the document it plays (WP-1H.01 handoff).
//!
//! A thread per open document follows its event stream. After every change (coalesced: one
//! update for a burst of events, skipped when the revision did not move) it
//!
//! - gives the transport the sequence's new frame grid and extent (`set_timing`), or makes it
//!   re-render the current frame (`refresh`);
//! - re-points the mixer's clip sources at the document and replaces its segments with the
//!   whole sequence (`evaluate_audio`), so edits are heard `edit_latency` after the playhead;
//! - drops the decoder pools and cached frames of media that were removed or relinked, and
//!   their thumbnails and filmstrips (WP-1D.05 handoff).
//!
//! It never takes an engine lock; [`DocWatcher::stop`] joins it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration as StdDuration;

use cutweave_audio::MixerHandle;
use cutweave_cache::ThumbCache;
use cutweave_commands::{DocHandle, RecvError, Rev, TryRecvError};
use cutweave_model::{Project, Time};
use cutweave_playback::transport::Transport;
use cutweave_render::evaluate_audio;

use super::audio::LivePcm;
use super::frames::MediaFrames;
use super::preview::{active_sequence, timing_of};

/// How long the thread waits for an event before looking at its stop flag.
const POLL: StdDuration = StdDuration::from_millis(50);

/// What the watcher updates.
pub(crate) struct Targets {
    pub(crate) transport: Transport,
    pub(crate) mixer: Option<Arc<MixerHandle>>,
    pub(crate) pcm: Arc<LivePcm>,
    pub(crate) frames: Arc<MediaFrames>,
    pub(crate) thumbs: Option<Arc<ThumbCache>>,
}

impl Targets {
    /// Brings everything in line with `doc`.
    fn apply(&self, doc: &Project, first: bool) {
        let timing = timing_of(doc);
        if timing != self.transport.timing() {
            self.transport.set_timing(timing);
        } else if !first {
            self.transport.refresh();
        }
        self.pcm.set_doc(Some(doc));
        if let Some(mixer) = &self.mixer {
            let segments = active_sequence(doc)
                .map(|seq| {
                    let end = doc
                        .sequences
                        .get(&seq)
                        .map_or(Time::ZERO, |s| Time::from(s.duration()));
                    evaluate_audio(doc, seq, Time::ZERO, end)
                })
                .transpose();
            match segments {
                Ok(segments) => {
                    if let Err(e) = mixer.set_segments(&segments.unwrap_or_default()) {
                        tracing::warn!(error = %e, "the mixer refused the sequence's audio");
                    }
                }
                Err(e) => tracing::warn!(error = %e, "cannot evaluate the sequence's audio"),
            }
        }
        for hash in self.frames.forget_stale() {
            if let Some(thumbs) = &self.thumbs
                && let Err(e) = thumbs.disk().remove_media(&hash)
            {
                tracing::debug!(error = %e, "cannot drop the thumbnails of removed media");
            }
        }
    }
}

/// The watcher thread of one document (see the module docs).
pub(crate) struct DocWatcher {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl DocWatcher {
    /// Starts watching `doc`, applying its current state first.
    pub(crate) fn spawn(doc: DocHandle, targets: Targets) -> std::io::Result<DocWatcher> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let mut stream = doc.subscribe();
        let first = doc.snapshot();
        targets.apply(&first.doc, true);
        let mut last: Rev = first.rev;
        let thread = std::thread::Builder::new()
            .name("cw-doc-watch".into())
            .spawn(move || {
                while !flag.load(Ordering::Acquire) {
                    match stream.recv_timeout(POLL) {
                        Ok(None) => continue,
                        Ok(Some(_)) | Err(RecvError::Lagged { .. }) => {}
                        Err(RecvError::Closed) => break,
                    }
                    // Coalesce a burst.
                    loop {
                        match stream.try_recv() {
                            Ok(_) | Err(TryRecvError::Lagged { .. }) => {}
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Closed) => return,
                        }
                    }
                    let snap = doc.snapshot();
                    if snap.rev == last {
                        continue;
                    }
                    last = snap.rev;
                    targets.apply(&snap.doc, false);
                }
            })?;
        Ok(DocWatcher {
            stop,
            thread: Some(thread),
        })
    }

    /// Stops and joins the thread.
    pub(crate) fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take()
            && t.join().is_err()
        {
            tracing::error!("the document watcher panicked");
        }
    }
}

impl Drop for DocWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
