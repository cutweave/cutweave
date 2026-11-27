// SPDX-License-Identifier: GPL-3.0-or-later
//! Parsed form of `design/tokens.json`, alias resolution and colour parsing.

use serde_json::Value;

/// A token whose value can differ per theme (colours and shadows).
#[derive(Debug, Clone)]
pub struct ThemedToken {
    pub name: String,
    /// Raw value per theme, in `Tokens::themes` order (missing themes fall back to the first).
    pub values: Vec<String>,
    pub usage: String,
}

/// A theme-independent token (spacing, radius, size, ...).
#[derive(Debug, Clone)]
pub struct PlainToken {
    pub name: String,
    pub value: String,
}

/// A non-colour family such as `spacing` or `zIndex`.
#[derive(Debug, Clone)]
pub struct Family {
    pub key: String,
    pub tokens: Vec<PlainToken>,
}

/// A density other than the default one (`density.sets` in the token file): the size and
/// spacing tokens it makes larger, under `[data-density="<id>"]`.
#[derive(Debug, Clone)]
pub struct Density {
    pub id: String,
    /// The tokens it overrides, in file order.
    pub tokens: Vec<PlainToken>,
}

/// One typography style.
#[derive(Debug, Clone)]
pub struct TypeStyle {
    pub name: String,
    pub family: String,
    pub font_size: String,
    pub line_height: String,
    pub font_weight: String,
    pub letter_spacing: Option<String>,
}

/// The whole token file.
#[derive(Debug, Clone)]
pub struct Tokens {
    pub name: String,
    /// Theme ids, `dark` first.
    pub themes: Vec<String>,
    pub colors: Vec<ThemedToken>,
    pub shadows: Vec<ThemedToken>,
    pub families: Vec<Family>,
    /// Densities besides the default one (the plain `families` values).
    pub densities: Vec<Density>,
    pub font_families: Vec<(String, String)>,
    pub type_styles: Vec<TypeStyle>,
}

/// An sRGB colour with 8-bit channels and straight (non-premultiplied) alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

fn str_of<'a>(v: &'a Value, key: &str, ctx: &str) -> Result<&'a str, String> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{ctx}: missing string `{key}`"))
}

fn scalar(v: &Value, ctx: &str) -> Result<String, String> {
    match v {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        _ => Err(format!("{ctx}: expected a string or number")),
    }
}

fn themed(list: &Value, themes: &[String], ctx: &str) -> Result<Vec<ThemedToken>, String> {
    let arr = list
        .get("tokens")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{ctx}: missing `tokens` array"))?;
    let mut out = Vec::new();
    for tok in arr {
        let name = str_of(tok, "name", ctx)?.to_owned();
        let raw = tok
            .get("value")
            .ok_or_else(|| format!("{ctx}.{name}: missing value"))?;
        let values = match raw {
            Value::Object(map) => {
                let first = map
                    .get(&themes[0])
                    .map(|v| scalar(v, &name))
                    .transpose()?
                    .ok_or_else(|| format!("{ctx}.{name}: no value for theme `{}`", themes[0]))?;
                let mut vals = Vec::new();
                for t in themes {
                    vals.push(match map.get(t) {
                        Some(v) => scalar(v, &name)?,
                        None => first.clone(),
                    });
                }
                vals
            }
            other => vec![scalar(other, &name)?; themes.len()],
        };
        let usage = tok
            .get("usage")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        out.push(ThemedToken {
            name,
            values,
            usage,
        });
    }
    Ok(out)
}

