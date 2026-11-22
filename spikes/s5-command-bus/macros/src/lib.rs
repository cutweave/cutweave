//! Spike S5: `#[command]`, `#[query]` and `#[wire]` attribute macros.
//!
//! `#[command(...)]` / `#[query(...)]` turn a params struct into a registered bus entry:
//!
//! 1. add the wire derives (`Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema,
//!    specta::Type`) through `::s5_bus::__private` re-exports, so the defining crate needs no
//!    direct serde/schemars/specta dependency;
//! 2. force `#[serde(deny_unknown_fields)]` (a typo'd agent argument must fail, not be ignored);
//! 3. mark `Option<T>` fields `#[serde(default)]` so they become optional (`field?:` in TS);
//! 4. implement `s5_bus::Described` with a const `CommandMeta` (name, summary, flags, MCP tier,
//!    MCP annotations);
//! 5. submit an `inventory` registration built by `CommandEntry::command::<T>()` /
//!    `CommandEntry::query::<T>()`, which in turn requires the user's `impl Command` /
//!    `impl Query` (a missing impl is a compile error pointing at the attribute).
//!
//! Every policy decision is validated at compile time (name grammar and namespace allowlist,
//! summary length, explicit `undoable` and `destructive` on commands, named-field structs only,
//! UI-only entries never `mcp = "core"`).
//!
//! `scope = "ui"` (WP-0H.02) marks a UI-only entry (arch §6: `ai.set_key`, approvals, allowlists,
//! budgets, plugin grants). The engine refuses it for any caller without the UI credential.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{quote, quote_spanned};
use syn::{
    Fields, ItemStruct, LitBool, LitStr, Type, meta::ParseNestedMeta, parse_macro_input,
    spanned::Spanned,
};

/// Command namespaces from docs/01-architecture.md §5.1. New namespaces require an ADR, so the
/// macro rejects anything else at compile time.
const NAMESPACES: &[&str] = &[
    "project",
    "media",
    "sequence",
    "track",
    "timeline",
    "item",
    "clip",
    "text",
    "caption",
    "transcript",
    "effect",
    "transition",
    "keyframe",
    "mask",
    "color",
    "audio",
    "composition",
    "template",
    "font",
    "asset",
    "analysis",
    "tracking",
    "ai",
    "render",
    "export",
    "plugin",
    "edit",
    "view",
];

/// Upper bound for `summary`. Summaries become MCP tool descriptions, which count against the
/// tool-list token budget (arch §14), so long prose belongs in the doc comment instead.
const MAX_SUMMARY_LEN: usize = 100;

/// Registers a mutating command. See the crate docs for what it generates.
///
/// ```ignore
/// #[command(
///     name = "clip.split",
///     summary = "Split a clip at a timeline time",
///     undoable,
///     mcp = "core",
///     annotations(destructive = false, idempotent = false)
/// )]
/// pub struct SplitClip { pub item: ItemId, pub at: Time }
/// ```
#[proc_macro_attribute]
pub fn command(args: TokenStream, input: TokenStream) -> TokenStream {
    expand_entry(Kind::Command, args, input)
}

/// Registers a read-only query (`readOnlyHint = true`, idempotent, never undoable).
#[proc_macro_attribute]
pub fn query(args: TokenStream, input: TokenStream) -> TokenStream {
    expand_entry(Kind::Query, args, input)
}

