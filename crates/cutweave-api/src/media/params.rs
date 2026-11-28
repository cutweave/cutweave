// SPDX-License-Identifier: GPL-3.0-or-later
//! Query parsing and bounds checks of the media routes. Every problem is a
//! [`MediaError::Invalid`] naming the parameter (HTTP 422). Unknown parameters are refused, so a
//! typo (`width` for `w`) is an error rather than a silent default; `exp` and `sig` belong to
//! signed URLs and are skipped.

use std::collections::BTreeMap;

use super::{
    FilmstripFormat, FilmstripRequest, FrameFormat, FrameRequest, MAX_DIMENSION,
    MAX_FILMSTRIP_TILES, MAX_SHEET_FRAMES, MAX_WAVEFORM_LOD, MediaError, SHEET_SIZES, SheetRequest,
    ThumbRequest, TimeParam, WaveformFormat, WaveformRequest, WaveformTrack,
};
use crate::media::sign::{EXPIRES_PARAM, SIGNATURE_PARAM};

/// Default thumbnail width (the poster the import analysis caches).
pub const DEFAULT_THUMB_WIDTH: u32 = 320;
/// Default filmstrip tile width (the strip the import analysis caches).
pub const DEFAULT_TILE_WIDTH: u32 = 160;
/// Default filmstrip tile count (the strip the import analysis caches).
pub const DEFAULT_TILE_COUNT: u32 = 100;
/// Default size of a waveform PNG.
pub const DEFAULT_WAVEFORM_PNG: (u32, u32) = (1024, 128);
/// Longest accepted time text (`2.5s`, `f:60`, a timecode).
const MAX_TIME_TEXT: usize = 64;

/// The decoded query of one request.
pub(super) struct Query {
    pairs: BTreeMap<String, String>,
}

fn invalid(param: &str, message: impl Into<String>) -> MediaError {
    MediaError::Invalid {
        param: param.to_owned(),
        message: message.into(),
    }
}

impl Query {
    /// Decodes `raw` and checks every name against `allowed`.
    pub(super) fn parse(raw: Option<&str>, allowed: &[&str]) -> Result<Query, MediaError> {
        let mut pairs = BTreeMap::new();
        for (key, value) in form_urlencoded::parse(raw.unwrap_or("").as_bytes()) {
            if key == EXPIRES_PARAM || key == SIGNATURE_PARAM {
                continue;
            }
            if !allowed.contains(&key.as_ref()) {
                return Err(invalid(
                    &key,
                    format!("unknown parameter (expected one of {})", allowed.join(", ")),
                ));
            }
            if pairs.insert(key.to_string(), value.into_owned()).is_some() {
                return Err(invalid(&key, "given more than once"));
            }
        }
        Ok(Query { pairs })
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.pairs.get(name).map(String::as_str)
    }

    fn uint(&self, name: &str, lo: u32, hi: u32) -> Result<Option<u32>, MediaError> {
        let Some(raw) = self.get(name) else {
            return Ok(None);
        };
        match raw.parse::<u64>() {
            Ok(v) if (u64::from(lo)..=u64::from(hi)).contains(&v) => {
                Ok(Some(u32::try_from(v).unwrap_or(hi)))
            }
            _ => Err(invalid(
                name,
                format!("must be an integer within {lo}..={hi}"),
            )),
        }
    }

    fn dimension(&self, name: &str) -> Result<Option<u32>, MediaError> {
        self.uint(name, 1, MAX_DIMENSION)
    }

    fn time(&self, name: &str) -> Result<Option<TimeParam>, MediaError> {
        self.get(name).map(|raw| time_of(name, raw)).transpose()
    }

    fn sequence(&self) -> Result<Option<String>, MediaError> {
        match self.get("seq") {
            None => Ok(None),
            Some(s) if !s.is_empty() && s.len() <= 128 && s.is_ascii() => Ok(Some(s.to_owned())),
            Some(_) => Err(invalid("seq", "must be a sequence id")),
        }
    }

    fn hash(&self) -> Result<Option<String>, MediaError> {
        match self.get("hash") {
            None => Ok(None),
            Some(h) if !h.is_empty() && h.len() <= 128 && h.is_ascii() => Ok(Some(h.to_owned())),
            Some(_) => Err(invalid(
                "hash",
                "must be a content hash (blake3-fast:<hex>)",
            )),
        }
    }
}

/// One time of the parameter `name`: integer flicks, or the text of a `TimeInput`.
fn time_of(name: &str, raw: &str) -> Result<TimeParam, MediaError> {
    if let Ok(flicks) = raw.parse::<i64>() {
        return Ok(TimeParam::Flicks(flicks));
    }
    if raw.is_empty() || raw.len() > MAX_TIME_TEXT {
        return Err(invalid(
            name,
            "must be flicks, seconds (\"2.5s\"), a frame (\"f:60\") or a timecode",
        ));
    }
    Ok(TimeParam::Text(raw.to_owned()))
}

