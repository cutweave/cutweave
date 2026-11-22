// SPDX-License-Identifier: GPL-3.0-or-later
//! Test-media corpus: manifest parsing and validation, resumable fetch, checksum verification (WP-0I.01).
//!
//! The manifest is `testdata/corpus.toml`; media lands in the git-ignored `testdata/corpus/`.
//! Downloads shell out to `curl` (present on Windows 10+, macOS and Linux runners; `-C -` resumes and
//! `file://` works for offline fixtures), so no HTTP stack is added to the workspace.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Hash placeholder used until the first fetch pins the real digest.
pub const PENDING: &str = "PENDING";

/// Licenses a corpus item may carry (docs/01-architecture.md §21: redistributable for testing).
const LICENSES: &[&str] = &[
    "CC0-1.0",
    "CC-BY-3.0",
    "CC-BY-4.0",
    "public-domain",
    "self-generated",
];

/// Download tier. `small` is fetched on PR CI, `full` (everything) nightly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Small,
    Full,
}

impl Tier {
    fn includes(self, item: Tier) -> bool {
        self == Tier::Full || item == Tier::Small
    }
}

/// One corpus entry.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // descriptive fields are part of the manifest contract and read by tests/docs
pub struct Item {
    pub id: String,
    pub category: String,
    pub description: String,
    pub purpose: String,
    pub codec: String,
    pub container: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub file: String,
    pub tier: Tier,
    #[serde(default)]
    pub license: String,
    pub attribution: Option<String>,
    #[serde(default)]
    pub license_review: bool,
    pub source_url: Option<String>,
    pub generate: Option<String>,
    #[serde(default)]
    pub url_verified: bool,
    pub sha256: String,
    pub size: u64,
}

impl Item {
    /// True while no digest has been recorded.
    pub fn pending(&self) -> bool {
        self.sha256 == PENDING
    }
}

/// The parsed manifest.
#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub item: Vec<Item>,
}

/// Parse and validate manifest text. Rejects items without a license (WP-0I.01 acceptance).
pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let m: Manifest = toml::from_str(text).map_err(|e| format!("manifest parse error: {e}"))?;
    let mut ids = BTreeSet::new();
    let mut files = BTreeSet::new();
    for it in &m.item {
        let id = &it.id;
        if !ids.insert(id.clone()) {
            return Err(format!("duplicate id `{id}`"));
        }
        if !files.insert(it.file.clone()) {
            return Err(format!("`{id}`: duplicate file name `{}`", it.file));
        }
        if it.file.contains(['/', '\\']) || it.file.starts_with('.') {
            return Err(format!("`{id}`: file must be a plain file name"));
        }
        if it.license.trim().is_empty() {
            return Err(format!("`{id}`: missing license"));
        }
        if !LICENSES.contains(&it.license.as_str()) {
            return Err(format!(
                "`{id}`: license `{}` is not one of {LICENSES:?}",
                it.license
            ));
        }
        if it.license.starts_with("CC-BY")
            && it
                .attribution
                .as_deref()
                .is_none_or(|a| a.trim().is_empty())
        {
            return Err(format!("`{id}`: CC-BY items need an attribution"));
        }
        match (&it.source_url, &it.generate) {
            (Some(_), None) | (None, Some(_)) => {}
            _ => {
                return Err(format!(
                    "`{id}`: exactly one of source_url / generate is required"
                ));
            }
        }
        if it.license == "self-generated" && it.generate.is_none() {
            return Err(format!(
                "`{id}`: self-generated items need a generate recipe"
            ));
        }
        let h = &it.sha256;
        if h != PENDING && !(h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())) {
            return Err(format!("`{id}`: sha256 must be {PENDING} or 64 hex digits"));
        }
    }
    Ok(m)
}

/// Lower-case hex sha256 of a file, streamed.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    let mut s = String::with_capacity(64);
    for b in h.finalize() {
        let _ = write!(s, "{b:02x}");
    }
    Ok(s)
}

/// Where corpus files live: `<root>/testdata/corpus`.
pub fn corpus_dir(root: &Path) -> PathBuf {
    root.join("testdata").join("corpus")
}

/// Path of a corpus item by id (the `corpus::path("h264_4k30_longgop")` helper from WP-0I.01).
#[allow(dead_code)] // consumed by tests once decode work packages land; moves to a dev-crate in 1A
pub fn path(root: &Path, manifest: &Manifest, id: &str) -> Option<PathBuf> {
    manifest
        .item
        .iter()
        .find(|i| i.id == id)
        .map(|i| corpus_dir(root).join(&i.file))
}

