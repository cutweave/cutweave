// SPDX-License-Identifier: GPL-3.0-or-later
//! `render_frame` over MCP (WP-1J.02; arch §14 "seeing without screenshots").
//!
//! The engine's `render.frame` returns the image as base64 `data` next to its metadata. Over MCP
//! the image is an **image content block** and the rest is `structuredContent` (`time`, `rev`,
//! `width`, `height`, `items_visible`, `items_hidden`, `notes`, ...), so a model sees the picture
//! and reads the facts without the base64 in its text.
//!
//! - **Size cap.** A result never exceeds [`MAX_PAYLOAD`] bytes (1 MB of JSON, base64 included).
//!   A JPEG over the cap is rendered again at a lower quality (60, 40, 25; the result's `notes`
//!   say so); a PNG, or a frame that is still too big, is refused with a hint to lower
//!   `max_size`.
//! - **Large frames.** A request for `max_size` above [`LINK_ABOVE`] (1568 px: larger images are
//!   scaled down by the models that read them, so the extra pixels are wasted context) answers
//!   with a **resource link** `cutweave://frame/{rev}/{t}?...` and the metadata, without the
//!   image. Reading the link renders it again ([`resources`](crate::mcp::resources)).
//! - **Transactions.** `render.frame` renders the *committed* document on the engine's renderer;
//!   inside a `cutweave_execute` transaction it cannot, and `cutweave_execute` refuses it before
//!   anything runs (see [`TRANSACTION_HINT`]): render after the batch commits.

use cutweave_commands::Rev;
use rmcp::model::{CallToolResult, ContentBlock, Resource};
use serde_json::Value;

use crate::mcp::{
    resources::{self, FrameUri},
    results,
};

/// The tool name.
pub const TOOL: &str = "render_frame";
/// The registry name of the query behind it.
pub const COMMAND: &str = "render.frame";
/// The most bytes of JSON a `render_frame` result may have.
pub const MAX_PAYLOAD: usize = 1_000_000;
/// A request for a longer side than this gets a resource link instead of an image.
pub const LINK_ABOVE: u32 = 1568;
/// The JPEG qualities tried, in order, when a result is over [`MAX_PAYLOAD`].
pub const LOWER_QUALITIES: [u8; 3] = [60, 40, 25];
/// What to do about `render.frame` in a `cutweave_execute` batch.
pub const TRANSACTION_HINT: &str = "render_frame shows the committed document, so it cannot run \
inside a transaction: run the batch, then call render_frame (cutweave_execute dry_run rolls \
back, so nothing could be seen there either)";

/// Whether `args` ask for a frame big enough to be returned as a resource link.
#[must_use]
pub fn wants_link(args: &Value) -> bool {
    args.get("max_size")
        .and_then(Value::as_u64)
        .is_some_and(|m| m > u64::from(LINK_ABOVE))
}

/// The JPEG quality to try after `current` (the default 80 when none was given), if any.
#[must_use]
pub fn next_quality(current: Option<u64>) -> Option<u8> {
    let current = current.unwrap_or(80);
    LOWER_QUALITIES
        .into_iter()
        .find(|q| u64::from(*q) < current)
}

/// Whether the frame in `output` is a JPEG.
#[must_use]
pub fn is_jpeg(output: &Value) -> bool {
    output["mime"] == "image/jpeg"
}

fn take_image(output: &mut Value) -> Option<(String, String)> {
    let map = output.as_object_mut()?;
    let Value::String(data) = map.remove(results::IMAGE_FIELDS[0].1.trim_start_matches('/'))?
    else {
        return None;
    };
    let mime = map.get("mime")?.as_str()?.to_owned();
    Some((mime, data))
}

fn summary(output: &Value, rev: Rev) -> String {
    let at = output["timecode"]
        .as_str()
        .map_or_else(|| "this time".to_owned(), str::to_owned);
    let visible = output["items_visible"].as_array().map_or(0, Vec::len);
    format!(
        "render_frame: {}x{} {} at {at}, {visible} item(s) visible (document at rev {rev})",
        output["width"],
        output["height"],
        output["mime"].as_str().unwrap_or("image"),
    )
}

/// Appends `note` to the result's `notes`.
pub fn add_note(output: &mut Value, note: String) {
    if let Some(notes) = output.get_mut("notes").and_then(Value::as_array_mut) {
        notes.push(Value::String(note));
    }
}