/// `GET /media/{id}/thumb?t=&w=&hash=`.
pub(super) fn thumb(media: String, raw: Option<&str>) -> Result<ThumbRequest, MediaError> {
    let q = Query::parse(raw, &["t", "w", "hash"])?;
    Ok(ThumbRequest {
        media,
        t: q.time("t")?.unwrap_or(TimeParam::Flicks(0)),
        w: q.dimension("w")?.unwrap_or(DEFAULT_THUMB_WIDTH),
        hash: q.hash()?,
    })
}

/// `GET /media/{id}/filmstrip?w=&h=&count=&hash=&fmt=jpeg|json`.
pub(super) fn filmstrip(media: String, raw: Option<&str>) -> Result<FilmstripRequest, MediaError> {
    let q = Query::parse(raw, &["w", "h", "count", "hash", "fmt"])?;
    let format = match q.get("fmt") {
        None | Some("jpeg") => FilmstripFormat::Jpeg,
        Some("json") => FilmstripFormat::Manifest,
        Some(_) => return Err(invalid("fmt", "must be `jpeg` or `json`")),
    };
    Ok(FilmstripRequest {
        media,
        w: q.dimension("w")?.unwrap_or(DEFAULT_TILE_WIDTH),
        h: q.dimension("h")?,
        count: q
            .uint("count", 1, MAX_FILMSTRIP_TILES)?
            .unwrap_or(DEFAULT_TILE_COUNT),
        hash: q.hash()?,
        format,
    })
}

/// `GET /media/{id}/waveform?lod=&start=&end=&track=&stream=&hash=&fmt=peaks|png&w=&h=`.
pub(super) fn waveform(media: String, raw: Option<&str>) -> Result<WaveformRequest, MediaError> {
    let q = Query::parse(
        raw,
        &[
            "lod", "start", "end", "track", "stream", "hash", "fmt", "w", "h",
        ],
    )?;
    let track = match q.get("track") {
        None | Some("mix") => WaveformTrack::Mix,
        Some(raw) => raw
            .parse::<u16>()
            .map(WaveformTrack::Channel)
            .map_err(|_| invalid("track", "must be `mix` or a 0-based channel number"))?,
    };
    let format = match q.get("fmt") {
        None | Some("peaks") => {
            for name in ["w", "h"] {
                if q.get(name).is_some() {
                    return Err(invalid(name, "only applies to `fmt=png`"));
                }
            }
            WaveformFormat::Peaks
        }
        Some("png") => WaveformFormat::Png {
            width: q.dimension("w")?.unwrap_or(DEFAULT_WAVEFORM_PNG.0),
            height: q.dimension("h")?.unwrap_or(DEFAULT_WAVEFORM_PNG.1),
        },
        Some(_) => return Err(invalid("fmt", "must be `peaks` or `png`")),
    };
    let lod = q.uint("lod", 0, u32::from(MAX_WAVEFORM_LOD))?.unwrap_or(0);
    Ok(WaveformRequest {
        media,
        lod: u8::try_from(lod).unwrap_or(MAX_WAVEFORM_LOD),
        start: q.time("start")?.unwrap_or(TimeParam::Flicks(0)),
        end: q.time("end")?,
        track,
        stream: q.uint("stream", 0, u32::MAX)?,
        hash: q.hash()?,
        format,
    })
}

/// `GET /media/frame?seq=&t=&w=&h=&fmt=jpeg|png&q=`.
pub(super) fn frame(raw: Option<&str>) -> Result<FrameRequest, MediaError> {
    let q = Query::parse(raw, &["seq", "t", "w", "h", "fmt", "q"])?;
    let format = match q.get("fmt") {
        None | Some("jpeg") => FrameFormat::Jpeg,
        Some("png") => FrameFormat::Png,
        Some(_) => return Err(invalid("fmt", "must be `jpeg` or `png`")),
    };
    let quality = q.uint("q", 1, 100)?.map(|v| u8::try_from(v).unwrap_or(100));
    if quality.is_some() && format == FrameFormat::Png {
        return Err(invalid("q", "only applies to `fmt=jpeg`"));
    }
    Ok(FrameRequest {
        sequence: q.sequence()?,
        t: q.time("t")?
            .ok_or_else(|| invalid("t", "is required (the timeline time to render)"))?,
        w: q.dimension("w")?,
        h: q.dimension("h")?,
        format,
        quality,
    })
}