impl Tokens {
    /// Parses the contents of `tokens.json`.
    pub fn parse(json: &str) -> Result<Self, String> {
        let root: Value = serde_json::from_str(json).map_err(|e| format!("tokens.json: {e}"))?;
        let obj = root.as_object().ok_or("tokens.json: not an object")?;
        let name = str_of(&root, "name", "tokens.json")?.to_owned();
        let color = obj.get("color").ok_or("tokens.json: missing `color`")?;
        let themes: Vec<String> = color
            .get("themes")
            .and_then(Value::as_array)
            .ok_or("color: missing `themes`")?
            .iter()
            .map(|t| str_of(t, "id", "color.themes").map(str::to_owned))
            .collect::<Result<_, _>>()?;
        if themes.is_empty() {
            return Err("color.themes is empty".into());
        }
        let colors = themed(color, &themes, "color")?;
        let shadows = match obj.get("shadow") {
            Some(s) => themed(s, &themes, "shadow")?,
            None => Vec::new(),
        };

        let mut families = Vec::new();
        for (key, body) in obj {
            if matches!(key.as_str(), "color" | "type" | "shadow") {
                continue;
            }
            let Some(list) = body.get("tokens").and_then(Value::as_array) else {
                continue;
            };
            let mut tokens = Vec::new();
            for tok in list {
                tokens.push(PlainToken {
                    name: str_of(tok, "name", key)?.to_owned(),
                    value: scalar(tok.get("value").unwrap_or(&Value::Null), key)?,
                });
            }
            families.push(Family {
                key: key.clone(),
                tokens,
            });
        }

        let densities = match obj.get("density") {
            Some(d) => parse_densities(d, &families)?,
            None => Vec::new(),
        };

        let ty = obj.get("type").ok_or("tokens.json: missing `type`")?;
        let font_families = ty
            .get("families")
            .and_then(Value::as_object)
            .ok_or("type: missing `families`")?
            .iter()
            .map(|(k, v)| scalar(v, k).map(|s| (k.clone(), s)))
            .collect::<Result<Vec<_>, _>>()?;
        let mut type_styles = Vec::new();
        for g in ty
            .get("groups")
            .and_then(Value::as_array)
            .ok_or("type: missing `groups`")?
        {
            let gfam = str_of(g, "family", "type.groups")?;
            for s in g
                .get("styles")
                .and_then(Value::as_array)
                .ok_or("type.groups: missing `styles`")?
            {
                let sname = str_of(s, "name", "type.styles")?.to_owned();
                let field = |k: &str| -> Result<String, String> {
                    scalar(
                        s.get(k).unwrap_or(&Value::Null),
                        &format!("type.{sname}.{k}"),
                    )
                };
                type_styles.push(TypeStyle {
                    family: s
                        .get("family")
                        .and_then(Value::as_str)
                        .unwrap_or(gfam)
                        .to_owned(),
                    font_size: field("fontSize")?,
                    line_height: field("lineHeight")?,
                    font_weight: field("fontWeight")?,
                    letter_spacing: s
                        .get("letterSpacing")
                        .map(|v| scalar(v, &sname))
                        .transpose()?,
                    name: sname,
                });
            }
        }
        Ok(Self {
            name,
            themes,
            colors,
            shadows,
            families,
            densities,
            font_families,
            type_styles,
        })
    }

    /// Resolves colour token `name` for theme index `theme` to a concrete colour,
    /// following `{alias}` references.
    pub fn resolve_color(&self, name: &str, theme: usize) -> Result<Rgba, String> {
        let mut current = name;
        for _ in 0..=self.colors.len() {
            let tok = self
                .colors
                .iter()
                .find(|t| t.name == current)
                .ok_or_else(|| format!("unknown colour token `{current}`"))?;
            let raw = tok.values[theme].trim();
            match alias_target(raw) {
                Some(next) => current = next,
                None => return parse_hex(raw).map_err(|e| format!("{current}: {e}")),
            }
        }
        Err(format!("alias cycle starting at `{name}`"))
    }
}

/// A `12px` length as a number.
fn px_of(value: &str) -> Option<f64> {
    value.strip_suffix("px")?.parse::<f64>().ok()
}