/// Marks a type that crosses the wire (outputs and nested param types): adds the same derives
/// and crate paths as `#[command]`, plus `deny_unknown_fields` on named-field structs.
#[proc_macro_attribute]
pub fn wire(args: TokenStream, input: TokenStream) -> TokenStream {
    if !args.is_empty() {
        return syn::Error::new(
            TokenStream2::from(args).span(),
            "#[wire] takes no arguments",
        )
        .to_compile_error()
        .into();
    }
    let item = parse_macro_input!(input as syn::Item);
    let deny = match &item {
        syn::Item::Struct(s) if matches!(s.fields, Fields::Named(_)) => {
            quote!(#[serde(deny_unknown_fields)])
        }
        syn::Item::Struct(_) | syn::Item::Enum(_) => quote!(),
        other => {
            return syn::Error::new(other.span(), "#[wire] supports structs and enums only")
                .to_compile_error()
                .into();
        }
    };
    let derives = wire_derives();
    quote!(#derives #deny #item).into()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Command,
    Query,
}

impl Kind {
    fn attr(self) -> &'static str {
        match self {
            Kind::Command => "command",
            Kind::Query => "query",
        }
    }
}

/// The derive set shared by every wire type. Helper attributes must follow the derive.
fn wire_derives() -> TokenStream2 {
    quote! {
        #[derive(
            ::core::fmt::Debug,
            ::core::clone::Clone,
            ::core::cmp::PartialEq,
            ::s5_bus::__private::serde::Serialize,
            ::s5_bus::__private::serde::Deserialize,
            ::s5_bus::__private::schemars::JsonSchema,
            ::s5_bus::__private::specta::Type
        )]
        #[serde(crate = "::s5_bus::__private::serde")]
        #[schemars(crate = "::s5_bus::__private::schemars")]
        #[specta(crate = ::s5_bus::__private::specta)]
    }
}

#[derive(Default)]
struct Args {
    name: Option<LitStr>,
    summary: Option<LitStr>,
    undoable: Option<LitBool>,
    engine: bool,
    mcp: Option<LitStr>,
    scope: Option<LitStr>,
    destructive: Option<LitBool>,
    idempotent: Option<LitBool>,
    open_world: Option<LitBool>,
    annotations_span: Option<Span>,
}

fn parse_bool_flag(meta: &ParseNestedMeta) -> syn::Result<LitBool> {
    if meta.input.peek(syn::Token![=]) {
        meta.value()?.parse()
    } else {
        Ok(LitBool::new(true, meta.path.span()))
    }
}

fn set_once<T>(
    slot: &mut Option<T>,
    value: T,
    meta: &ParseNestedMeta,
    key: &str,
) -> syn::Result<()> {
    if slot.is_some() {
        return Err(meta.error(format!("duplicate `{key}`")));
    }
    *slot = Some(value);
    Ok(())
}

fn parse_args(kind: Kind, args: TokenStream) -> syn::Result<Args> {
    let mut out = Args::default();
    let parser = syn::meta::parser(|meta| {
        let key = meta
            .path
            .get_ident()
            .map_or_else(String::new, ToString::to_string);
        match key.as_str() {
            "name" => set_once(&mut out.name, meta.value()?.parse()?, &meta, "name"),
            "summary" => set_once(&mut out.summary, meta.value()?.parse()?, &meta, "summary"),
            "mcp" => set_once(&mut out.mcp, meta.value()?.parse()?, &meta, "mcp"),
            "scope" => set_once(&mut out.scope, meta.value()?.parse()?, &meta, "scope"),
            "undoable" if kind == Kind::Command => {
                let v = parse_bool_flag(&meta)?;
                set_once(&mut out.undoable, v, &meta, "undoable")
            }
            "undoable" => Err(meta.error("queries are read-only and never undoable")),
            // Engine ops (history, project lifecycle) implement `EngineOp` instead of `Command`.
            "engine" if kind == Kind::Command => {
                if out.engine {
                    return Err(meta.error("duplicate `engine`"));
                }
                out.engine = true;
                Ok(())
            }
            "annotations" => {
                out.annotations_span = Some(meta.path.span());
                meta.parse_nested_meta(|inner| {
                    let akey = inner
                        .path
                        .get_ident()
                        .map_or_else(String::new, ToString::to_string);
                    match akey.as_str() {
                        "destructive" | "idempotent" if kind == Kind::Query => Err(inner.error(
                            "queries are always read-only and idempotent; only `open_world` may be set",
                        )),
                        "destructive" => {
                            let v = parse_bool_flag(&inner)?;
                            set_once(&mut out.destructive, v, &inner, "destructive")
                        }
                        "idempotent" => {
                            let v = parse_bool_flag(&inner)?;
                            set_once(&mut out.idempotent, v, &inner, "idempotent")
                        }
                        "open_world" => {
                            let v = parse_bool_flag(&inner)?;
                            set_once(&mut out.open_world, v, &inner, "open_world")
                        }
                        "read_only" => Err(inner.error(
                            "`read_only` is derived: #[query] is read-only, #[command] never is",
                        )),
                        _ => Err(inner.error(
                            "unknown annotation; expected `destructive`, `idempotent` or `open_world`",
                        )),
                    }
                })
            }
            _ => Err(meta.error(format!(
                "unknown #[{}] argument; expected `name`, `summary`, {}`mcp`, `scope` or `annotations(...)`",
                kind.attr(),
                if kind == Kind::Command {
                    "`undoable`, `engine`, "
                } else {
                    ""
                }
            ))),
        }
    });
    syn::parse::Parser::parse(parser, args)?;
    Ok(out)
}

