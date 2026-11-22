// SPDX-License-Identifier: GPL-3.0-or-later
//! End-to-end without the GPU: transpile → QuickJS → scene; props schema; diagnostics.

use std::time::Instant;

use s4b::diagnostics::{ErrorKind, Phase, codes};
use s4b::runtime::{BindOptions, Limits, load_source};
use s4b::{LOWER_THIRD_PROPS, LOWER_THIRD_TSX, STRESS_TSX, transpile};
use serde_json::json;

#[test]
fn lower_third_props_schema_is_json_schema() {
    let comp = load_source("lower-third.tsx", LOWER_THIRD_TSX, Limits::default()).expect("load");
    let info = comp.info();
    assert_eq!(info.id, "acme.lower-third");
    assert_eq!(info.duration_in_frames, 120);
    assert_eq!(info.fps, None);
    assert_eq!(
        info.props_schema,
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "title": { "type": "string" },
                "accent": {
                    "type": "string",
                    "format": "color",
                    "pattern": "^#(?:[0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$",
                    "x-cutweave-widget": "color",
                    "default": "#7c5cff"
                }
            },
            "required": ["name", "title"],
            "additionalProperties": false,
            "x-cutweave-order": ["name", "title", "accent"]
        })
    );
}

#[test]
fn lower_third_frames_follow_spring_and_fade() {
    let comp = load_source("lower-third.tsx", LOWER_THIRD_TSX, Limits::default()).expect("load");
    let ev = comp
        .bind(LOWER_THIRD_PROPS, &BindOptions::default())
        .expect("bind");
    assert_eq!(ev.fps(), 30.0);
    let at = |f: f64| ev.eval(f).expect("eval");

    let f0 = at(0.0);
    assert_eq!(f0.node_count(), 4);
    let g = &f0.nodes[0];
    assert_eq!(g.kind, "group");
    assert_eq!(g.x, Some(-400.0));
    assert_eq!(g.y, Some(860.0));
    assert_eq!(g.opacity, Some(1.0));
    assert_eq!(g.children[0].fill.as_deref(), Some("#7c5cff"));
    assert_eq!(g.children[1].text.as_deref(), Some("Ada Lovelace"));
    assert_eq!(
        g.children[2].text.as_deref(),
        Some("Analyst, Analytical Engines")
    );

    let f60 = at(60.0);
    let x60 = f60.nodes[0].x.expect("x");
    assert!((x60 - 80.0).abs() < 0.01, "settled at 80, got {x60}");
    let f10 = at(10.0);
    assert!(
        f10.nodes[0].x.expect("x") > 80.0,
        "spring overshoots (damping ratio 0.6)"
    );
    let o119 = at(119.0).nodes[0].opacity.expect("opacity");
    assert!((o119 - 0.05).abs() < 1e-9, "{o119}");
}