/// Parses `density.sets`. A set may only make a `spacing` or `size` token larger: every token it
/// names exists there, is a px length with a usage note, and is not smaller than the default.
/// No control or track may end up smaller than the set's own `hit-min`.
fn parse_densities(density: &Value, families: &[Family]) -> Result<Vec<Density>, String> {
    let base = |name: &str| -> Option<&str> {
        families
            .iter()
            .filter(|f| matches!(f.key.as_str(), "spacing" | "size"))
            .flat_map(|f| &f.tokens)
            .find(|t| t.name == name)
            .map(|t| t.value.as_str())
    };
    let sets = density
        .get("sets")
        .and_then(Value::as_array)
        .ok_or("density: missing `sets` array")?;
    let mut out: Vec<Density> = Vec::new();
    for set in sets {
        let id = str_of(set, "id", "density.sets")?.to_owned();
        let ctx = format!("density.{id}");
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_lowercase() || c == '-') {
            return Err(format!("{ctx}: the id must be lower-case letters"));
        }
        if out.iter().any(|d| d.id == id) {
            return Err(format!("{ctx}: defined twice"));
        }
        let list = set
            .get("tokens")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("{ctx}: missing `tokens` array"))?;
        let mut tokens: Vec<PlainToken> = Vec::new();
        for tok in list {
            let name = str_of(tok, "name", &ctx)?.to_owned();
            let value = scalar(tok.get("value").unwrap_or(&Value::Null), &ctx)?;
            let usage = tok.get("usage").and_then(Value::as_str).unwrap_or_default();
            if usage.trim().is_empty() {
                return Err(format!("{ctx}.{name}: needs a usage note"));
            }
            if tokens.iter().any(|t| t.name == name) {
                return Err(format!("{ctx}.{name}: listed twice"));
            }
            let standard = base(&name)
                .ok_or_else(|| format!("{ctx}.{name}: not a `spacing` or `size` token"))?;
            let (Some(new), Some(old)) = (px_of(&value), px_of(standard)) else {
                return Err(format!("{ctx}.{name}: `{value}` must be a px length"));
            };
            if new < old {
                return Err(format!(
                    "{ctx}.{name}: {value} is smaller than the default {standard}"
                ));
            }
            tokens.push(PlainToken { name, value });
        }
        let resolved = |name: &str| -> Option<f64> {
            tokens
                .iter()
                .find(|t| t.name == name)
                .map(|t| t.value.as_str())
                .or_else(|| base(name))
                .and_then(px_of)
        };
        if let Some(hit) = resolved("hit-min") {
            let sizes = families.iter().filter(|f| f.key == "size");
            for t in sizes.flat_map(|f| &f.tokens) {
                let target = t.name.starts_with("control-") || t.name.starts_with("track-");
                if target && resolved(&t.name).is_some_and(|v| v < hit) {
                    return Err(format!("{ctx}: `{}` is smaller than `hit-min`", t.name));
                }
            }
        }
        out.push(Density { id, tokens });
    }
    Ok(out)
}