fn validate_name(lit: &LitStr) -> syn::Result<()> {
    let name = lit.value();
    let Some((ns, verb)) = name.split_once('.') else {
        return Err(syn::Error::new(
            lit.span(),
            "command name must be `namespace.verb_object` (e.g. \"clip.split\")",
        ));
    };
    let ok_ident = |s: &str| {
        s.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    };
    if !ok_ident(ns) || !ok_ident(verb) {
        return Err(syn::Error::new(
            lit.span(),
            "command name must be lowercase `namespace.verb_object` using [a-z0-9_]",
        ));
    }
    if !NAMESPACES.contains(&ns) {
        return Err(syn::Error::new(
            lit.span(),
            format!(
                "unknown command namespace `{ns}`; new namespaces require an ADR (arch §5.1). Known: {}",
                NAMESPACES.join(", ")
            ),
        ));
    }
    Ok(())
}

fn is_option(ty: &Type) -> bool {
    // Syntactic check: aliases of Option are not detected (documented limitation).
    match ty {
        Type::Path(p) if p.qself.is_none() => p
            .path
            .segments
            .last()
            .is_some_and(|seg| seg.ident == "Option"),
        _ => false,
    }
}

fn expand_entry(kind: Kind, args: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as syn::Item);
    match expand_entry_inner(kind, args, item) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Resolved, validated metadata for one entry (the bools mirror `CommandMeta`/`Annotations`).
#[allow(clippy::struct_excessive_bools)]
struct Meta {
    name: LitStr,
    summary: LitStr,
    mcp_tier: TokenStream2,
    kind: TokenStream2,
    ui_only: bool,
    undoable: bool,
    read_only: bool,
    destructive: bool,
    idempotent: bool,
    open_world: bool,
}

fn resolve_meta(kind: Kind, args: Args) -> syn::Result<Meta> {
    let attr = kind.attr();
    let attr_span = Span::call_site();
    let missing = |what: &str| syn::Error::new(attr_span, format!("#[{attr}] requires {what}"));

    let name = args
        .name
        .ok_or_else(|| missing("`name = \"namespace.verb\"`"))?;
    validate_name(&name)?;
    let summary = args.summary.ok_or_else(|| missing("`summary = \"...\"`"))?;
    let summary_len = summary.value().chars().count();
    if summary_len == 0 || summary_len > MAX_SUMMARY_LEN {
        return Err(syn::Error::new(
            summary.span(),
            format!(
                "summary must be 1..={MAX_SUMMARY_LEN} chars (it is the MCP tool description and \
                 counts against the tool-list budget); put details in the doc comment"
            ),
        ));
    }
    let mcp = args
        .mcp
        .ok_or_else(|| missing("`mcp = \"core\" | \"catalog\" | \"hidden\"`"))?;
    let mcp_tier = match mcp.value().as_str() {
        "core" => quote!(::s5_bus::McpTier::Core),
        "catalog" => quote!(::s5_bus::McpTier::Catalog),
        "hidden" => quote!(::s5_bus::McpTier::Hidden),
        _ => {
            return Err(syn::Error::new(
                mcp.span(),
                "mcp must be \"core\", \"catalog\" or \"hidden\"",
            ));
        }
    };
    let ui_only = match &args.scope {
        None => false,
        Some(s) if s.value() == "ui" => true,
        Some(s) => {
            return Err(syn::Error::new(
                s.span(),
                "scope must be \"ui\" (UI-only, arch §6); omit it for entries any caller may use",
            ));
        }
    };
    if ui_only && mcp.value() == "core" {
        return Err(syn::Error::new(
            mcp.span(),
            "a `scope = \"ui\"` entry cannot be an MCP core tool: agent credentials can never use \
             UI-only scopes, so listing it would only advertise a tool that always fails",
        ));
    }
    let open_world = args.open_world.is_some_and(|b| b.value);

    Ok(match kind {
        Kind::Command => {
            let undoable = args.undoable.ok_or_else(|| {
                syn::Error::new(
                    attr_span,
                    "#[command] must state `undoable` or `undoable = false` explicitly",
                )
            })?;
            let destructive = args.destructive.ok_or_else(|| {
                syn::Error::new(
                    args.annotations_span.unwrap_or(attr_span),
                    "#[command] must declare `annotations(destructive = true|false)`: MCP's \
                     destructiveHint defaults to true, so leaving it implicit would mislabel the tool",
                )
            })?;
            Meta {
                name,
                summary,
                mcp_tier,
                kind: quote!(::s5_bus::EntryKind::Command),
                ui_only,
                undoable: undoable.value,
                read_only: false,
                destructive: destructive.value,
                idempotent: args.idempotent.is_some_and(|b| b.value),
                open_world,
            }
        }
        Kind::Query => Meta {
            name,
            summary,
            mcp_tier,
            kind: quote!(::s5_bus::EntryKind::Query),
            ui_only,
            undoable: false,
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world,
        },
    })
}