/// The result with the image as a content block, or the size of the payload when it is over
/// [`MAX_PAYLOAD`].
///
/// # Errors
/// The payload size in bytes, when too big.
pub fn inline(mut output: Value, revs: (Rev, Rev)) -> Result<CallToolResult, usize> {
    let Some((mime, data)) = take_image(&mut output) else {
        return Ok(results::success(
            summary(&output, revs.0),
            output,
            Some(revs),
        ));
    };
    let mut result = results::success(summary(&output, revs.0), output, Some(revs));
    result.content.push(ContentBlock::image(data, mime));
    let size = serde_json::to_vec(&result).map_or(usize::MAX, |v| v.len());
    if size > MAX_PAYLOAD {
        Err(size)
    } else {
        Ok(result)
    }
}

/// What a large request's link names: the frame as rendered.
#[must_use]
pub fn frame_uri(output: &Value, args: &Value, rev: Rev) -> String {
    resources::frame_uri(&FrameUri {
        rev: rev.0,
        t: output["time"].as_i64().unwrap_or_default(),
        sequence: output["sequence"].as_str().map(str::to_owned),
        max_size: args
            .get("max_size")
            .and_then(Value::as_u64)
            .and_then(|m| u32::try_from(m).ok()),
        format: Some(if is_jpeg(output) { "jpeg" } else { "png" }.to_owned()),
        quality: args
            .get("quality")
            .and_then(Value::as_u64)
            .and_then(|q| u8::try_from(q).ok()),
    })
}

/// The result with a resource link in place of the image.
#[must_use]
pub fn link(mut output: Value, args: &Value, revs: (Rev, Rev)) -> CallToolResult {
    let image = take_image(&mut output);
    let uri = frame_uri(&output, args, revs.0);
    output["link"] = Value::String(uri.clone());
    let mut summary = summary(&output, revs.0);
    summary.push_str("; the image is large: read the link (resources/read) to get it");
    let mut result = results::success(summary, output, Some(revs));
    let mut resource = Resource::new(uri, "frame");
    if let Some((mime, _)) = image {
        resource = resource.with_mime_type(mime);
    }
    result.content.push(ContentBlock::resource_link(resource));
    result
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn frame(data: &str) -> Value {
        json!({
            "mime": "image/jpeg", "data": data, "bytes": 3, "width": 4, "height": 2,
            "sequence": "seq_1", "time": 705_600_000, "timecode": "00:00:01:00", "rev": 3,
            "items_visible": [{ "id": "itm_1" }], "items_hidden": [], "notes": []
        })
    }

    #[test]
    fn an_image_block_and_metadata_without_the_base64() {
        let result = inline(frame("QUJD"), (Rev(3), Rev(3))).unwrap();
        let s = result.structured_content.unwrap();
        assert!(s.get("data").is_none());
        assert_eq!(s["items_visible"][0]["id"], "itm_1");
        let images = result
            .content
            .iter()
            .filter(|c| matches!(c, ContentBlock::Image(_)))
            .count();
        assert_eq!(images, 1);
    }

    #[test]
    fn a_result_over_the_cap_reports_its_size() {
        let big = "A".repeat(MAX_PAYLOAD);
        assert!(inline(frame(&big), (Rev(3), Rev(3))).unwrap_err() > MAX_PAYLOAD);
    }

    #[test]
    fn large_requests_link_and_qualities_step_down() {
        assert!(wants_link(&json!({ "max_size": 2048 })));
        assert!(!wants_link(&json!({ "max_size": 1568 })));
        assert!(!wants_link(&json!({})));
        assert_eq!(next_quality(None), Some(60));
        assert_eq!(next_quality(Some(60)), Some(40));
        assert_eq!(next_quality(Some(25)), None);
        let result = link(
            frame("QUJD"),
            &json!({ "max_size": 2048 }),
            (Rev(3), Rev(3)),
        );
        let s = result.structured_content.unwrap();
        assert_eq!(
            s["link"],
            "cutweave://frame/3/705600000?sequence=seq_1&max_size=2048&format=jpeg"
        );
        assert!(
            result
                .content
                .iter()
                .any(|c| matches!(c, ContentBlock::ResourceLink(_)))
        );
    }
}