/// Returns `x` for `"{x}"`.
pub fn alias_target(v: &str) -> Option<&str> {
    let v = v.trim();
    let inner = v.strip_prefix('{')?.strip_suffix('}')?;
    (!inner.is_empty()
        && inner
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')))
    .then_some(inner)
}

/// Parses `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`.
pub fn parse_hex(s: &str) -> Result<Rgba, String> {
    let digits = s
        .strip_prefix('#')
        .ok_or_else(|| format!("`{s}` is not a #hex colour"))?;
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("`{s}` has non-hex digits"));
    }
    let nib = |i: usize| u8::from_str_radix(&digits[i..=i], 16).map(|n| n * 17);
    let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16);
    let bad = |_| format!("`{s}` is not a valid colour");
    let (red, green, blue, alpha) = match digits.len() {
        3 => (nib(0), nib(1), nib(2), Ok(255)),
        4 => (nib(0), nib(1), nib(2), nib(3)),
        6 => (byte(0), byte(2), byte(4), Ok(255)),
        8 => (byte(0), byte(2), byte(4), byte(6)),
        _ => return Err(format!("`{s}` must have 3, 4, 6 or 8 hex digits")),
    };
    Ok(Rgba {
        r: red.map_err(bad)?,
        g: green.map_err(bad)?,
        b: blue.map_err(bad)?,
        a: alpha.map_err(bad)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINI: &str = r##"{
      "name": "T", "version": 1,
      "color": {
        "themes": [{"id": "dark"}, {"id": "light"}],
        "tokens": [
          {"name": "gold", "value": "#F2B33D", "usage": "u"},
          {"name": "playhead", "value": "{gold}", "usage": "u"},
          {"name": "chain", "value": {"dark": "{playhead}", "light": "#fff"}, "usage": "u"},
          {"name": "loop-a", "value": "{loop-b}", "usage": "u"},
          {"name": "loop-b", "value": "{loop-a}", "usage": "u"}
        ]
      },
      "type": {"families": {"ui": "x"}, "groups": []}
    }"##;

    #[test]
    fn hex_forms() {
        let c = parse_hex("#f2b33d24").unwrap();
        assert_eq!((c.r, c.g, c.b, c.a), (0xf2, 0xb3, 0x3d, 0x24));
        assert_eq!(parse_hex("#fff").unwrap().a, 255);
        assert_eq!(parse_hex("#f008").unwrap().a, 0x88);
        assert!(parse_hex("f2b33d").is_err());
        assert!(parse_hex("#12345").is_err());
        assert!(parse_hex("#gg0000").is_err());
    }

    #[test]
    fn aliases_resolve_per_theme_and_detect_cycles() {
        let t = Tokens::parse(MINI).unwrap();
        let gold = t.resolve_color("gold", 0).unwrap();
        assert_eq!(t.resolve_color("playhead", 1).unwrap(), gold);
        assert_eq!(t.resolve_color("chain", 0).unwrap(), gold);
        assert_eq!(t.resolve_color("chain", 1).unwrap().r, 255);
        assert!(t.resolve_color("loop-a", 0).unwrap_err().contains("cycle"));
        assert!(t.resolve_color("nope", 0).is_err());
    }

    fn with_density(set: &str) -> Result<Tokens, String> {
        let json = r##"{
          "name": "T", "version": 1,
          "color": {"themes": [{"id": "dark"}], "tokens": []},
          "spacing": {"tokens": [{"name": "space-2", "value": "8px", "usage": "u"}]},
          "size": {"tokens": [
            {"name": "hit-min", "value": "24px", "usage": "u"},
            {"name": "control-sm", "value": "24px", "usage": "u"}
          ]},
          "zIndex": {"tokens": [{"name": "z-top", "value": "20", "usage": "u"}]},
          "density": {"sets": [{"id": "comfortable", "tokens": [SET]}]},
          "type": {"families": {"ui": "x"}, "groups": []}
        }"##;
        Tokens::parse(&json.replace("SET", set))
    }

    #[test]
    fn a_density_makes_size_and_spacing_tokens_larger() {
        let t = with_density(
            r#"{"name": "control-sm", "value": "28px", "usage": "u"},
               {"name": "hit-min", "value": "28px", "usage": "u"},
               {"name": "space-2", "value": "10px", "usage": "u"}"#,
        )
        .unwrap();
        assert_eq!(t.densities.len(), 1);
        assert_eq!(t.densities[0].id, "comfortable");
        let names: Vec<&str> = t.densities[0]
            .tokens
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names, ["control-sm", "hit-min", "space-2"]);
        // The default values are untouched, and `density` is not a token family of its own.
        assert!(t.families.iter().all(|f| f.key != "density"));
        let size = t.families.iter().find(|f| f.key == "size").unwrap();
        assert_eq!(size.tokens[1].value, "24px");
    }

    #[test]
    fn a_density_is_checked() {
        let err = |set: &str| with_density(set).unwrap_err();
        assert!(err(r#"{"name": "nope", "value": "28px", "usage": "u"}"#).contains("not a"));
        assert!(err(r#"{"name": "z-top", "value": "28px", "usage": "u"}"#).contains("not a"));
        assert!(err(r#"{"name": "space-2", "value": "6px", "usage": "u"}"#).contains("smaller"));
        assert!(err(r#"{"name": "space-2", "value": "1em", "usage": "u"}"#).contains("px length"));
        assert!(err(r#"{"name": "space-2", "value": "10px", "usage": " "}"#).contains("usage"));
        assert!(err(r#"{"name": "space-2", "value": "10px"}"#).contains("usage"));
        // A larger minimum target with a control left below it.
        assert!(err(r#"{"name": "hit-min", "value": "28px", "usage": "u"}"#).contains("hit-min"));
        let twice = r#"{"name": "space-2", "value": "10px", "usage": "u"},
                       {"name": "space-2", "value": "12px", "usage": "u"}"#;
        assert!(err(twice).contains("twice"));
    }

    #[test]
    fn alias_syntax() {
        assert_eq!(alias_target("{marigold}"), Some("marigold"));
        assert_eq!(alias_target("#fff"), None);
        assert_eq!(alias_target("{}"), None);
    }
}
