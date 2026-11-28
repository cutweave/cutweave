// SPDX-License-Identifier: GPL-3.0-or-later
//! Pictures over MCP (WP-2I.02; arch §14 "seeing without screenshots"): `render.contact_sheet`,
//! `media.filmstrip` and `audio.waveform`, reached through `cutweave_query` (they are catalog
//! commands: listed as tools they do not fit the standard list's budget, ADR-0039).
//!
//! The engine returns each picture as base64 `data` next to its metadata, like `render.frame`.
//! Over MCP the picture is an **image content block** and the rest is `structuredContent`
//! (times, sizes, the items of each cell, the levels and silences), so a model sees the picture
//! and reads the facts without the base64 in its text. [`frame`](crate::mcp::frame) does the
//! same for `render_frame`.
//!
//! - **Size cap.** A result never exceeds [`frame::MAX_PAYLOAD`] bytes of JSON (base64
//!   included). The pictures are capped by their commands (a 2048 px sheet or strip, a
//!   2048 x 1024 px waveform), so this is rare: a result over the cap is refused with a hint to
//!   ask for a smaller picture.
//! - **Large pictures.** A request for more than [`frame::LINK_ABOVE`] px (models scale larger
//!   images down, so the pixels are wasted context) whose result names a `/media/...` route
//!   (`link`: the whole clip's filmstrip, a media file's waveform) is answered with a
//!   **resource link** to that route on this server and the metadata, without the image. The
//!   route answers a `GET` with the same bearer token (or a URL signed by `POST /media/sign`)
//!   and carries an ETag made of the content hash and a hash of the parameters. A contact
//!   sheet, a part of a clip and the waveform of a clip or a mix have no route: they are
//!   returned inline. Over stdio there is no HTTP origin to link to, so everything is inline.
//! - **Untrusted text.** The names of the items in a contact sheet's cells are text a person
//!   typed or a file name: they are labelled `untrusted` ([`results::UNTRUSTED_FIELDS`]).

use cutweave_commands::Rev;
use rmcp::model::{CallToolResult, ContentBlock, Resource};
use serde_json::Value;

use crate::mcp::{frame, results};

/// The commands whose picture this module returns, with the parameter that sets the picture's
/// size (what a hint tells the model to lower).
pub const COMMANDS: [(&str, &str); 3] = [
    ("render.contact_sheet", "max_size"),
    ("media.filmstrip", "max_size"),
    ("audio.waveform", "width"),
];

/// The size parameter of `command`, when it is one of [`COMMANDS`].
#[must_use]
pub fn size_param(command: &str) -> Option<&'static str> {
    COMMANDS
        .iter()
        .find(|(name, _)| *name == command)
        .map(|(_, param)| *param)
}

/// Whether `args` ask `command` for a picture big enough to be returned as a link.
#[must_use]
pub fn wants_link(command: &str, args: &Value) -> bool {
    size_param(command)
        .and_then(|param| args.get(param))
        .and_then(Value::as_u64)
        .is_some_and(|size| size > u64::from(frame::LINK_ABOVE))
}

fn take_image(output: &mut Value) -> Option<(String, String)> {
    let map = output.as_object_mut()?;
    let Value::String(data) = map.remove("data")? else {
        return None;
    };
    let mime = map.get("mime")?.as_str()?.to_owned();
    Some((mime, data))
}

