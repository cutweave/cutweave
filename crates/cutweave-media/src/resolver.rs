// SPDX-License-Identifier: GPL-3.0-or-later
//! [`MediaResolver`]: the one place that decides which file a reader opens for a media item,
//! the original or its proxy (arch §7 "Proxies", §8 quality ladder; WP-2F.02).
//!
//! A reader says why it reads ([`MediaPurpose`]) and gets a [`MediaSource`]:
//!
//! - **Export, thumbnails and analysis always read the original.** The resolver does not even
//!   look a proxy up for them, so an export of a project with proxies is the export without.
//! - **The preview reads the proxy** when the preview may use proxies
//!   ([`PreviewQuality::use_proxies`]: the transport's toggle), the item has a ready one (the
//!   [`ProxyLookup`]: it also says no for an item whose proxies are off and for one whose
//!   original is not on this computer, so offline media shows its slate, never a proxy
//!   picture), and the proxy holds the pixels the picture is drawn at
//!   ([`PreviewQuality::long_edge`]): **the resolution rule**. A picture drawn larger than the
//!   proxy (a full-resolution preview on a large canvas, a zoomed-in clip) reads the original.
//! - **A proxy that is gone** (evicted, deleted, its file missing) is simply not found by the
//!   lookup, and a proxy that cannot be decoded makes the reader fall back: the original is
//!   always a valid answer.
//!
//! The resolver is pure and allocates nothing by itself. A reader on a real-time thread looks
//! the proxy up once ([`MediaResolver::candidate`], when the store's generation moved) and
//! asks [`MediaResolver::serves`] per frame, which is a comparison.

/// Why a media file is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaPurpose {
    /// A picture for the preview, at this quality.
    Preview(PreviewQuality),
    /// An export, or an agent frame (`render.frame`, `/media/frame`), which shows what an
    /// export would: always the original.
    Export,
    /// A thumbnail or a filmstrip tile: always the original.
    Thumbnail,
    /// Analysis (frame index, waveform, content hash, making the proxy itself): always the
    /// original.
    Analysis,
}

impl MediaPurpose {
    /// Whether a reader with this purpose may be given a proxy at all.
    #[must_use]
    pub const fn may_use_proxy(self) -> bool {
        matches!(self, MediaPurpose::Preview(q) if q.use_proxies)
    }
}

/// What a preview picture needs from the file it is decoded from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PreviewQuality {
    /// Whether the preview uses proxies (`view.set_preview_quality{use_proxies}`).
    pub use_proxies: bool,
    /// The longest side, in pixels, the whole picture is drawn at on the preview's output
    /// (never more than the original has). `None`: any size will do (reverse playback, where
    /// decoding speed matters more).
    pub long_edge: Option<u32>,
}

impl PreviewQuality {
    /// A preview that reads originals only.
    pub const ORIGINALS: PreviewQuality = PreviewQuality {
        use_proxies: false,
        long_edge: None,
    };

    /// A preview that may use proxies for a picture drawn `long_edge` pixels on its long side.
    #[must_use]
    pub const fn proxies(long_edge: Option<u32>) -> PreviewQuality {
        PreviewQuality {
            use_proxies: true,
            long_edge,
        }
    }
}

/// Which rendition of a media item is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rendition {
    /// The file the project names.
    Original,
    /// Its proxy from the cache.
    Proxy,
}

/// A proxy file, as much as the resolver needs to know of it.
pub trait ProxyFile {
    /// The longest side of its pictures, in pixels.
    fn long_edge(&self) -> u32;
}

impl<P: ProxyFile + ?Sized> ProxyFile for std::sync::Arc<P> {
    fn long_edge(&self) -> u32 {
        (**self).long_edge()
    }
}

/// Finds the ready proxy of a media item `M`.
pub trait ProxyLookup<M: ?Sized> {
    /// The proxy it hands out.
    type Proxy: ProxyFile;

