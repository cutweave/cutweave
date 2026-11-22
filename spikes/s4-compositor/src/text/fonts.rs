//! Bundled fonts (all SIL OFL 1.1) → a `fontdb` database and a `cosmic-text` font system.
//!
//! No system fonts are ever loaded: text must render identically on every machine (§8
//! determinism). Latin comes from the UI's woff2 files (`packages/ui/src/fonts`, decoded here);
//! the RTL, CJK and emoji samples come from Noto subsets in `assets/fonts` (OFL texts beside
//! them; provenance in the crate README).

use std::collections::HashMap;
use std::sync::Arc;

use cosmic_text::{FontSystem, fontdb};

use crate::hash::Fnv;
use crate::{Error, Result};

/// Family used when a requested family is not bundled (like a CSS generic fallback).
pub const DEFAULT_FAMILY: &str = "Instrument Sans";
/// Display family used by the caption demo.
pub const CAPTION_FAMILY: &str = "Bricolage Grotesque";

const BRICOLAGE: &[u8] = include_bytes!(
    "../../../../packages/ui/src/fonts/bricolagegrotesque/bricolage-grotesque-latin.woff2"
);
const INSTRUMENT: &[u8] =
    include_bytes!("../../../../packages/ui/src/fonts/instrumentsans/instrument-sans-latin.woff2");
const NOTO_ARABIC: &[u8] = include_bytes!("../../assets/fonts/NotoSansArabic-subset.ttf");
const NOTO_SC: &[u8] = include_bytes!("../../assets/fonts/NotoSansSC-subset.ttf");
const NOTO_EMOJI: &[u8] = include_bytes!("../../assets/fonts/NotoEmoji-subset.ttf");

/// Raw font file of one face.
#[derive(Clone, Debug)]
pub struct FaceData {
    pub data: Arc<Vec<u8>>,
    pub index: u32,
    pub family: String,
    /// FNV-1a of the file bytes (stable across runs; part of glyph cache keys).
    pub content_hash: u64,
}

/// `"Bricolage Grotesque 800"` → (`"Bricolage Grotesque"`, 800). A trailing number is the weight.
#[must_use]
pub fn parse_font_spec(spec: &str) -> (String, u16) {
    let spec = spec.trim();
    if let Some((family, last)) = spec.rsplit_once(' ')
        && let Ok(w) = last.parse::<u16>()
        && (1..=1000).contains(&w)
    {
        return (family.trim().to_string(), w);
    }
    (spec.to_string(), 400)
}

/// The spike's font library.
pub struct FontLibrary {
    pub system: FontSystem,
    faces: HashMap<fontdb::ID, FaceData>,
    families: Vec<String>,
}

impl std::fmt::Debug for FontLibrary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontLibrary")
            .field("families", &self.families)
            .finish_non_exhaustive()
    }
}

fn decode(name: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.starts_with(b"wOF2") {
        wuff::decompress_woff2(bytes)
            .map_err(|e| Error::Font(format!("{name}: woff2 decode: {e:?}")))
    } else {
        Ok(bytes.to_vec())
    }
}

impl FontLibrary {
    /// Loads every bundled face. Deterministic order (ids are assigned in load order).
    pub fn bundled() -> Result<Self> {
        let sources: [(&str, &[u8]); 5] = [
            ("bricolage-grotesque-latin.woff2", BRICOLAGE),
            ("instrument-sans-latin.woff2", INSTRUMENT),
            ("NotoSansArabic-subset.ttf", NOTO_ARABIC),
            ("NotoSansSC-subset.ttf", NOTO_SC),
            ("NotoEmoji-subset.ttf", NOTO_EMOJI),
        ];
        let mut db = fontdb::Database::new();
        let mut faces = HashMap::new();
        let mut families = Vec::new();
        for (name, bytes) in sources {
            let data = Arc::new(decode(name, bytes)?);
            let content_hash = Fnv::new().bytes(&data).key().0;
            let ids = db.load_font_source(fontdb::Source::Binary(data.clone()));
            if ids.is_empty() {
                return Err(Error::Font(format!("{name}: no faces")));
            }
            for id in ids {
                let face = db
                    .face(id)
                    .ok_or_else(|| Error::Font(format!("{name}: face vanished")))?;
                let family = face
                    .families
                    .first()
                    .map(|(f, _)| f.clone())
                    .unwrap_or_default();
                families.push(family.clone());
                faces.insert(
                    id,
                    FaceData {
                        data: data.clone(),
                        index: face.index,
                        family,
                        content_hash,
                    },
                );
            }
        }
        db.set_sans_serif_family(DEFAULT_FAMILY);
        let system = FontSystem::new_with_locale_and_db("en-US".into(), db);
        Ok(Self {
            system,
            faces,
            families,
        })
    }

    /// Bundled family names, in load order.
    #[must_use]
    pub fn families(&self) -> &[String] {
        &self.families
    }

    /// The family actually used for `requested` (bundled, else [`DEFAULT_FAMILY`]).
    #[must_use]
    pub fn resolve_family(&self, requested: &str) -> &str {
        self.families
            .iter()
            .find(|f| f.eq_ignore_ascii_case(requested))
            .map_or(DEFAULT_FAMILY, String::as_str)
    }

    /// Every face id, in load order.
    #[must_use]
    pub fn face_ids(&self) -> Vec<fontdb::ID> {
        let mut v: Vec<fontdb::ID> = self.faces.keys().copied().collect();
        v.sort();
        v
    }

    /// Face data for a font id produced by shaping.
    #[must_use]
    pub fn face(&self, id: fontdb::ID) -> Option<&FaceData> {
        self.faces.get(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_spec_parsing() {
        assert_eq!(
            parse_font_spec("Bricolage Grotesque 800"),
            ("Bricolage Grotesque".into(), 800)
        );
        assert_eq!(parse_font_spec("Inter"), ("Inter".into(), 400));
        assert_eq!(
            parse_font_spec("Noto Sans SC"),
            ("Noto Sans SC".into(), 400)
        );
    }

    #[test]
    fn bundled_fonts_load_and_woff2_decodes() {
        let lib = FontLibrary::bundled().expect("fonts");
        for f in [
            "Bricolage Grotesque",
            "Instrument Sans",
            "Noto Sans Arabic",
            "Noto Sans SC",
            "Noto Emoji",
        ] {
            assert!(
                lib.families().iter().any(|x| x == f),
                "{f} missing from {:?}",
                lib.families()
            );
        }
        assert_eq!(lib.resolve_family("Inter"), DEFAULT_FAMILY);
        assert_eq!(lib.resolve_family("bricolage grotesque"), CAPTION_FAMILY);
    }
}
