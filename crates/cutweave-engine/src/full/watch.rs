// SPDX-License-Identifier: GPL-3.0-or-later
//! [`DocWatcher`]: keeps playback in step with the document it plays (WP-1H.01 handoff).
//!
//! A thread per open document follows its event stream. After every change (coalesced: one
//! update for a burst of events, skipped when the revision did not move) it
//!
//! - gives the transport the sequence's new frame grid and extent, or makes it re-render the
//!   current frame (`Transport::sequence_edited`); while a source plays (ADR-0022) the sequence
//!   waits and the source is not interrupted;
//! - ends a source whose asset was removed, and re-times or refreshes one whose asset changed
//!   (`Transport::source_changed`);
//! - re-points the mixer's clip sources at the document and feeds the mixer the sound of what
//!   plays ([`AudioFeed`]: the whole sequence, or the source), so edits are heard
//!   `edit_latency` after the playhead;
//! - drops the decoder pools and cached frames of media that were removed or relinked, and
//!   their thumbnails and filmstrips (WP-1D.05 handoff);
//! - asks for the proxies of long-GOP media that a clip now plays backwards, and warns while
//!   they build (WP-2D.01b, [`FullProxies::reversed`]).
//!
//! It never takes an engine lock; [`DocWatcher::stop`] joins it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration as StdDuration;

use cutweave_cache::ThumbCache;
use cutweave_commands::{DocHandle, RecvError, TryRecvError};
use cutweave_model::Project;
use cutweave_playback::transport::{PlaybackTarget, Transport};

use super::audio::{AudioFeed, LivePcm};
use super::frames::MediaFrames;
use super::preview::{source_info, timing_of};
use super::proxy::FullProxies;

/// How long the thread waits for an event before looking at its stop flag.
const POLL: StdDuration = StdDuration::from_millis(50);

/// What the watcher updates.
pub(crate) struct Targets {
    pub(crate) transport: Transport,
    pub(crate) feed: Arc<AudioFeed>,
    pub(crate) pcm: Arc<LivePcm>,
    pub(crate) frames: Arc<MediaFrames>,
    pub(crate) thumbs: Option<Arc<ThumbCache>>,
    /// Proxies to ask for when a clip plays long-GOP media backwards.
    pub(crate) proxies: Option<Arc<FullProxies>>,
}

impl Targets {
    /// Brings everything in line with `doc`, which follows `previous` (`None`: the document
    /// playback attached to).
    fn apply(&self, doc: &Project, previous: Option<&Project>) {
        let timing = timing_of(doc);
        match previous {
            None => {
                if timing != self.transport.sequence_timing() {
                    self.transport.set_timing(timing);
                }
            }
            Some(previous) => {
                self.transport.sequence_edited(timing);
                if let PlaybackTarget::Media(media) = self.transport.target()
                    && previous.media.get(&media) != doc.media.get(&media)
                {
                    // Removed (the source ends), re-probed or relinked.
                    self.transport
                        .source_changed(media, source_info(doc, media).ok());
                }
            }
        }
        self.pcm.set_doc(Some(doc));
        self.feed.document(doc);
        for hash in self.frames.forget_stale() {
            if let Some(thumbs) = &self.thumbs
                && let Err(e) = thumbs.disk().remove_media(&hash)
            {
                tracing::debug!(error = %e, "cannot drop the thumbnails of removed media");
            }
        }
        // Not for the document playback attached to: `apply(_, None)` runs under the session
        // lock, and attaching reconciles the document's proxies (reversed clips too) on its
        // own thread.
        if previous.is_some()
            && let Some(proxies) = &self.proxies
        {
            proxies.reversed(doc);
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
        targets.apply(&first.doc, None);
        let mut last = first;
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
                    if snap.rev == last.rev {
                        continue;
                    }
                    targets.apply(&snap.doc, Some(&last.doc));
                    last = snap;
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