    /// The proxy of `media` the preview may read now: ready, its file there, the item's
    /// proxies not turned off, and the original on this computer. `None` otherwise.
    fn ready_proxy(&self, media: &M) -> Option<Self::Proxy>;
}

impl<M: ?Sized, L: ProxyLookup<M> + ?Sized> ProxyLookup<M> for std::sync::Arc<L> {
    type Proxy = L::Proxy;
    fn ready_proxy(&self, media: &M) -> Option<L::Proxy> {
        (**self).ready_proxy(media)
    }
}

/// A lookup that never finds a proxy (an engine without a proxy cache, an export).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoProxies;

/// The proxy [`NoProxies`] never hands out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoProxy {}

impl ProxyFile for NoProxy {
    fn long_edge(&self) -> u32 {
        match *self {}
    }
}

impl<M: ?Sized> ProxyLookup<M> for NoProxies {
    type Proxy = NoProxy;
    fn ready_proxy(&self, _media: &M) -> Option<NoProxy> {
        None
    }
}

/// What [`MediaResolver::open`] answers: read the original, or this proxy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaSource<P> {
    /// The file the project names.
    Original,
    /// The item's proxy.
    Proxy(P),
}

impl<P> MediaSource<P> {
    /// Which rendition this is.
    #[must_use]
    pub const fn rendition(&self) -> Rendition {
        match self {
            MediaSource::Original => Rendition::Original,
            MediaSource::Proxy(_) => Rendition::Proxy,
        }
    }
}

/// Decides which file a reader opens (see the module docs).
#[derive(Clone, Copy, Debug, Default)]
pub struct MediaResolver<L = NoProxies> {
    proxies: Option<L>,
}

impl MediaResolver<NoProxies> {
    /// A resolver without proxies: every purpose reads the original.
    #[must_use]
    pub const fn originals() -> MediaResolver<NoProxies> {
        MediaResolver { proxies: None }
    }
}

impl<L> MediaResolver<L> {
    /// A resolver over the proxies `proxies` finds (`None`: there are none).
    #[must_use]
    pub const fn new(proxies: Option<L>) -> MediaResolver<L> {
        MediaResolver { proxies }
    }

    /// The proxy lookup.
    #[must_use]
    pub const fn proxies(&self) -> Option<&L> {
        self.proxies.as_ref()
    }

    /// The resolution rule: whether a proxy whose long side is `proxy_long_edge` pixels serves
    /// `purpose`. Never for export, thumbnails or analysis; for the preview when it uses
    /// proxies and draws the picture no larger than the proxy.
    #[must_use]
    pub const fn serves(purpose: MediaPurpose, proxy_long_edge: u32) -> bool {
        match purpose {
            MediaPurpose::Preview(PreviewQuality {
                use_proxies: true,
                long_edge,
            }) => match long_edge {
                Some(needed) => needed <= proxy_long_edge,
                None => true,
            },
            MediaPurpose::Preview(_)
            | MediaPurpose::Export
            | MediaPurpose::Thumbnail
            | MediaPurpose::Analysis => false,
        }
    }

    /// The proxy of `media` a reader with `purpose` could be given, before the resolution rule
    /// ([`serves`](Self::serves)): looked up only for a purpose that may use one. A reader
    /// that renders many frames keeps it and asks `serves` per frame.
    pub fn candidate<M: ?Sized>(&self, media: &M, purpose: MediaPurpose) -> Option<L::Proxy>
    where
        L: ProxyLookup<M>,
    {
        if !purpose.may_use_proxy() {
            return None;
        }
        self.proxies.as_ref()?.ready_proxy(media)
    }