/// Rewrite the manifest text so item `id` carries the given digest and size, leaving comments intact.
pub fn pin_in_text(text: &str, id: &str, sha: &str, size: u64) -> String {
    let marker = format!("id = \"{id}\"");
    let mut out = String::with_capacity(text.len());
    let mut in_item = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("[[item]]") {
            in_item = false;
        } else if t.replace(' ', "") == marker.replace(' ', "") {
            in_item = true;
        }
        if in_item && t.starts_with("sha256") {
            let _ = writeln!(out, "sha256 = \"{sha}\"");
        } else if in_item && t.starts_with("size") {
            let _ = writeln!(out, "size = {size}");
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// What happened to one item during a fetch.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Already present and (if pinned) matching.
    Cached,
    /// Downloaded and verified; carries the digest and size.
    Fetched(String, u64),
    /// Not fetchable here (generator without ffmpeg).
    Skipped(String),
}

fn curl_download(url: &str, part: &Path) -> Result<(), String> {
    let run = |resume: bool| {
        let mut c = Command::new("curl");
        c.args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--retry",
            "3",
        ]);
        if resume {
            c.args(["-C", "-"]);
        }
        c.arg("-o").arg(part).arg(url);
        c.status()
    };
    let resumed = part.exists();
    let ok = run(resumed)
        .map_err(|e| format!("cannot run curl: {e}"))?
        .success();
    if ok {
        return Ok(());
    }
    // A stale or already-complete partial file can make a ranged request fail: retry from scratch once.
    if resumed {
        let _ = fs::remove_file(part);
        if run(false)
            .map_err(|e| format!("cannot run curl: {e}"))?
            .success()
        {
            return Ok(());
        }
    }
    Err(format!("download failed: {url}"))
}

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Fetch (or generate) one item into `dir`. A digest mismatch deletes the download and errors.
pub fn fetch_item(root: &Path, dir: &Path, it: &Item) -> Result<Outcome, String> {
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let dest = dir.join(&it.file);
    if dest.is_file() {
        if it.pending() || sha256_file(&dest).map_err(|e| e.to_string())? == it.sha256 {
            return Ok(Outcome::Cached);
        }
        eprintln!(
            "{}: cached file does not match the manifest, refetching",
            it.id
        );
        fs::remove_file(&dest).map_err(|e| e.to_string())?;
    }
    let part = dir.join(format!("{}.part", it.file));
    if let Some(url) = &it.source_url {
        curl_download(url, &part)?;
    } else if let Some(recipe) = &it.generate {
        if !have_ffmpeg() {
            return Ok(Outcome::Skipped("needs ffmpeg on PATH".into()));
        }
        let ok = Command::new("bash")
            .arg(root.join(recipe))
            .arg(&part)
            .current_dir(root)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            let _ = fs::remove_file(&part);
            return Err(format!("{}: generator `{recipe}` failed", it.id));
        }
    }
    let sha = sha256_file(&part).map_err(|e| format!("{}: {e}", it.id))?;
    if !it.pending() && sha != it.sha256 {
        let _ = fs::remove_file(&part);
        return Err(format!(
            "{}: sha256 mismatch (expected {}, got {sha})",
            it.id, it.sha256
        ));
    }
    let size = fs::metadata(&part).map_err(|e| e.to_string())?.len();
    fs::rename(&part, &dest).map_err(|e| format!("{}: {e}", it.id))?;
    Ok(Outcome::Fetched(sha, size))
}

struct Opts {
    tier: Tier,
    manifest: PathBuf,
    dir: PathBuf,
    pin: bool,
    allow_unpinned: bool,
}

fn parse_opts(root: &Path, args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        tier: Tier::Small,
        manifest: root.join("testdata").join("corpus.toml"),
        dir: corpus_dir(root),
        pin: false,
        allow_unpinned: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |name: &str| it.next().cloned().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--tier" => {
                o.tier = match val("--tier")?.as_str() {
                    "small" => Tier::Small,
                    "full" => Tier::Full,
                    other => return Err(format!("unknown tier `{other}` (small|full)")),
                }
            }
            "--manifest" => o.manifest = PathBuf::from(val("--manifest")?),
            "--dir" => o.dir = PathBuf::from(val("--dir")?),
            "--pin" => o.pin = true,
            "--allow-unpinned" => o.allow_unpinned = true,
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    Ok(o)
}

fn load(path: &Path) -> Result<(String, Manifest), String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let m = parse_manifest(&text)?;
    Ok((text, m))
}

