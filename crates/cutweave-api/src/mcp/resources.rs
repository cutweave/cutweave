// SPDX-License-Identifier: GPL-3.0-or-later
//! MCP resources (WP-1J.02; arch §14): read-only views an agent can read and subscribe to.
//!
//! | URI | Content |
//! |---|---|
//! | `cutweave://project` | `project.info` plus the document `rev` (JSON) |
//! | `cutweave://sequence/{id}/timeline{?cursor}` | the sequence's tracks and items, paginated: every page is at most [`PAGE_BYTES`] of JSON |
//! | `cutweave://catalog/commands` | every command and query an agent can reach, with how to call it |
//! | `cutweave://frame/{rev}/{t}{?sequence,max_size,format,quality}` | a rendered frame (image blob); the link `render_frame` returns for large requests |
//!
//! The project and the timelines change with the document: subscribing to them gets a
//! notification per change ([`notify`](crate::mcp::notify)). A timeline page is a snapshot of
//! the revision it names (`rev`); later pages are read at their own revision, so a client that
//! pages through a changing document compares `rev`s.
//!
//! A frame link is valid for the revision it was made at: reading it re-renders at that time
//! and fails with a conflict once the document moved on (a frame is not stored).

use rmcp::model::{Resource, ResourceTemplate};
use serde_json::{Map, Value, json};

use crate::mcp::catalog::{self, Catalog};

/// `cutweave://project`.
pub const PROJECT: &str = "cutweave://project";
/// `cutweave://catalog/commands`.
pub const COMMANDS: &str = "cutweave://catalog/commands";
/// Most bytes of JSON in one timeline page.
pub const PAGE_BYTES: usize = 4096;
/// The JSON MIME type of the text resources.
pub const JSON: &str = "application/json";

const SCHEME: &str = "cutweave://";

/// What a URI names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `cutweave://project`.
    Project,
    /// `cutweave://catalog/commands`.
    Commands,
    /// `cutweave://sequence/{id}/timeline`.
    Timeline,
    /// `cutweave://frame/{rev}/{t}`.
    Frame,
}

/// A parsed resource URI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Uri {
    /// The project summary.
    Project,
    /// The command catalog.
    Commands,
    /// One page of a sequence's timeline, starting at row `cursor`.
    Timeline {
        /// The sequence id.
        sequence: String,
        /// The first row of the page.
        cursor: usize,
    },
    /// A rendered frame.
    Frame(FrameUri),
}

/// The parts of a `cutweave://frame/{rev}/{t}` link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameUri {
    /// The document revision the frame was made at.
    pub rev: u64,
    /// The time, in flicks.
    pub t: i64,
    /// The sequence.
    pub sequence: Option<String>,
    /// The longest side.
    pub max_size: Option<u32>,
    /// `jpeg` or `png`.
    pub format: Option<String>,
    /// JPEG quality.
    pub quality: Option<u8>,
}

impl Uri {
    /// What this names.
    #[must_use]
    pub fn kind(&self) -> Kind {
        match self {
            Uri::Project => Kind::Project,
            Uri::Commands => Kind::Commands,
            Uri::Timeline { .. } => Kind::Timeline,
            Uri::Frame(_) => Kind::Frame,
        }
    }
}

/// What `uri` names, when it is one of ours.
#[must_use]
pub fn kind(uri: &str) -> Option<Kind> {
    parse(uri).ok().map(|u| u.kind())
}