fn count(output: &Value, pointer: &str) -> usize {
    output
        .pointer(pointer)
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

/// The first text line of a result: what the picture is, in words.
fn summary(tool: &str, command: &str, output: &Value, rev: Rev) -> String {
    let what = match command {
        "render.contact_sheet" => format!("{} frame(s)", count(output, "/frames")),
        "media.filmstrip" => format!("{} tile(s)", count(output, "/tiles")),
        _ => {
            let peak = output
                .pointer("/audio_levels/peak_db")
                .and_then(Value::as_f64)
                .map_or_else(|| "silent".to_owned(), |db| format!("peak {db:.1} dBFS"));
            format!(
                "{peak}, {} silence(s)",
                count(output, "/audio_levels/silences")
            )
        }
    };
    format!(
        "{tool}: {command} {}x{} {}, {what} (document at rev {rev})",
        output["width"],
        output["height"],
        output["mime"].as_str().unwrap_or("image"),
    )
}

/// The MCP result of `command`'s `output` (untrusted text already wrapped): the picture as an
/// image block, or, for a large request whose output names a `/media/...` route and with the
/// server's HTTP `origin` known, as a resource link to it.
///
/// # Errors
/// The payload size in bytes, when the result is over [`frame::MAX_PAYLOAD`].
pub fn result(
    tool: &str,
    command: &str,
    mut output: Value,
    args: &Value,
    revs: (Rev, Rev),
    origin: Option<&str>,
) -> Result<CallToolResult, usize> {
    let image = take_image(&mut output);
    let mut summary = summary(tool, command, &output, revs.0);
    let route = output.get("link").and_then(Value::as_str);
    if let (true, Some(route), Some(origin)) = (wants_link(command, args), route, origin) {
        let uri = format!("{origin}{route}");
        summary.push_str(
            "; the image is large: GET the link with your bearer token (or sign it with POST \
             /media/sign) to get it",
        );
        let mut resource = Resource::new(uri, command.replace('.', "_"));
        if let Some((mime, _)) = &image {
            resource = resource.with_mime_type(mime.clone());
        }
        let mut result = results::success(summary, output, Some(revs));
        result.content.push(ContentBlock::resource_link(resource));
        return Ok(result);
    }
    let mut result = results::success(summary, output, Some(revs));
    if let Some((mime, data)) = image {
        result.content.push(ContentBlock::image(data, mime));
    }
    let size = serde_json::to_vec(&result).map_or(usize::MAX, |v| v.len());
    if size > frame::MAX_PAYLOAD {
        Err(size)
    } else {
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn strip(data: &str, link: bool) -> Value {
        let mut out = json!({
            "mime": "image/jpeg", "data": data, "bytes": 3, "width": 2048, "height": 144,
            "media": "med_1", "from": 0, "to": 100, "tile_width": 256,
            "tiles": [{ "t": 0, "x": 0 }, { "t": 50, "x": 256 }], "etag": "abc-def"
        });
        if link {
            out["link"] = json!("/media/med_1/filmstrip?w=256&count=2&hash=abc");
        }
        out
    }

    fn images(result: &CallToolResult) -> usize {
        result
            .content
            .iter()
            .filter(|c| matches!(c, ContentBlock::Image(_)))
            .count()
    }

    fn links(result: &CallToolResult) -> Vec<String> {
        result
            .content
            .iter()
            .filter_map(|c| match c {
                ContentBlock::ResourceLink(link) => Some(link.uri.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_picture_is_an_image_block_and_the_metadata_has_no_base64() {
        let revs = (Rev(3), Rev(3));
        let result = result(
            "cutweave_query",
            "media.filmstrip",
            strip("QUJD", true),
            &json!({ "media": "med_1" }),
            revs,
            Some("http://127.0.0.1:4000"),
        )
        .unwrap();
        assert_eq!(images(&result), 1);
        assert!(
            links(&result).is_empty(),
            "a default-size request is inline"
        );
        let s = result.structured_content.unwrap();
        assert!(s.get("data").is_none());
        assert_eq!(s["tiles"][1]["x"], 256);
        assert_eq!(s["link"], "/media/med_1/filmstrip?w=256&count=2&hash=abc");
        let ContentBlock::Text(first) = &result.content[0] else {
            panic!("a text summary first");
        };
        assert_eq!(
            first.text,
            "cutweave_query: media.filmstrip 2048x144 image/jpeg, 2 tile(s) (document at rev 3)"
        );
    }

    #[test]
    fn a_large_request_with_a_route_is_a_link_on_http_only() {
        let revs = (Rev(3), Rev(3));
        let big = json!({ "media": "med_1", "max_size": 2048 });
        let linked = result(
            "cutweave_query",
            "media.filmstrip",
            strip("QUJD", true),
            &big,
            revs,
            Some("http://127.0.0.1:4000"),
        )
        .unwrap();
        assert_eq!(images(&linked), 0);
        assert_eq!(
            links(&linked),
            ["http://127.0.0.1:4000/media/med_1/filmstrip?w=256&count=2&hash=abc"]
        );
        assert!(linked.structured_content.unwrap().get("data").is_none());
        // No route (a part of a clip), or no HTTP origin (stdio): inline.
        for (output, origin) in [
            (strip("QUJD", false), Some("http://127.0.0.1:4000")),
            (strip("QUJD", true), None),
        ] {
            let inline = result(
                "cutweave_query",
                "media.filmstrip",
                output,
                &big,
                revs,
                origin,
            )
            .unwrap();
            assert_eq!(images(&inline), 1);
            assert_eq!(links(&inline), [] as [String; 0]);
        }
        assert!(wants_link("audio.waveform", &json!({ "width": 1569 })));
        assert!(!wants_link(
            "audio.waveform",
            &json!({ "width": 1568, "max_size": 2048 })
        ));
        assert!(wants_link(
            "render.contact_sheet",
            &json!({ "max_size": 2048 })
        ));
        assert!(!wants_link("render.frame", &json!({ "max_size": 2048 })));
        assert_eq!(size_param("timeline.get"), None);
    }

    #[test]
    fn a_result_over_the_cap_reports_its_size_and_summaries_name_the_picture() {
        let revs = (Rev(1), Rev(1));
        let big = "A".repeat(frame::MAX_PAYLOAD);
        let size = result(
            "cutweave_query",
            "media.filmstrip",
            strip(&big, false),
            &json!({}),
            revs,
            None,
        )
        .unwrap_err();
        assert!(size > frame::MAX_PAYLOAD);
        let sheet = json!({
            "mime": "image/jpeg", "data": "QUJD", "width": 768, "height": 397,
            "frames": [{}, {}, {}]
        });
        let wave = json!({
            "mime": "image/png", "data": "QUJD", "width": 768, "height": 192,
            "audio_levels": { "peak_db": -6.0206, "silences": [{ "from": 0, "to": 9 }] }
        });
        let quiet = json!({
            "mime": "image/png", "data": "QUJD", "width": 768, "height": 192,
            "audio_levels": { "peak_db": null, "silences": [] }
        });
        for (command, output, text) in [
            (
                "render.contact_sheet",
                sheet,
                "t: render.contact_sheet 768x397 image/jpeg, 3 frame(s) (document at rev 1)",
            ),
            (
                "audio.waveform",
                wave,
                "t: audio.waveform 768x192 image/png, peak -6.0 dBFS, 1 silence(s) (document at rev 1)",
            ),
            (
                "audio.waveform",
                quiet,
                "t: audio.waveform 768x192 image/png, silent, 0 silence(s) (document at rev 1)",
            ),
        ] {
            let result = result("t", command, output, &json!({}), revs, None).unwrap();
            let ContentBlock::Text(first) = &result.content[0] else {
                panic!("a text summary first");
            };
            assert_eq!(first.text, text);
        }
    }
}
