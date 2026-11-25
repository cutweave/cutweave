// SPDX-License-Identifier: GPL-3.0-or-later
//! [`ProxyProfile`]: what a proxy is made of (codec and largest side), and its cache key.

use std::fmt;

use cutweave_media::{Capabilities, PixelFormat};

/// The intra-frame codec of a proxy, in the order the generator prefers them (WP-2F.01): ProRes
/// Proxy, then DNxHR LB, then MJPEG. Every frame is a keyframe, so a seek decodes one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProxyCodec {
    /// Apple ProRes 422 Proxy (10-bit 4:2:2).
    ProRes,
    /// Avid DNxHR LB (8-bit 4:2:2).
    DnxHr,
    /// Motion JPEG (8-bit 4:2:2).
    Mjpeg,
}

impl ProxyCodec {
    /// Every codec, best first.
    pub const ALL: [ProxyCodec; 3] = [ProxyCodec::ProRes, ProxyCodec::DnxHr, ProxyCodec::Mjpeg];

    /// The codec-id name the media backend encodes (`prores`, `dnxhd`, `mjpeg`).
    #[must_use]
    pub const fn codec(self) -> &'static str {
        match self {
            ProxyCodec::ProRes => "prores",
            ProxyCodec::DnxHr => "dnxhd",
            ProxyCodec::Mjpeg => "mjpeg",
        }
    }

    /// The encoder implementations that make this flavour (FFmpeg names).
    const fn encoders(self) -> &'static [&'static str] {
        match self {
            ProxyCodec::ProRes => &["prores_ks", "prores_videotoolbox", "prores"],
            ProxyCodec::DnxHr => &["dnxhd"],
            ProxyCodec::Mjpeg => &["mjpeg"],
        }
    }

    /// The layout frames are decoded into for the encoder.
    #[must_use]
    pub const fn input_format(self) -> PixelFormat {
        match self {
            ProxyCodec::ProRes => PixelFormat::Yuv422p10,
            ProxyCodec::DnxHr | ProxyCodec::Mjpeg => PixelFormat::Yuv422p,
        }
    }

    /// The name in a profile key (`prores_proxy`, `dnxhr_lb`, `mjpeg`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            ProxyCodec::ProRes => "prores_proxy",
            ProxyCodec::DnxHr => "dnxhr_lb",
            ProxyCodec::Mjpeg => "mjpeg",
        }
    }

    /// The best codec `caps` can encode, if any.
    #[must_use]
    pub fn choose(caps: &Capabilities) -> Option<ProxyCodec> {
        ProxyCodec::ALL.into_iter().find(|c| {
            caps.codec(c.codec()).is_some_and(|s| {
                s.encoders
                    .iter()
                    .any(|e| c.encoders().contains(&e.as_str()) || e == c.codec())
            })
        })
    }
}

impl fmt::Display for ProxyCodec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// How a proxy is made: its codec and the largest side of its pictures. Its [`key`](Self::key)
/// (`prores_proxy-960`) and the original's content hash key the proxy in the cache, so another
/// profile is another proxy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProxyProfile {
    /// The codec.
    pub codec: ProxyCodec,
    /// The longest side of the proxy's pictures, in pixels (never larger than the original's).
    pub long_edge: u32,
}

impl ProxyProfile {
    /// The default longest side: 960 pixels (960 × 540 for 16:9 video, WP-2F.01).
    pub const DEFAULT_LONG_EDGE: u32 = 960;

    /// A profile with the default size.
    #[must_use]
    pub const fn new(codec: ProxyCodec) -> ProxyProfile {
        ProxyProfile {
            codec,
            long_edge: ProxyProfile::DEFAULT_LONG_EDGE,
        }
    }

    /// The best codec `caps` can encode at the default size; `None` when it encodes none.
    #[must_use]
    pub fn for_backend(caps: &Capabilities) -> Option<ProxyProfile> {
        ProxyCodec::choose(caps).map(ProxyProfile::new)
    }

    /// The cache key (`prores_proxy-960`).
    #[must_use]
    pub fn key(&self) -> String {
        format!("{}-{}", self.codec.name(), self.long_edge)
    }

    /// The proxy picture size for a `width × height` original (coded orientation): scaled
    /// down, aspect kept, so the longest side is at most [`long_edge`](Self::long_edge), never
    /// scaled up, and both sides even (4:2:2) and at least 2.
    #[must_use]
    pub fn size_for(&self, (width, height): (u32, u32)) -> (u32, u32) {
        let long = width.max(height).max(1);
        let target = self.long_edge.max(2);
        let (w, h) = if long <= target {
            (width, height)
        } else {
            let scale = |side: u32| {
                // Rounded to nearest: side × target / long.
                (u64::from(side) * u64::from(target) + u64::from(long) / 2) / u64::from(long)
            };
            (
                u32::try_from(scale(width)).unwrap_or(target),
                u32::try_from(scale(height)).unwrap_or(target),
            )
        };
        let even = |v: u32| (v & !1).max(2);
        (even(w), even(h))
    }
}

impl fmt::Display for ProxyProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.key())
    }
}

#[cfg(test)]
mod tests {
    use cutweave_media::{CodecSupport, FakeBackend, MediaBackend};
    use cutweave_model::StreamKind;

    use super::*;

    fn caps(encoders: &[(&str, &str)]) -> Capabilities {
        Capabilities::with_codecs(encoders.iter().map(|(codec, encoder)| CodecSupport {
            name: (*codec).to_owned(),
            kind: StreamKind::Video,
            decoders: vec![(*codec).to_owned()],
            encoders: vec![(*encoder).to_owned()],
        }))
    }

    #[test]
    fn sizes_keep_the_aspect_fit_the_long_edge_and_stay_even() {
        let p = ProxyProfile::new(ProxyCodec::ProRes);
        assert_eq!(p.size_for((3840, 2160)), (960, 540));
        assert_eq!(p.size_for((1920, 1080)), (960, 540));
        assert_eq!(p.size_for((1080, 1920)), (540, 960));
        assert_eq!(p.size_for((4096, 2160)), (960, 506));
        assert_eq!(p.size_for((2704, 1520)), (960, 540));
        // Never scaled up; odd sides become even.
        assert_eq!(p.size_for((640, 360)), (640, 360));
        assert_eq!(p.size_for((99, 57)), (98, 56));
        assert_eq!(p.size_for((1, 1)), (2, 2));
        assert_eq!(p.key(), "prores_proxy-960");
        assert_eq!(
            ProxyProfile {
                codec: ProxyCodec::Mjpeg,
                long_edge: 1280
            }
            .to_string(),
            "mjpeg-1280"
        );
    }

    #[test]
    fn the_best_codec_the_backend_encodes_wins() {
        let all = caps(&[
            ("mjpeg", "mjpeg"),
            ("dnxhd", "dnxhd"),
            ("prores", "prores_ks"),
        ]);
        assert_eq!(ProxyCodec::choose(&all), Some(ProxyCodec::ProRes));
        let no_prores = caps(&[("mjpeg", "mjpeg"), ("dnxhd", "dnxhd")]);
        assert_eq!(ProxyCodec::choose(&no_prores), Some(ProxyCodec::DnxHr));
        assert_eq!(
            ProxyCodec::choose(&caps(&[("mjpeg", "mjpeg")])),
            Some(ProxyCodec::Mjpeg)
        );
        // The fake backend encodes only raw video: no proxies.
        assert_eq!(
            ProxyProfile::for_backend(&FakeBackend::new().capabilities()),
            None
        );
    }
}