/// Parses a resource URI.
///
/// # Errors
/// A message saying what is wrong with `uri`.
pub fn parse(uri: &str) -> Result<Uri, String> {
    let rest = uri
        .strip_prefix(SCHEME)
        .ok_or_else(|| format!("`{uri}` is not a cutweave:// resource"))?;
    let (path, query) = rest.split_once('?').map_or((rest, ""), |(p, q)| (p, q));
    let segments: Vec<&str> = path.split('/').collect();
    let params = query_params(query)?;
    let no_params = |what: Uri| {
        if params.is_empty() {
            Ok(what)
        } else {
            Err(format!("`{uri}` takes no parameters"))
        }
    };
    match segments.as_slice() {
        ["project"] => no_params(Uri::Project),
        ["catalog", "commands"] => no_params(Uri::Commands),
        ["sequence", id, "timeline"] if is_id(id) => {
            let mut cursor = 0;
            for (key, value) in &params {
                match key.as_str() {
                    "cursor" => {
                        cursor = value
                            .parse()
                            .map_err(|_| format!("cursor `{value}` is not a row number"))?;
                    }
                    other => return Err(format!("unknown parameter `{other}`; expected cursor")),
                }
            }
            Ok(Uri::Timeline {
                sequence: (*id).to_owned(),
                cursor,
            })
        }
        ["frame", rev, t] => {
            let rev = rev
                .parse()
                .map_err(|_| format!("`{rev}` is not a revision"))?;
            let t = t
                .parse()
                .map_err(|_| format!("`{t}` is not a time in flicks"))?;
            let mut frame = FrameUri {
                rev,
                t,
                sequence: None,
                max_size: None,
                format: None,
                quality: None,
            };
            for (key, value) in params {
                match key.as_str() {
                    "sequence" if is_id(&value) => frame.sequence = Some(value),
                    "max_size" => {
                        frame.max_size = Some(
                            value
                                .parse()
                                .map_err(|_| format!("max_size `{value}` is not a size"))?,
                        );
                    }
                    "format" if matches!(value.as_str(), "jpeg" | "png") => {
                        frame.format = Some(value);
                    }
                    "quality" => {
                        frame.quality = Some(
                            value
                                .parse()
                                .map_err(|_| format!("quality `{value}` is not 1-100"))?,
                        );
                    }
                    other => {
                        return Err(format!(
                            "bad parameter `{other}={value}`; expected sequence, max_size, \
                             format (jpeg or png) or quality"
                        ));
                    }
                }
            }
            Ok(Uri::Frame(frame))
        }
        _ => Err(format!(
            "unknown resource `{uri}`; resources/list and resources/templates/list show them"
        )),
    }
}

fn is_id(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn query_params(query: &str) -> Result<Vec<(String, String)>, String> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            pair.split_once('=')
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .ok_or_else(|| format!("bad parameter `{pair}`; expected key=value"))
        })
        .collect()
}

/// `cutweave://sequence/{id}/timeline`, from row `cursor` on.
#[must_use]
pub fn timeline_uri(sequence: &str, cursor: usize) -> String {
    if cursor == 0 {
        format!("{SCHEME}sequence/{sequence}/timeline")
    } else {
        format!("{SCHEME}sequence/{sequence}/timeline?cursor={cursor}")
    }
}

/// The link of a frame (see the module docs).
#[must_use]
pub fn frame_uri(frame: &FrameUri) -> String {
    let mut uri = format!("{SCHEME}frame/{}/{}", frame.rev, frame.t);
    let mut sep = '?';
    let mut add = |key: &str, value: String| {
        uri.push(sep);
        uri.push_str(key);
        uri.push('=');
        uri.push_str(&value);
        sep = '&';
    };
    if let Some(s) = &frame.sequence {
        add("sequence", s.clone());
    }
    if let Some(m) = frame.max_size {
        add("max_size", m.to_string());
    }
    if let Some(f) = &frame.format {
        add("format", f.clone());
    }
    if let Some(q) = frame.quality {
        add("quality", q.to_string());
    }
    uri
}

/// The resources that always exist.
#[must_use]
pub fn fixed() -> Vec<Resource> {
    vec![
        Resource::new(PROJECT, "project")
            .with_title("Project summary")
            .with_description(
                "Project name, default format and a summary of each sequence, with the \
                 document revision. Subscribe for changes.",
            )
            .with_mime_type(JSON),
        Resource::new(COMMANDS, "commands")
            .with_title("Command catalog")
            .with_description(
                "Every command and query an agent can reach, with its summary and how to call it",
            )
            .with_mime_type(JSON),
    ]
}

