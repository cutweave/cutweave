// SPDX-License-Identifier: GPL-3.0-or-later
//! [`SpyBackend`]: a [`MediaBackend`] that passes everything to another one and notes every
//! file it is asked to read (WP-2F.02: "an export opens no proxy file").

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use cutweave_model::ProbeInfo;

use crate::{
    AudioDecoder, Capabilities, ConcatSpec, DecodeOpts, EncodeSpec, EncodeSummary, Encoder,
    FrameIndex, MediaBackend, MediaError, VideoDecoder,
};

/// How a file was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OpenKind {
    /// [`MediaBackend::probe`].
    Probe,
    /// [`MediaBackend::index_media`].
    Index,
    /// [`MediaBackend::open_decoder`].
    Video,
    /// [`MediaBackend::open_audio_decoder`].
    Audio,
}

/// One read of a file through a [`SpyBackend`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileOpen {
    /// How it was read.
    pub kind: OpenKind,
    /// The file.
    pub path: PathBuf,
}

/// The reads a [`SpyBackend`] saw, shared with the test that made it.
#[derive(Clone, Debug, Default)]
pub struct OpenLog(Arc<Mutex<Vec<FileOpen>>>);

impl OpenLog {
    fn note(&self, kind: OpenKind, path: &Path) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(FileOpen {
                kind,
                path: path.to_path_buf(),
            });
    }

    /// Every read so far, oldest first.
    #[must_use]
    pub fn opens(&self) -> Vec<FileOpen> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The decoder opens (pictures and sound) so far, oldest first.
    #[must_use]
    pub fn decoder_opens(&self) -> Vec<FileOpen> {
        let mut opens = self.opens();
        opens.retain(|o| matches!(o.kind, OpenKind::Video | OpenKind::Audio));
        opens
    }

    /// The reads of files under `dir` so far.
    #[must_use]
    pub fn under(&self, dir: &Path) -> Vec<FileOpen> {
        let mut opens = self.opens();
        opens.retain(|o| o.path.starts_with(dir));
        opens
    }

    /// Forgets the reads so far.
    pub fn clear(&self) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

/// A backend that notes every file `inner` is asked to read (see [`OpenLog`]).
pub struct SpyBackend {
    inner: Arc<dyn MediaBackend>,
    log: OpenLog,
}

impl std::fmt::Debug for SpyBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpyBackend")
            .field("opens", &self.log.opens().len())
            .finish_non_exhaustive()
    }
}

impl SpyBackend {
    /// A spy on `inner`.
    #[must_use]
    pub fn new(inner: Arc<dyn MediaBackend>) -> SpyBackend {
        SpyBackend {
            inner,
            log: OpenLog::default(),
        }
    }

    /// The reads it saw (shared: keep it after the backend is handed to an engine).
    #[must_use]
    pub fn log(&self) -> OpenLog {
        self.log.clone()
    }
}

impl MediaBackend for SpyBackend {
    fn probe(&self, path: &Path) -> Result<ProbeInfo, MediaError> {
        self.log.note(OpenKind::Probe, path);
        self.inner.probe(path)
    }

    fn index_media(&self, path: &Path) -> Result<FrameIndex, MediaError> {
        self.log.note(OpenKind::Index, path);
        self.inner.index_media(path)
    }

    fn open_decoder(
        &self,
        path: &Path,
        opts: &DecodeOpts,
    ) -> Result<Box<dyn VideoDecoder>, MediaError> {
        self.log.note(OpenKind::Video, path);
        self.inner.open_decoder(path, opts)
    }

    fn open_audio_decoder(
        &self,
        path: &Path,
        stream: Option<u32>,
    ) -> Result<Box<dyn AudioDecoder>, MediaError> {
        self.log.note(OpenKind::Audio, path);
        self.inner.open_audio_decoder(path, stream)
    }

    fn open_encoder(&self, spec: &EncodeSpec) -> Result<Box<dyn Encoder>, MediaError> {
        self.inner.open_encoder(spec)
    }

    fn concat(&self, spec: &ConcatSpec) -> Result<EncodeSummary, MediaError> {
        self.inner.concat(spec)
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use cutweave_model::{Duration, FrameRate};

    use super::*;
    use crate::testing::{FakeBackend, FakeMedia};

    #[test]
    fn every_read_is_noted_with_its_file() {
        let clip = Path::new("/media/a.mp4");
        let d = Duration::from_secs(1).unwrap();
        let fake = FakeBackend::new().with_media(clip, FakeMedia::clip(d, FrameRate::FPS_30));
        let spy = SpyBackend::new(Arc::new(fake.clone()));
        let log = spy.log();
        spy.probe(clip).unwrap();
        spy.index_media(clip).unwrap();
        spy.open_decoder(clip, &DecodeOpts::default()).unwrap();
        spy.open_audio_decoder(clip, None).unwrap();
        assert!(spy.probe(Path::new("/cache/proxies/x.mov")).is_err());
        let kinds: Vec<_> = log.opens().iter().map(|o| o.kind).collect();
        assert_eq!(
            kinds,
            [
                OpenKind::Probe,
                OpenKind::Index,
                OpenKind::Video,
                OpenKind::Audio,
                OpenKind::Probe
            ]
        );
        assert_eq!(log.decoder_opens().len(), 2);
        assert_eq!(log.under(Path::new("/cache/proxies")).len(), 1);
        assert_eq!(log.under(Path::new("/media")).len(), 4);
        assert_eq!(fake.decoders_opened(), 1);
        log.clear();
        assert_eq!(log.opens(), Vec::new());
    }
}
