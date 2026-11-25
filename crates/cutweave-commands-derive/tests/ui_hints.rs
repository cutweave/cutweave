// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2H.02: `#[ui(...)]` adds an `x-ui` object to a property's JSON Schema (read by the
//! schema-driven form in `packages/ui`) and changes nothing else; the TypeScript types stay
//! generated from the same Rust type.

#![allow(clippy::unwrap_used)]

use cutweave_commands::{CmdError, Command, Registry, Tx, command, wire};
use serde_json::json;

/// A 2D offset.
#[wire]
pub struct Offset {
    /// Horizontal.
    #[ui(widget = "number", unit = "px", step = 1, min = -8192, max = 8192)]
    pub x: f64,
    /// Vertical.
    #[ui(widget = "number", unit = "px", step = 1)]
    pub y: f64,
}

/// Sets look properties (a fixture for the hints).
#[command(
    name = "item.set_look",
    summary = "Set the look of an item",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct SetLook {
    /// Opacity, 0 to 1.
    #[ui(
        widget = "slider",
        min = 0.0,
        max = 1.0,
        step = 0.01,
        unit = "%",
        scale = 100.0,
        keyframeable = true
    )]
    pub opacity: f64,
    /// Blend mode.
    #[ui(group = "Look", order = 2)]
    pub blend: String,
    /// Where it sits.
    pub position: Offset,
    /// Shown only for a custom blend.
    #[ui(visible_when = "blend == 'custom'", placeholder = "Expression")]
    pub custom: Option<String>,
    /// No hint at all.
    pub plain: bool,
}

/// Empty result.
#[wire]
#[derive(Default)]
pub struct Done {}

impl Command for SetLook {
    type Output = Done;
    fn apply(&self, _: &mut Tx) -> Result<Done, CmdError> {
        Ok(Done {})
    }
}

fn schema() -> serde_json::Value {
    let mut registry = Registry::new();
    registry.register::<SetLook>().map(|_| ()).unwrap();
    registry
        .get("item.set_look")
        .unwrap()
        .meta
        .input_schema
        .clone()
}

#[test]
fn ui_attribute_adds_x_ui_to_the_schema() {
    let schema = schema();
    let props = &schema["properties"];
    assert_eq!(
        props["opacity"]["x-ui"],
        json!({"widget": "slider", "min": 0.0, "max": 1.0, "step": 0.01, "unit": "%",
               "scale": 100.0, "keyframeable": true})
    );
    assert_eq!(props["blend"]["x-ui"], json!({"group": "Look", "order": 2}));
    assert_eq!(
        props["custom"]["x-ui"],
        json!({"visibleWhen": "blend == 'custom'", "placeholder": "Expression"})
    );
    assert!(props["plain"].get("x-ui").is_none());
    insta::assert_snapshot!(
        "set_look_input_schema",
        cutweave_model::canonical_json_pretty(&schema)
    );
}

#[test]
fn ui_hints_reach_nested_wire_types_and_do_not_change_validation() {
    let schema = schema();
    let offset = &schema["properties"]["position"]["properties"];
    assert_eq!(offset["x"]["x-ui"]["min"], json!(-8192));
    assert_eq!(offset["y"]["x-ui"]["unit"], json!("px"));
    // A value is valid or not exactly as before: the hint is documentation only.
    let ok: SetLook = serde_json::from_value(json!({
        "opacity": 0.5, "blend": "normal", "position": {"x": 1.0, "y": 2.0}, "plain": true
    }))
    .unwrap();
    assert!((ok.opacity - 0.5).abs() < f64::EPSILON);
}