/// The timeline resource of `sequence` (`name` is its display name).
#[must_use]
pub fn timeline_resource(sequence: &str, name: &str) -> Resource {
    Resource::new(timeline_uri(sequence, 0), format!("timeline {name}"))
        .with_title(format!("Timeline of {name}"))
        .with_description(
            "Tracks and items of the sequence, paginated (4 KB pages; follow `next`). \
             Subscribe for changes.",
        )
        .with_mime_type(JSON)
}

/// The URI templates.
#[must_use]
pub fn templates() -> Vec<ResourceTemplate> {
    vec![
        ResourceTemplate::new("cutweave://sequence/{id}/timeline{?cursor}", "timeline")
            .with_title("Sequence timeline")
            .with_description("Tracks and items of a sequence, paginated (4 KB pages)")
            .with_mime_type(JSON),
        ResourceTemplate::new(
            "cutweave://frame/{rev}/{t}{?sequence,max_size,format,quality}",
            "frame",
        )
        .with_title("Rendered frame")
        .with_description(
            "A frame at time t (flicks) of document revision rev, as render_frame links it \
             for max_size above 1568",
        ),
    ]
}

/// The command catalog resource: every reachable entry.
#[must_use]
pub fn commands(catalog: &Catalog) -> Value {
    let commands: Vec<Value> = catalog
        .reachable_entries()
        .map(|e| catalog::entry_summary(catalog, &e.meta))
        .collect();
    json!({ "count": commands.len(), "commands": commands })
}

/// The sequences of a `project.info` output as `(id, name)`.
#[must_use]
pub fn sequences(info: &Value) -> Vec<(String, String)> {
    info["sequences"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| Some((s["id"].as_str()?.to_owned(), s["name"].as_str()?.to_owned())))
        .collect()
}

/// One row of a timeline: a track, then its items.
enum Row {
    Track(Value),
    Item(Value),
}