/// `GET /media/sheet?seq=&at=&columns=&size=&labels=0|1`: `at` is the comma-separated list of
/// the frames' timeline times.
pub(super) fn sheet(raw: Option<&str>) -> Result<SheetRequest, MediaError> {
    let q = Query::parse(raw, &["seq", "at", "columns", "size", "labels"])?;
    let at = q
        .get("at")
        .ok_or_else(|| invalid("at", "is required (the timeline times of the frames)"))?
        .split(',')
        .map(|t| time_of("at", t))
        .collect::<Result<Vec<TimeParam>, MediaError>>()?;
    if at.len() > MAX_SHEET_FRAMES {
        return Err(invalid(
            "at",
            format!("must name at most {MAX_SHEET_FRAMES} times"),
        ));
    }
    let labels = match q.get("labels") {
        None | Some("1") => true,
        Some("0") => false,
        Some(_) => return Err(invalid("labels", "must be `0` or `1`")),
    };
    Ok(SheetRequest {
        sequence: q.sequence()?,
        at,
        columns: q.uint(
            "columns",
            1,
            u32::try_from(MAX_SHEET_FRAMES).unwrap_or(u32::MAX),
        )?,
        size: q.uint("size", SHEET_SIZES.0, SHEET_SIZES.1)?,
        labels,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn param_of(e: MediaError) -> String {
        match e {
            MediaError::Invalid { param, .. } => param,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn thumbs_default_to_the_cached_poster() {
        let t = thumb("med_1".into(), None).unwrap();
        assert_eq!((t.t, t.w, t.hash), (TimeParam::Flicks(0), 320, None));
        let t = thumb("med_1".into(), Some("t=2.5s&w=160&hash=blake3-fast%3Aab")).unwrap();
        assert_eq!(t.t, TimeParam::Text("2.5s".into()));
        assert_eq!(t.hash.as_deref(), Some("blake3-fast:ab"));
    }

    #[test]
    fn bounds_and_unknown_parameters_are_422() {
        for (q, param) in [
            ("w=100000", "w"),
            ("w=0", "w"),
            ("w=-1", "w"),
            ("w=12px", "w"),
            ("width=10", "width"),
            ("w=1&w=2", "w"),
            ("t=", "t"),
        ] {
            assert_eq!(
                param_of(thumb("m".into(), Some(q)).unwrap_err()),
                param,
                "{q}"
            );
        }
        assert_eq!(
            param_of(filmstrip("m".into(), Some("count=1001")).unwrap_err()),
            "count"
        );
        assert_eq!(
            param_of(waveform("m".into(), Some("lod=12")).unwrap_err()),
            "lod"
        );
        assert_eq!(
            param_of(waveform("m".into(), Some("w=100")).unwrap_err()),
            "w"
        );
        assert_eq!(param_of(frame(Some("w=10")).unwrap_err()), "t");
        assert_eq!(param_of(frame(Some("t=0&fmt=png&q=5")).unwrap_err()), "q");
        assert_eq!(param_of(frame(Some("t=0&h=4097")).unwrap_err()), "h");
    }

    #[test]
    fn sheets_take_a_list_of_times() {
        let s = sheet(Some("at=0,2.5s,f%3A60,00%3A00%3A04%3A00")).unwrap();
        assert_eq!(
            s.at,
            [
                TimeParam::Flicks(0),
                TimeParam::Text("2.5s".into()),
                TimeParam::Text("f:60".into()),
                TimeParam::Text("00:00:04:00".into()),
            ]
        );
        assert_eq!(
            (s.sequence, s.columns, s.size, s.labels),
            (None, None, None, true)
        );
        let s = sheet(Some("seq=seq_1&at=705600000&columns=4&size=2048&labels=0")).unwrap();
        assert_eq!(s.sequence.as_deref(), Some("seq_1"));
        assert_eq!((s.columns, s.size, s.labels), (Some(4), Some(2048), false));
        let many = vec!["0"; MAX_SHEET_FRAMES + 1].join(",");
        for (q, param) in [
            (String::new(), "at"),
            ("at=".into(), "at"),
            ("at=0,,1".into(), "at"),
            (format!("at={many}"), "at"),
            ("at=0&size=63".into(), "size"),
            ("at=0&size=2049".into(), "size"),
            ("at=0&columns=0".into(), "columns"),
            ("at=0&columns=65".into(), "columns"),
            ("at=0&labels=yes".into(), "labels"),
            ("at=0&seq=".into(), "seq"),
            ("at=0&w=100".into(), "w"),
        ] {
            assert_eq!(param_of(sheet(Some(&q)).unwrap_err()), param, "{q}");
        }
        let most = format!("at={}", &many[2..]);
        assert_eq!(sheet(Some(&most)).unwrap().at.len(), MAX_SHEET_FRAMES);
    }

    #[test]
    fn signed_url_parameters_are_skipped() {
        let t = thumb("m".into(), Some("w=10&exp=5&sig=abc")).unwrap();
        assert_eq!(t.w, 10);
    }

    #[test]
    fn waveform_and_frame_forms() {
        let w = waveform(
            "m".into(),
            Some("lod=3&start=0&end=705600000&track=1&fmt=png&w=200&h=50"),
        )
        .unwrap();
        assert_eq!(w.lod, 3);
        assert_eq!(w.track, WaveformTrack::Channel(1));
        assert_eq!(
            w.format,
            WaveformFormat::Png {
                width: 200,
                height: 50
            }
        );
        let f = frame(Some("seq=seq_1&t=1411200000&w=640&h=360&fmt=jpeg&q=90")).unwrap();
        assert_eq!(f.sequence.as_deref(), Some("seq_1"));
        assert_eq!(f.t, TimeParam::Flicks(1_411_200_000));
        assert_eq!((f.w, f.h, f.quality), (Some(640), Some(360), Some(90)));
    }
}
