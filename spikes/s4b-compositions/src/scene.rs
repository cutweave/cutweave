// SPDX-License-Identifier: GPL-3.0-or-later
//! The composition scene format: what `render()` returns, as JSON.
//!
//! Plain data, one flat node shape: `{ type, id?, x, y, ...props, children? }` with `type` ∈
//! `group | rect | ellipse | text | image`. The same JSON is what a *declarative* (no-JS)
//! composition would contain, which keeps the documented fallback (declarative JSON scenes +
//! expressions) on the same lowering path.

use serde::{Deserialize, Deserializer, Serialize};

/// `null` (what `JSON.stringify` makes of NaN/±Infinity) becomes NaN so lowering can report the
/// offending prop by path instead of failing the whole parse.
fn num<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    Ok(Some(Option::<f64>::deserialize(d)?.unwrap_or(f64::NAN)))
}

fn pair<'de, D: Deserializer<'de>>(d: D) -> Result<Option<[f64; 2]>, D::Error> {
    let v = Option::<[Option<f64>; 2]>::deserialize(d)?;
    Ok(Some(match v {
        Some([a, b]) => [a.unwrap_or(f64::NAN), b.unwrap_or(f64::NAN)],
        None => [f64::NAN, f64::NAN],
    }))
}

/// One scene node (all props optional at the JSON level; lowering validates per type).
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct RawNode {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(deserialize_with = "num", skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(deserialize_with = "num", skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    #[serde(deserialize_with = "num", skip_serializing_if = "Option::is_none")]
    pub w: Option<f64>,
    #[serde(deserialize_with = "num", skip_serializing_if = "Option::is_none")]
    pub h: Option<f64>,
    /// Corner radius (rect).
    #[serde(deserialize_with = "num", skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    /// `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa` (sRGB, straight alpha).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill: Option<String>,
    #[serde(deserialize_with = "num", skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    /// Clockwise degrees around the anchor.
    #[serde(deserialize_with = "num", skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f64>,
    #[serde(deserialize_with = "pair", skip_serializing_if = "Option::is_none")]
    pub scale: Option<[f64; 2]>,
    /// Normalised anchor (0,0 = top-left of the node; ignored for groups, which pivot on their
    /// local origin).
    #[serde(deserialize_with = "pair", skip_serializing_if = "Option::is_none")]
    pub anchor: Option<[f64; 2]>,
    /// Blend mode name (`normal`, `multiply`, `screen`, `overlay`, `darken`, `lighten`, `add`,
    /// `soft_light`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blend: Option<String>,
    /// Blur radius in px (S4 blur-lite).
    #[serde(deserialize_with = "num", skip_serializing_if = "Option::is_none")]
    pub blur: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// Font size in px (text).
    #[serde(deserialize_with = "num", skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    /// Asset id (image).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,
    /// Source frame (image/video assets).
    #[serde(deserialize_with = "num", skip_serializing_if = "Option::is_none")]
    pub frame: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<RawNode>,
}

impl RawNode {
    /// Number of nodes in this subtree (including itself).
    #[must_use]
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(Self::count).sum::<usize>()
    }
}

/// A `console.*` line captured during the frame.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct LogEntry {
    pub level: String,
    pub message: String,
}

/// What one `renderFrame(frame)` call returns.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FrameOutput {
    pub nodes: Vec<RawNode>,
    pub logs: Vec<LogEntry>,
    pub dropped_logs: u32,
}

impl FrameOutput {
    /// Total node count.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.iter().map(RawNode::count).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nodes_and_maps_null_numbers_to_nan() {
        let json = r##"{"nodes":[{"type":"group","x":1,"children":[
            {"type":"rect","w":10,"h":null,"fill":"#fff","scale":[2,null]},
            {"type":"text","text":"Hi","unknownProp":true}]}],"logs":[],"droppedLogs":0}"##;
        let out: FrameOutput = serde_json::from_str(json).expect("parse");
        assert_eq!(out.node_count(), 3);
        let rect = &out.nodes[0].children[0];
        assert_eq!(rect.w, Some(10.0));
        assert!(rect.h.is_some_and(f64::is_nan));
        assert!(rect.scale.is_some_and(|s| s[0] == 2.0 && s[1].is_nan()));
        assert_eq!(rect.x, None);
    }
}
