<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Schema hints (`x-ui`)

`<SchemaForm>` turns a JSON Schema into an accessible form. The schema says what a value _is_; the `x-ui` object on a property says how to _show_ it. Hints never change validation, and the MCP tool schemas leave them out (`lean_schema` drops `x-ui`), so they cost agents no tokens.

Hints come from Rust, with a field attribute on a `#[command]`, `#[query]` or `#[wire]` struct:

```rust
/// Opacity, 0 to 1.
#[ui(widget = "slider", min = 0.0, max = 1.0, step = 0.01, unit = "%", scale = 100.0, keyframeable = true)]
pub opacity: f64,
```

The macro checks keys and value types at compile time (an unknown key or widget is an error on the offending token) and writes `"x-ui": {...}` into that property's schema. A host can add or override hints by path with the `hints` prop (`{ "transform.scale": { linked: true } }`).

## Keys

| Key (Rust / JSON)              | Type   | Meaning                                                                                                                               |
| ------------------------------ | ------ | ------------------------------------------------------------------------------------------------------------------------------------- |
| `widget`                       | string | One of the widgets below. Optional: the type picks a default.                                                                         |
| `label`                        | string | Row label. Default: the schema `title`, else the humanized key (`max_bitrate` is "Max bitrate").                                      |
| `min`, `max`                   | number | Range. Defaults to the schema's `minimum` / `maximum`. A `slider` needs both.                                                         |
| `step`                         | number | Key and drag step, in value units. Default: `multipleOf`, 1 for integers, else about 100 notches over the range (0.1 when open).      |
| `unit`                         | string | Shown after the value and spoken in `aria-valuetext` (`px`, `dB`, `%`, `°`).                                                          |
| `precision`                    | number | Decimals shown. Default: the shortest exact form.                                                                                     |
| `scale`                        | number | Display = value x `scale`. Opacity 0..1 with `scale = 100`, `unit = "%"` shows 0..100 %. `min`, `max` and `step` stay in value units. |
| `group`                        | string | Section name. Fields with the same group share a collapsible section; the rest come first.                                            |
| `order`                        | number | Sort key inside the container; ties keep schema order.                                                                                |
| `placeholder`                  | string | Empty-text hint (text, textarea).                                                                                                     |
| `visible_when` / `visibleWhen` | string | Boolean expression over sibling values (below). Hidden fields are not rendered.                                                       |
| `keyframeable`                 | bool   | Shows the keyframe diamond when the host passes `keyframes`.                                                                          |
| `advanced`                     | bool   | Goes into the container's collapsed "Advanced" section.                                                                               |
| `lines`                        | number | More than 1 renders a `textarea` of that height.                                                                                      |
| `linked`                       | bool   | `vec2`: the axes start linked (an edit scales the other axis by the same ratio).                                                      |
| `kind`                         | string | `asset`: the media kind to offer (`video`, `audio`, `image`).                                                                         |

## Widgets

The widget is chosen from the hint, else from the schema:

| Widget             | Chosen for                                                                  | Value                                                                                                                              |
| ------------------ | --------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `number`           | `number`, `integer`                                                         | number; drag the label, arrow keys, or type; `+10`, `*2`, `/2` and `50%` (of the shown value, unless the unit is `%`) are relative |
| `slider`           | hint, with `min` and `max`                                                  | number                                                                                                                             |
| `angle`            | hint                                                                        | number in degrees; a dial plus a numeric field, Shift snaps the dial to 15°                                                        |
| `color`            | hint on a string                                                            | `"#rrggbb"` or `"#rrggbbaa"`; picker, hex field and alpha. The 8-digit form is kept once a value has alpha                         |
| `toggle`           | `boolean`                                                                   | bool                                                                                                                               |
| `segmented`        | an enum of at most 4 short values                                           | the enum value                                                                                                                     |
| `select`           | any other enum (`enum`, or `oneOf` / `anyOf` of `const`)                    | the enum value; a nullable enum offers "None"                                                                                      |
| `vec2`             | an object with exactly numeric `x`, `y`; an array of 2 numbers; or the hint | `{x, y}` or `[x, y]`, reported whole so one command carries both axes                                                              |
| `tuple`            | an array of a fixed length other than 2, of numbers                         | the whole array                                                                                                                    |
| `array`            | any other array                                                             | the whole array (add, remove); respects `minItems` / `maxItems`                                                                    |
| `object`           | an object with properties                                                   | a titled group; fields report `parent.child` paths                                                                                 |
| `text`, `textarea` | `string`; a number-or-string union (a time) is `text`                       | string, or the number when the union allows it and the text is an integer                                                          |
| `font`             | hint                                                                        | family name as text (placeholder until the font picker, P3)                                                                        |
| `asset`            | hint                                                                        | an asset id string, picked from the `assets` prop (text when none is passed)                                                       |
| `curve`            | hint on an enum                                                             | the easing name; a host `renderers.curve` slot replaces the default select (the curve editor plugs in there)                       |
| `hidden`           | hint                                                                        | not rendered                                                                                                                       |