    /// Which file a reader of `media` with `purpose` opens.
    pub fn open<M: ?Sized>(&self, media: &M, purpose: MediaPurpose) -> MediaSource<L::Proxy>
    where
        L: ProxyLookup<M>,
    {
        match self.candidate(media, purpose) {
            Some(proxy) if Self::serves(purpose, proxy.long_edge()) => MediaSource::Proxy(proxy),
            _ => MediaSource::Original,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::cell::Cell;

    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Proxy(u32);

    impl ProxyFile for Proxy {
        fn long_edge(&self) -> u32 {
            self.0
        }
    }

    /// Finds a proxy of the given size for the media named "proxied"; counts the lookups.
    struct Store {
        size: u32,
        lookups: Cell<u32>,
    }

    impl ProxyLookup<str> for Store {
        type Proxy = Proxy;
        fn ready_proxy(&self, media: &str) -> Option<Proxy> {
            self.lookups.set(self.lookups.get() + 1);
            (media == "proxied").then_some(Proxy(self.size))
        }
    }

    fn resolver() -> MediaResolver<Store> {
        MediaResolver::new(Some(Store {
            size: 960,
            lookups: Cell::new(0),
        }))
    }

    fn preview(long_edge: Option<u32>) -> MediaPurpose {
        MediaPurpose::Preview(PreviewQuality::proxies(long_edge))
    }

    #[test]
    fn export_thumbnails_and_analysis_read_the_original_without_looking_a_proxy_up() {
        let r = resolver();
        for purpose in [
            MediaPurpose::Export,
            MediaPurpose::Thumbnail,
            MediaPurpose::Analysis,
            MediaPurpose::Preview(PreviewQuality::ORIGINALS),
            // The toggle is off: the size asked for changes nothing.
            MediaPurpose::Preview(PreviewQuality {
                use_proxies: false,
                long_edge: Some(100),
            }),
        ] {
            assert!(!purpose.may_use_proxy(), "{purpose:?}");
            assert_eq!(r.open("proxied", purpose), MediaSource::Original);
            assert_eq!(r.candidate("proxied", purpose), None);
            assert!(!MediaResolver::<Store>::serves(purpose, u32::MAX));
        }
        assert_eq!(r.proxies().unwrap().lookups.get(), 0);
    }

    #[test]
    fn the_preview_reads_the_proxy_when_it_holds_the_pixels_the_picture_is_drawn_at() {
        let r = resolver();
        // Smaller than, and exactly, the proxy: the proxy.
        for needed in [1, 480, 959, 960] {
            let source = r.open("proxied", preview(Some(needed)));
            assert_eq!(source, MediaSource::Proxy(Proxy(960)), "{needed}");
            assert_eq!(source.rendition(), Rendition::Proxy);
        }
        // A full-resolution preview that needs more than the proxy has: the original.
        for needed in [961, 1920, 3840] {
            let source = r.open("proxied", preview(Some(needed)));
            assert_eq!(source, MediaSource::Original, "{needed}");
            assert_eq!(source.rendition(), Rendition::Original);
        }
        // No size asked for (reverse playback): the proxy.
        assert_eq!(
            r.open("proxied", preview(None)),
            MediaSource::Proxy(Proxy(960))
        );
    }

    #[test]
    fn a_missing_proxy_falls_back_to_the_original() {
        let r = resolver();
        assert_eq!(r.open("plain", preview(Some(100))), MediaSource::Original);
        assert_eq!(r.candidate("plain", preview(None)), None);
        // No proxy cache at all.
        let none = MediaResolver::<Store>::new(None);
        assert_eq!(none.open("proxied", preview(None)), MediaSource::Original);
        let originals = MediaResolver::originals();
        assert_eq!(
            originals.open("proxied", preview(None)),
            MediaSource::Original
        );
        assert!(originals.proxies().is_none());
    }

    #[test]
    fn a_reader_keeps_its_candidate_and_asks_the_rule_per_frame() {
        let r = resolver();
        let held = r.candidate("proxied", preview(Some(4000))).unwrap();
        assert_eq!(r.proxies().unwrap().lookups.get(), 1);
        assert!(!MediaResolver::<Store>::serves(
            preview(Some(4000)),
            held.long_edge()
        ));
        assert!(MediaResolver::<Store>::serves(
            preview(Some(400)),
            held.long_edge()
        ));
        assert_eq!(r.proxies().unwrap().lookups.get(), 1);
    }
}