#[test]
fn props_errors_are_structured_with_json_pointer_paths() {
    let comp = load_source("lower-third.tsx", LOWER_THIRD_TSX, Limits::default()).expect("load");
    let opts = BindOptions::default();
    let err = |props: &str| comp.bind(props, &opts).err().expect("props error");

    let e = err("{}");
    assert_eq!(e.kind, ErrorKind::Props);
    assert_eq!(e.code(), codes::PROPS);
    assert_eq!(e.diagnostics[0].phase, Phase::Props);
    assert_eq!(e.diagnostics[0].path.as_deref(), Some("/name"));
    assert_eq!(e.diagnostics[0].message, "required");

    let e = err(r#"{"name":"a","title":"b","accent":"red"}"#);
    assert_eq!(e.diagnostics[0].path.as_deref(), Some("/accent"));
    assert!(e.diagnostics[0].message.contains("hex colour"));

    let e = err(r##"{"name":"a","title":"b","colour":"#fff"}"##);
    assert_eq!(e.diagnostics[0].path.as_deref(), Some("/colour"));

    let e = err(r#"{"name":1,"title":"b"}"#);
    assert_eq!(e.diagnostics[0].message, "expected string, got number");
}

#[test]
fn runtime_errors_map_back_to_tsx_lines() {
    let src = r#"import { defineComposition, interpolate } from "@cutweave/compose";
type T = { a: number };
export default defineComposition({
  id: "t.throw",
  durationInFrames: 10,
  render: ({ frame }) => {
    const t: T = { a: frame };
    if (t.a === 2) throw new RangeError("too late");
    if (t.a === 3) return interpolate(frame, [0], [1]) as never;
    return [];
  },
});
"#;
    let comp = load_source("throw.tsx", src, Limits::default()).expect("load");
    let ev = comp.bind("{}", &BindOptions::default()).expect("bind");
    ev.eval(1.0).expect("frame 1 fine");

    let e = ev.eval(2.0).expect_err("throws");
    let d = &e.diagnostics[0];
    assert_eq!((e.kind, d.code), (ErrorKind::Runtime, codes::EXCEPTION));
    assert_eq!(d.message, "RangeError: too late");
    assert_eq!(d.file.as_deref(), Some("throw.tsx"));
    assert_eq!(d.line, Some(8), "{d:?}");
    assert!(
        d.stack.iter().any(|f| f.contains("throw.tsx:8:")),
        "{:?}",
        d.stack
    );

    // Thrown inside @cutweave/compose: located at the caller's line in the composition.
    let e = ev.eval(3.0).expect_err("bad interpolate");
    let d = &e.diagnostics[0];
    assert!(d.message.contains("interpolate()"), "{}", d.message);
    assert_eq!(d.line, Some(9), "{d:?}");
}

#[test]
fn module_errors_and_bad_definitions_are_structured() {
    let e = load_source(
        "def.tsx",
        "import { defineComposition } from \"@cutweave/compose\";\n\
         export default defineComposition({ id: \"Bad Id\", durationInFrames: 1, render: () => [] });\n",
        Limits::default(),
    )
    .err()
    .expect("bad id");
    assert_eq!(e.code(), codes::DEFINITION);
    assert_eq!(e.diagnostics[0].line, Some(2));

    let e = load_source(
        "plain.ts",
        "export default { render() {} };",
        Limits::default(),
    )
    .err()
    .expect("not a composition");
    assert_eq!(e.code(), codes::DEFINITION);

    let e = load_source(
        "top.ts",
        "export default 1;\nwhile (true) {}\n",
        Limits::default(),
    )
    .err()
    .expect("top-level loop");
    assert_eq!(e.kind, ErrorKind::Timeout);
}

#[test]
fn unknown_elements_and_bad_children_are_scene_errors() {
    let src = r#"import { defineComposition, Group } from "@cutweave/compose";
export default defineComposition({
  id: "t.child", durationInFrames: 2,
  render: ({ frame }) => frame === 0 ? <Group>{"loose text"}</Group> : <blink />,
});
"#;
    let comp = load_source("child.tsx", src, Limits::default()).expect("load");
    let ev = comp.bind("{}", &BindOptions::default()).expect("bind");
    let e = ev.eval(0.0).expect_err("loose text");
    assert_eq!((e.kind, e.code()), (ErrorKind::Scene, codes::SCENE_INVALID));
    assert_eq!(e.diagnostics[0].line, Some(4));
    let e = ev.eval(1.0).expect_err("unknown element");
    assert!(e.diagnostics[0].message.contains("<blink>"));
}

#[test]
fn stress_composition_has_200_nodes_and_is_pure() {
    let comp = load_source("stress-200.tsx", STRESS_TSX, Limits::default()).expect("load");
    assert_eq!(comp.info().fps, Some(60.0));
    let ev = comp
        .bind("{}", &BindOptions::default())
        .expect("bind with defaults");
    for f in [0.0, 30.0, 239.0] {
        assert_eq!(ev.eval(f).expect("eval").node_count(), 200);
    }
    ev.check_pure(&[0.0, 17.0, 120.0, 239.0]).expect("pure");
}

#[test]
fn impure_composition_is_detected() {
    let src = r#"import { defineComposition, Rect } from "@cutweave/compose";
let calls = 0;
export default defineComposition({
  id: "t.impure", durationInFrames: 10,
  render: () => <Rect w={++calls} h={1} />,
});
"#;
    let comp = load_source("impure.tsx", src, Limits::default()).expect("load");
    let ev = comp.bind("{}", &BindOptions::default()).expect("bind");
    let e = ev.check_pure(&[0.0, 1.0]).expect_err("stateful");
    assert_eq!(e.code(), codes::IMPURE);
}

#[test]
fn transpile_is_well_under_50_ms() {
    // Informal guard; the real numbers come from `s4b bench` (README).
    for (name, src) in [
        ("lower-third.tsx", LOWER_THIRD_TSX),
        ("stress-200.tsx", STRESS_TSX),
    ] {
        transpile(name, src).expect("warm-up");
        let t = Instant::now();
        for _ in 0..10 {
            transpile(name, src).expect("transpile");
        }
        let per = t.elapsed().as_secs_f64() * 1e3 / 10.0;
        assert!(per < 50.0, "{name}: {per:.2} ms");
    }
}

#[test]
fn console_output_is_returned_with_the_frame() {
    let src = r#"import { defineComposition } from "@cutweave/compose";
export default defineComposition({
  id: "t.log", durationInFrames: 2,
  render: ({ frame, props }) => { console.log("frame", frame, { props }); return []; },
});
"#;
    let comp = load_source("log.tsx", src, Limits::default()).expect("load");
    let ev = comp.bind("{}", &BindOptions::default()).expect("bind");
    let out = ev.eval(1.0).expect("eval");
    assert_eq!(out.logs.len(), 1);
    assert_eq!(out.logs[0].message, "frame 1 {\"props\":{}}");
}
