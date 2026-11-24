// SPDX-License-Identifier: GPL-3.0-or-later
//! Reads `media/manifest.tsv` written by `gen_media.py`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::Result;

/// One generated file.
#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    /// `video`, `proxy`, `audio` or `broken`.
    pub kind: String,
    /// Expected properties (`barcode=1`, `gop=48`, `vfr=1`, `rotation=90`, ...).
    pub expect: BTreeMap<String, String>,
}

impl Entry {
    /// An expectation value.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.expect.get(key).map(String::as_str)
    }

    /// Whether `key=1`.
    pub fn flag(&self, key: &str) -> bool {
        self.get(key) == Some("1")
    }
}

/// Loads the manifest in `dir`.
pub fn load(dir: &Path) -> Result<Vec<Entry>> {
    let text = std::fs::read_to_string(dir.join("manifest.tsv"))?;
    Ok(parse(dir, &text))
}

fn parse(dir: &Path, text: &str) -> Vec<Entry> {
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| {
            let cols: Vec<&str> = l.split('\t').collect();
            let (name, kind) = (cols.first()?, cols.get(1)?);
            let expect = cols
                .get(2)
                .map(|e| {
                    e.split(';')
                        .filter_map(|kv| kv.split_once('='))
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            Some(Entry {
                path: dir.join(name),
                name: (*name).to_string(),
                kind: (*kind).to_string(),
                expect,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rows_and_expectations() {
        let m = parse(
            Path::new("m"),
            "# header\na.mp4\tvideo\tbarcode=1;gop=48\t10\tff\nb.wav\taudio\t\t1\t00\n",
        );
        assert_eq!(m.len(), 2);
        assert!(m[0].flag("barcode"));
        assert_eq!(m[0].get("gop"), Some("48"));
        assert!(m[1].expect.is_empty());
        assert_eq!(m[1].path, Path::new("m").join("b.wav"));
    }
}