/// One page of a `timeline.get{depth: 2}` output, at most [`PAGE_BYTES`] of JSON (a page always
/// holds at least one row, so paging advances).
///
/// The rows are the tracks (without their items) in display order, then every item with its
/// `track`; a page splits them into `tracks` and `items`. `page.next` is the URI of the next
/// page, `null` on the last.
///
/// # Errors
/// A message when `cursor` is past the last row.
pub fn timeline_page(
    timeline: &Value,
    uri_sequence: &str,
    rev: u64,
    cursor: usize,
) -> Result<Value, String> {
    let mut rows = Vec::new();
    let mut items = Vec::new();
    for track in timeline["tracks"].as_array().into_iter().flatten() {
        let mut head = track.clone();
        let track_id = track["id"].clone();
        if let Some(Value::Array(list)) = head.as_object_mut().and_then(|m| m.remove("items")) {
            for item in list {
                let mut item = item;
                if let Some(map) = item.as_object_mut() {
                    map.insert("track".into(), track_id.clone());
                }
                items.push(Row::Item(item));
            }
        }
        rows.push(Row::Track(head));
    }
    rows.append(&mut items);
    let total = rows.len();
    if cursor > total || (cursor == total && total > 0) {
        return Err(format!(
            "cursor {cursor} is past the end ({total} rows); start again without a cursor"
        ));
    }
    let mut head = Map::new();
    head.insert("uri".into(), json!(timeline_uri(uri_sequence, cursor)));
    head.insert("rev".into(), json!(rev));
    let mut sequence = json!({
        "id": timeline["id"],
        "name": timeline["name"],
        "duration": timeline["duration"],
        "track_count": timeline["track_count"],
        "item_count": timeline["item_count"],
    });
    if cursor == 0 {
        sequence["format"] = timeline["format"].clone();
    }
    head.insert("sequence".into(), sequence);

    let build = |taken: usize| {
        let mut tracks = Vec::new();
        let mut items = Vec::new();
        for row in &rows[cursor..cursor + taken] {
            match row {
                Row::Track(v) => tracks.push(v.clone()),
                Row::Item(v) => items.push(v.clone()),
            }
        }
        let end = cursor + taken;
        let next = (end < total).then(|| timeline_uri(uri_sequence, end));
        let mut page = head.clone();
        page.insert("tracks".into(), Value::Array(tracks));
        page.insert("items".into(), Value::Array(items));
        page.insert(
            "page".into(),
            json!({ "cursor": cursor, "rows": taken, "total_rows": total, "next": next }),
        );
        Value::Object(page)
    };
    let available = total - cursor;
    let mut taken = available.min(1);
    while taken < available && build(taken + 1).to_string().len() <= PAGE_BYTES {
        taken += 1;
    }
    Ok(build(taken))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_round_trip() {
        assert_eq!(parse(PROJECT), Ok(Uri::Project));
        assert_eq!(parse(COMMANDS), Ok(Uri::Commands));
        assert_eq!(
            parse("cutweave://sequence/seq_ab/timeline?cursor=7"),
            Ok(Uri::Timeline {
                sequence: "seq_ab".into(),
                cursor: 7
            })
        );
        assert_eq!(
            timeline_uri("seq_ab", 0),
            "cutweave://sequence/seq_ab/timeline"
        );
        let frame = FrameUri {
            rev: 4,
            t: 1_411_200_000,
            sequence: Some("seq_ab".into()),
            max_size: Some(2048),
            format: Some("png".into()),
            quality: None,
        };
        assert_eq!(parse(&frame_uri(&frame)), Ok(Uri::Frame(frame)));
        assert!(parse("cutweave://project?x=1").is_err());
        assert!(parse("cutweave://nothing").is_err());
        assert!(parse("https://project").is_err());
        assert!(parse("cutweave://frame/x/1").is_err());
        assert_eq!(kind("cutweave://frame/1/2"), Some(Kind::Frame));
    }

    fn timeline_with(items: usize) -> Value {
        let list: Vec<Value> = (0..items)
            .map(|i| {
                json!({
                    "id": format!("itm_{i:032x}"), "kind": "clip",
                    "start": i * 705_600_000, "duration": 705_600_000,
                    "media": format!("med_{i:032x}"), "linked_group": null, "enabled": true
                })
            })
            .collect();
        json!({
            "id": "seq_1", "name": "Main", "duration": 1, "track_count": 1,
            "item_count": items, "format": { "width": 1920 },
            "tracks": [{ "id": "trk_1", "kind": "video", "name": "V1", "item_count": items,
                         "items": list }]
        })
    }

    #[test]
    fn pages_hold_at_most_4_kb_and_cover_every_row() {
        let timeline = timeline_with(120);
        let mut cursor = 0;
        let mut seen = 0;
        let mut pages = 0;
        loop {
            let page = timeline_page(&timeline, "seq_1", 9, cursor).unwrap();
            let text = page.to_string();
            assert!(text.len() <= PAGE_BYTES, "{} bytes", text.len());
            seen += page["items"].as_array().unwrap().len();
            pages += 1;
            assert_eq!(page["rev"], 9);
            match page["page"]["next"].as_str() {
                Some(next) => {
                    let Ok(Uri::Timeline { cursor: c, .. }) = parse(next) else {
                        panic!("{next}")
                    };
                    assert!(c > cursor);
                    cursor = c;
                }
                None => break,
            }
        }
        assert_eq!(seen, 120);
        assert!(pages > 1);
        assert!(timeline_page(&timeline, "seq_1", 9, 500).is_err());
    }

    #[test]
    fn a_small_timeline_is_one_page() {
        let page = timeline_page(&timeline_with(2), "seq_1", 1, 0).unwrap();
        assert_eq!(page["page"]["next"], Value::Null);
        assert_eq!(page["tracks"].as_array().unwrap().len(), 1);
        assert_eq!(page["items"][1]["track"], "trk_1");
        assert!(page["tracks"][0].get("items").is_none());
    }
}