/// `cargo xtask fetch-corpus [--tier small|full] [--pin] [--manifest P] [--dir D]`.
pub fn fetch(root: &Path, args: &[String]) -> bool {
    let run = || -> Result<bool, String> {
        let o = parse_opts(root, args)?;
        let (mut text, m) = load(&o.manifest)?;
        let (mut failed, mut unpinned) = (false, 0);
        for it in m.item.iter().filter(|i| o.tier.includes(i.tier)) {
            match fetch_item(root, &o.dir, it) {
                Ok(Outcome::Cached) => println!("{}: cached", it.id),
                Ok(Outcome::Skipped(why)) => println!("{}: skipped ({why})", it.id),
                Ok(Outcome::Fetched(sha, size)) => {
                    println!("{}: fetched {size} bytes, sha256 {sha}", it.id);
                    if it.pending() {
                        if o.pin {
                            text = pin_in_text(&text, &it.id, &sha, size);
                        } else {
                            unpinned += 1;
                        }
                    }
                }
                Err(e) => {
                    eprintln!("{e}");
                    failed = true;
                }
            }
        }
        if o.pin {
            fs::write(&o.manifest, text).map_err(|e| e.to_string())?;
        } else if unpinned > 0 {
            println!(
                "{unpinned} item(s) have no pinned sha256; re-run with --pin to record them in the manifest"
            );
        }
        Ok(!failed)
    };
    run().unwrap_or_else(|e| {
        eprintln!("fetch-corpus: {e}");
        false
    })
}

