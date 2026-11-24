// SPDX-License-Identifier: GPL-3.0-or-later
//! [`Capabilities`]: what a backend can decode and encode.
//!
//! Codecs are named by FFmpeg's codec-id names (`h264`, `hevc`, `prores`, `aac`), the same names
//! the probe reports in `StreamInfo::codec`, so a caller can ask "can I decode this stream?"
//! with the probe result in hand.

use cutweave_model::StreamKind;

/// One codec and the software implementations a backend has for it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CodecSupport {
    /// Codec-id name (`h264`).
    pub name: String,
    /// What the codec carries.
    pub kind: StreamKind,
    /// Software decoder implementations, sorted (`h264`; `libdav1d` for `av1`).
    pub decoders: Vec<String>,
    /// Software encoder implementations, sorted (`libx264` for `h264`).
    pub encoders: Vec<String>,
}

/// A codec a hardware API can handle on this machine.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HwCodec {
    /// FFmpeg hardware device type (`d3d11va`, `cuda`, `amf`, `videotoolbox`, `vaapi`, `qsv`).
    /// Decoders that pick a GPU by vendor name it too (`d3d11va:nvidia`, `d3d11va:amd`): the
    /// spelling `CUTWEAVE_HW` and `FfmpegBackend::with_hw_decoders` take.
    pub device: String,
    /// Codec-id name (`h264`).
    pub codec: String,
    /// The FFmpeg implementation: for encoders the name [`HwEncode::Force`](crate::HwEncode::Force)
    /// takes (`h264_nvenc`, `h264_amf`, `h264_qsv`, `h264_videotoolbox`, `h264_vaapi`); for
    /// decoders the codec and hwaccel (`h264_nvdec`, `hevc_d3d11va`, `h264_videotoolbox`,
    /// `hevc_vaapi`), as [`VideoDecoder::hw_decoder`](crate::VideoDecoder::hw_decoder) reports.
    pub implementation: String,
}

/// What a backend can do. Deterministic for a given backend and machine; codecs are sorted by
/// name with no duplicates (the backend contract checks both).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Capabilities {
    /// Hardware decoders verified on this machine (WP-1D.04): each device opened and decoded a
    /// test stream of the codec bit-exactly like software. Probed once per process on first use,
    /// in the order [`HwMode::Auto`](crate::HwMode::Auto) tries the devices. Empty without one,
    /// and with hardware decode off.
    pub hw_decode: Vec<HwCodec>,
    /// Hardware encoders that completed a 256×256 test encode on this machine (probed once per
    /// process, WP-1E.02), in the order [`HwEncode::Auto`](crate::HwEncode::Auto) tries them.
    /// Empty on a machine without one.
    pub hw_encode: Vec<HwCodec>,
    /// Software codecs, sorted by name.
    pub codecs: Vec<CodecSupport>,
}

impl Capabilities {
    /// Capabilities with the given software codecs (sorted and merged by name) and no hardware
    /// paths.
    #[must_use]
    pub fn with_codecs(codecs: impl IntoIterator<Item = CodecSupport>) -> Capabilities {
        let mut merged: Vec<CodecSupport> = Vec::new();
        for c in codecs {
            match merged.iter_mut().find(|m| m.name == c.name) {
                Some(m) => {
                    m.decoders.extend(c.decoders);
                    m.encoders.extend(c.encoders);
                }
                None => merged.push(c),
            }
        }
        for m in &mut merged {
            m.decoders.sort();
            m.decoders.dedup();
            m.encoders.sort();
            m.encoders.dedup();
        }
        merged.sort_by(|a, b| a.name.cmp(&b.name));
        Capabilities {
            hw_decode: Vec::new(),
            hw_encode: Vec::new(),
            codecs: merged,
        }
    }

    /// The entry for codec `name`.
    #[must_use]
    pub fn codec(&self, name: &str) -> Option<&CodecSupport> {
        self.codecs
            .binary_search_by(|c| c.name.as_str().cmp(name))
            .ok()
            .and_then(|i| self.codecs.get(i))
    }

    /// Whether a software decoder for codec `name` exists.
    #[must_use]
    pub fn can_decode(&self, name: &str) -> bool {
        self.codec(name).is_some_and(|c| !c.decoders.is_empty())
    }

    /// Whether a software encoder for codec `name` exists.
    #[must_use]
    pub fn can_encode(&self, name: &str) -> bool {
        self.codec(name).is_some_and(|c| !c.encoders.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codec(name: &str, decoders: &[&str], encoders: &[&str]) -> CodecSupport {
        CodecSupport {
            name: name.to_owned(),
            kind: StreamKind::Video,
            decoders: decoders.iter().map(|s| (*s).to_owned()).collect(),
            encoders: encoders.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn implementations_merge_by_codec_and_sort() {
        let caps = Capabilities::with_codecs([
            codec("hevc", &["hevc"], &[]),
            codec("h264", &[], &["libx264"]),
            codec("h264", &["h264"], &[]),
            codec("av1", &["libdav1d", "av1"], &["libsvtav1"]),
        ]);
        let names: Vec<_> = caps.codecs.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["av1", "h264", "hevc"]);
        assert_eq!(
            caps.codec("av1").map(|c| c.decoders.clone()),
            Some(vec!["av1".to_owned(), "libdav1d".to_owned()])
        );
        assert!(caps.can_decode("h264") && caps.can_encode("h264"));
        assert!(caps.can_decode("hevc") && !caps.can_encode("hevc"));
        assert!(!caps.can_decode("prores"));
        assert!(caps.hw_decode.is_empty() && caps.hw_encode.is_empty());
    }
}
