"""Generate tokens.css from design/tokens.json (Cutweave design system).

Usage: python -I design/scripts/tokens_to_css.py design/tokens.json design/tokens.css

Layout follows the design-system artifact's compiled form:
  :root, [data-theme="<first>"] { colors + shadows }
  [data-theme="<other>"] { overrides }
  :root { spacing, radius, other families, --font-<key> }
  .<style> { ... } per type style
Phase 0 (WP-0K) replaces this with the full pipeline (Tailwind @theme, TS, Rust).
"""
import json, re, sys

src, out = sys.argv[1], sys.argv[2]
t = json.load(open(src, encoding="utf-8"))
themes = [th["id"] for th in t["color"]["themes"]]
first = themes[0]

def theme_val(tok, theme):
    v = tok["value"]
    if isinstance(v, str):
        return v
    return v.get(theme, v.get(first))

def css(v):
    m = re.fullmatch(r"\{([A-Za-z0-9_.-]+)\}", v.strip())
    return f"var(--{m.group(1)})" if m else v.lower() if v.startswith("#") else v

themed = t["color"]["tokens"] + t.get("shadow", {}).get("tokens", [])
lines = [f"/* {t['name']} — generated from tokens.json */"]
lines.append(f':root, [data-theme="{first}"] {{')
lines += [f"  --{tok['name']}: {css(theme_val(tok, first))};" for tok in themed]
lines.append("}")
for th in themes[1:]:
    lines.append(f'[data-theme="{th}"] {{')
    lines += [f"  --{tok['name']}: {css(theme_val(tok, th))};" for tok in themed]
    lines.append("}")
lines.append(":root {")
skip = {"name", "version", "meta", "color", "type", "shadow"}
for fam, body in t.items():
    if fam in skip or not isinstance(body, dict) or "tokens" not in body:
        continue
    lines += [f"  --{tok['name']}: {tok['value']};" for tok in body["tokens"]]
for key, stack in t["type"]["families"].items():
    lines.append(f"  --font-{key}: {stack};")
lines.append("}")
for g in t["type"]["groups"]:
    for s in g["styles"]:
        fam = s.get("family", g["family"])
        decl = [f"font-family: var(--font-{fam})", f"font-size: {s['fontSize']}",
                f"line-height: {s['lineHeight']}", f"font-weight: {s['fontWeight']}"]
        if "letterSpacing" in s:
            decl.append(f"letter-spacing: {s['letterSpacing']}")
        lines.append(f".{s['name']} {{ " + "; ".join(decl) + "; }")
open(out, "w", encoding="utf-8", newline="\n").write("\n".join(lines) + "\n")
print(f"wrote {out}: {len(lines)} lines")