A shape with no editor (a map with `additionalProperties`, a union of unrelated types) is shown read-only, never silently dropped.

## `visibleWhen`

```
expr    := and ('||' and)*
and     := not ('&&' not)*
not     := '!' not | '(' expr ')' | atom
atom    := path [('==' | '!=') literal]
literal := 'text' | "text" | number | true | false | null
```

A bare path is true when the value is truthy. Paths are dot paths, looked up in the object that holds the field first, then from the form root. A malformed expression leaves the field visible.

```rust
#[ui(visible_when = "rate_control == 'crf'")]
pub crf: Option<u32>,
```

## Reporting edits

`onChange(path, value, {gesture})`. `path` is the dot path of the property inside the params object (`transform.position`, `stops.2.color`; a number indexes an array). It addresses a _field_, never an entity: entities are addressed by ID in the command that the host builds.

- A toggle, picked option, typed or reset value has no `gesture`: one change, one undo step.
- A drag (scrub, slider, dial, color picker) sends `gesture: "start"` with its first streamed value, then `"update"` (at most 60 per second; the latest value always wins), then exactly one `"end"` with the final value. Escape closes the gesture with `end` at the value it started from. A gesture that never streamed (one key press) is a plain one-shot.
- `vec2`, `tuple` and `array` report the whole value at their own path.

The form is controlled and shows `value`; while a gesture runs it shows the pointer's value so a throttled update never snaps the control back.

## Multi-selection, defaults, keyframes

- `mixed`: dot paths where the selection disagrees (`mergeSelection(values)` computes the merged value and the list). Mixed fields show an em dash or "Mixed", never a guess, and offer reset.
- Defaults come from the schema's `default`, or the `defaults` prop by path. Reset shows only while the value differs; it calls `onReset(path)` when given, else sends the default through `onChange`. `onResetGroup(group, paths)` enables the section reset.
- `errors[path]` = the reason a value was refused (an engine `Invalid` pointer turned into a dot path, `/advanced/video/rate_control/mode` → `video.rate_control.mode`): the field's label turns to the danger color and the reason shows under the row (`role="alert"`). A path with no field on screen shows nothing; the host says it elsewhere (WP-2G.04).
- `keyframes[path]` = `{state: "off" | "on" | "elsewhere", onToggle, onPrev?, onNext?}` draws the diamond (filled while a keyframe sits at the playhead, `on`). Without the `keyframes` prop no diamond shows (export dialog); with it, a `keyframeable` field without an entry keeps a disabled slot so rows stay aligned. The form never calls the engine: the host wires these to `keyframe.*` and `item.set_props{auto_key}`.

## Strings and accessibility

Every string the form draws is in `DEFAULT_STRINGS`; pass `strings` to localize. Each control has an accessible name (its row label), keyboard operation and, for numbers, `aria-valuetext` with the unit. The fixtures in `fixtures.ts` are checked with axe (0 violations) and render the expected widget (`SchemaForm.test.ts`); the Storybook gallery shows them all.