/// `cargo xtask corpus-verify [--tier ..] [--allow-unpinned]`: every fetchable item must be present and match.
pub fn verify(root: &Path, args: &[String]) -> bool {
    let run = || -> Result<bool, String> {
        let o = parse_opts(root, args)?;
        let (_, m) = load(&o.manifest)?;
        let mut ok = true;
        for it in m.item.iter().filter(|i| o.tier.includes(i.tier)) {
            let p = o.dir.join(&it.file);
            if !p.is_file() {
                if it.generate.is_some() {
                    println!("{}: absent (generated; needs ffmpeg)", it.id);
                } else {
                    eprintln!("{}: MISSING", it.id);
                    ok = false;
                }
                continue;
            }
            if it.pending() {
                if o.allow_unpinned {
                    println!("{}: present, unpinned", it.id);
                } else {
                    eprintln!(
                        "{}: sha256 not pinned in the manifest (fetch-corpus --pin)",
                        it.id
                    );
                    ok = false;
                }
                continue;
            }
            let got = sha256_file(&p).map_err(|e| e.to_string())?;
            if got == it.sha256 {
                println!("{}: ok", it.id);
            } else {
                eprintln!("{}: CORRUPT (expected {}, got {got})", it.id, it.sha256);
                ok = false;
            }
        }
        Ok(ok)
    };
    run().unwrap_or_else(|e| {
        eprintln!("corpus-verify: {e}");
        false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    fn item_toml(extra: &str) -> String {
        format!(
            "[[item]]\nid=\"a\"\ncategory=\"c\"\ndescription=\"d\"\npurpose=\"p\"\ncodec=\"x\"\ncontainer=\"y\"\n\
             file=\"a.bin\"\ntier=\"small\"\nsha256=\"PENDING\"\nsize=0\n{extra}\n"
        )
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cutweave-xtask-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn real_manifest_is_valid() {
        let root = crate::workspace_root();
        let (_, m) = load(&root.join("testdata/corpus.toml")).unwrap();
        assert!(m.item.len() >= 20);
        for it in &m.item {
            if let Some(g) = &it.generate {
                assert!(root.join(g).is_file(), "{}: missing recipe {g}", it.id);
            }
        }
        // Every item the work package requires is present.
        for id in [
            "h264_4k30_longgop",
            "h264_1080p60",
            "hevc_10bit_sdr",
            "hevc_10bit_hdr10_pq",
            "prores_422hq_1080p",
            "vfr_phone_rotated90",
            "audio_22050",
            "audio_44100",
            "audio_48000",
            "audio_96000",
            "audio_5_1",
            "image_png",
            "image_jpeg",
            "broken_truncated_mp4",
            "no_audio_stream",
            "variable_frame_size",
        ] {
            assert!(
                path(&root, &m, id).is_some(),
                "required item `{id}` missing"
            );
        }
    }

    #[test]
    fn rejects_missing_license() {
        let e = parse_manifest(&item_toml("generate=\"g.sh\"")).unwrap_err();
        assert!(e.contains("missing license"), "{e}");
    }

    #[test]
    fn rejects_bad_entries() {
        assert!(parse_manifest(&item_toml("license=\"proprietary\"\ngenerate=\"g.sh\"")).is_err());
        assert!(
            parse_manifest(&item_toml(
                "license=\"CC-BY-3.0\"\nsource_url=\"file:///x\""
            ))
            .is_err()
        ); // no attribution
        assert!(parse_manifest(&item_toml("license=\"CC0-1.0\"")).is_err()); // no source
        assert!(
            parse_manifest(&item_toml(
                "license=\"CC0-1.0\"\nsource_url=\"u\"\ngenerate=\"g\""
            ))
            .is_err()
        );
        let two = format!(
            "{0}{0}",
            item_toml("license=\"self-generated\"\ngenerate=\"g.sh\"")
        );
        assert!(parse_manifest(&two).unwrap_err().contains("duplicate"));
        let ok = item_toml("license=\"self-generated\"\ngenerate=\"g.sh\"");
        assert!(parse_manifest(&ok).is_ok());
    }

    #[test]
    fn sha256_known_vector() {
        let d = temp("sha");
        fs::write(d.join("f"), b"abc").unwrap();
        assert_eq!(sha256_file(&d.join("f")).unwrap(), SHA_ABC);
    }

    #[test]
    fn pin_rewrites_only_the_target_item() {
        let t = "# c\n[[item]]\nid = \"a\"\nsha256 = \"PENDING\"\nsize = 0\n[[item]]\nid = \"b\"\nsha256 = \"PENDING\"\nsize = 0\n";
        let out = pin_in_text(t, "b", SHA_ABC, 3);
        assert!(out.contains(&format!("id = \"b\"\nsha256 = \"{SHA_ABC}\"\nsize = 3\n")));
        assert!(out.contains("id = \"a\"\nsha256 = \"PENDING\"\nsize = 0\n"));
    }

    fn fixture(d: &Path, sha: &str) -> (PathBuf, String) {
        let src = d.join("src.bin");
        fs::write(&src, b"abc").unwrap();
        let url = format!(
            "file:///{}",
            src.display()
                .to_string()
                .replace('\\', "/")
                .trim_start_matches('/')
        );
        let manifest = item_toml(&format!("license=\"CC0-1.0\"\nsource_url=\"{url}\""))
            .replace("PENDING", sha);
        let mp = d.join("corpus.toml");
        fs::write(&mp, &manifest).unwrap();
        (mp, url)
    }

    fn args(d: &Path, mp: &Path, extra: &[&str]) -> Vec<String> {
        let mut v = vec![
            "--manifest".into(),
            mp.display().to_string(),
            "--dir".into(),
            d.join("out").display().to_string(),
        ];
        v.extend(extra.iter().map(|s| (*s).to_owned()));
        v
    }

    #[test]
    fn fetch_pin_verify_and_corruption() {
        let d = temp("fetch");
        let (mp, _) = fixture(&d, PENDING);
        let root = crate::workspace_root();
        assert!(fetch(&root, &args(&d, &mp, &["--pin"])));
        assert!(fs::read_to_string(&mp).unwrap().contains(SHA_ABC));
        assert!(verify(&root, &args(&d, &mp, &[])));
        // Second fetch is a no-op cache hit; corrupting the file makes verify fail.
        assert!(fetch(&root, &args(&d, &mp, &[])));
        fs::write(d.join("out").join("a.bin"), b"abd").unwrap();
        assert!(!verify(&root, &args(&d, &mp, &[])));
        // fetch repairs it from the source.
        assert!(fetch(&root, &args(&d, &mp, &[])));
        assert!(verify(&root, &args(&d, &mp, &[])));
    }

    #[test]
    fn checksum_mismatch_is_rejected_and_leaves_no_file() {
        let d = temp("mismatch");
        let (mp, _) = fixture(&d, &"0".repeat(64));
        let root = crate::workspace_root();
        assert!(!fetch(&root, &args(&d, &mp, &[])));
        assert!(!d.join("out").join("a.bin").exists());
        assert!(!d.join("out").join("a.bin.part").exists());
    }

    #[test]
    fn resumes_a_partial_download() {
        let d = temp("resume");
        let (mp, _) = fixture(&d, SHA_ABC);
        fs::create_dir_all(d.join("out")).unwrap();
        fs::write(d.join("out").join("a.bin.part"), b"a").unwrap();
        let root = crate::workspace_root();
        assert!(fetch(&root, &args(&d, &mp, &[])));
        assert_eq!(fs::read(d.join("out").join("a.bin")).unwrap(), b"abc");
    }
}