fn expand_entry_inner(kind: Kind, args: TokenStream, item: syn::Item) -> syn::Result<TokenStream2> {
    let attr = kind.attr();
    let syn::Item::Struct(mut st) = item else {
        return Err(syn::Error::new(
            item.span(),
            format!("#[{attr}] must be placed on a struct with named fields"),
        ));
    };
    let args = parse_args(kind, args)?;
    check_struct_shape(attr, &st)?;
    let ctor = match kind {
        Kind::Command if args.engine => quote!(engine_op),
        Kind::Command => quote!(command),
        Kind::Query => quote!(query),
    };
    let Meta {
        name,
        summary,
        mcp_tier,
        kind: kind_ts,
        ui_only,
        undoable,
        read_only,
        destructive,
        idempotent,
        open_world,
    } = resolve_meta(kind, args)?;

    // Option<T> params are optional arguments: missing == None, and `field?:` in TS.
    if let Fields::Named(fields) = &mut st.fields {
        for field in &mut fields.named {
            if is_option(&field.ty) {
                field.attrs.push(syn::parse_quote!(#[serde(default)]));
            }
        }
    }

    let ident = &st.ident;
    let derives = wire_derives();
    let rust_type = ident.to_string();
    let ctor = quote_spanned!(name.span()=> #ctor);
    Ok(quote! {
        #derives
        #[serde(deny_unknown_fields)]
        #st

        impl ::s5_bus::Described for #ident {
            const META: ::s5_bus::CommandMeta = ::s5_bus::CommandMeta {
                name: #name,
                summary: #summary,
                kind: #kind_ts,
                ui_only: #ui_only,
                undoable: #undoable,
                mcp: #mcp_tier,
                annotations: ::s5_bus::Annotations {
                    read_only: #read_only,
                    destructive: #destructive,
                    idempotent: #idempotent,
                    open_world: #open_world,
                },
                rust_type: #rust_type,
            };
        }

        ::s5_bus::__private::inventory::submit! {
            ::s5_bus::CommandEntry::#ctor::<#ident>()
        }
    })
}

fn check_struct_shape(attr: &str, st: &ItemStruct) -> syn::Result<()> {
    if !st.generics.params.is_empty() {
        return Err(syn::Error::new(
            st.generics.span(),
            format!(
                "#[{attr}] structs cannot be generic (each registry entry is one concrete type)"
            ),
        ));
    }
    match &st.fields {
        Fields::Named(_) => Ok(()),
        Fields::Unnamed(_) | Fields::Unit => Err(syn::Error::new(
            st.ident.span(),
            format!(
                "#[{attr}] params must be a struct with named fields (use `{{}}` for no params): \
                 the JSON Schema root must be an object (MCP requires inputSchema.type = \"object\")"
            ),
        )),
    }
}
