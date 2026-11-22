// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-commands-derive`: the `#[command]`, `#[query]` and `#[wire]` attribute macros
//! (arch §5.1). Use them through `cutweave_commands`, which re-exports them.
//!
//! `#[command(...)]` / `#[query(...)]` turn a params struct into a registry entry:
//!
//! 1. add the wire derives (`Clone, PartialEq, Serialize, Deserialize, JsonSchema,
//!    specta::Type`) through `cutweave_commands::__private`, so a command crate needs no direct
//!    serde/schemars/specta dependency, plus a `Debug` that prints `#[secret]` params as
//!    `[redacted]`;
//! 2. force `#[serde(deny_unknown_fields)]` (a mistyped agent argument fails with `Invalid` at
//!    its JSON Pointer instead of being ignored) and `#[serde(default)]` on `Option` params
//!    (optional in serde, the JSON Schema and TypeScript); doc comments become the schema's
//!    `description`s;
//! 3. implement `Described` (the entry's `CommandSpec`) and `Registrable`, so the crate's
//!    `register` function reads `registry.register::<SplitItem>()?`. Registration needs the
//!    hand-written `impl Command` / `impl Query` / `impl EngineCommand`; forgetting it is a
//!    compile error that says so.
//!
//! The contract is checked at compile time, with the error on the offending token: the name
//! grammar `^[a-z]+\.[a-z_]+$` and the arch §5.1 namespaces, a 1–100 character single-line
//! summary, an explicit `undoable` and `annotations(destructive = …)` on commands, no
//! `undoable`/`destructive`/`idempotent` on queries, `view` exactly on `view.*` commands, no
//! UI-only MCP core tools, and named-field, non-generic structs only. The registry checks the
//! same rules again at start-up for entries written by hand.
//!
//! ```ignore
//! #[command(
//!     name = "clip.split",
//!     summary = "Split an item at a timeline time",
//!     undoable,
//!     mcp = "core",                    // core | catalog | hidden
//!     annotations(destructive = false, idempotent = false)
//! )]
//! pub struct SplitItem {
//!     pub item: ItemId,
//!     pub at: Time,
//! }
//! ```
//!
//! Arguments: `name`, `summary`, `mcp` (all required); `undoable` / `undoable = false`
//! (required on commands); `scope = "any" | "ui"` (`"ui_only"` is an alias; default `any`);
//! `annotations(destructive, idempotent, open_world)` (`destructive` required on commands;
//! queries accept only `open_world`); `view` (a `view.*` command: engine-run, never undoable,
//! never journaled); `engine` (a command run against engine state, e.g. `edit.undo`, via
//! `EngineCommand`). Field attribute: `#[secret]`.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{quote, quote_spanned};
use syn::{
    Fields, ItemStruct, LitBool, LitStr, Type, ext::IdentExt, meta::ParseNestedMeta,
    parse_macro_input, spanned::Spanned,
};

/// The command namespaces of arch §5.1 (mirrors `cutweave_commands::NAMESPACES`; a unit test
/// keeps them equal). New namespaces require an ADR.
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

/// Longest summary (mirrors `cutweave_commands::MAX_SUMMARY_CHARS`).
const MAX_SUMMARY_CHARS: usize = 100;

/// Registers a document command (or, with `engine` / `view`, an engine command). See the crate
/// docs.
#[proc_macro_attribute]
pub fn command(args: TokenStream, input: TokenStream) -> TokenStream {
    expand(Kind::Command, args, input)
}

/// Registers a read-only query (`readOnlyHint`, idempotent, never undoable). See the crate
/// docs.
#[proc_macro_attribute]
pub fn query(args: TokenStream, input: TokenStream) -> TokenStream {
    expand(Kind::Query, args, input)
}

/// Marks a type that crosses the wire (outputs, nested params): adds the same derives as
/// `#[command]` (with a derived `Debug`), and `deny_unknown_fields` on named-field structs.
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
    let derives = wire_derives(&quote!(::core::fmt::Debug,));
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

/// The derives every wire type gets. Helper attributes must follow the derive.
fn wire_derives(extra: &TokenStream2) -> TokenStream2 {
    quote! {
        #[derive(
            #extra
            ::core::clone::Clone,
            ::core::cmp::PartialEq,
            ::cutweave_commands::__private::serde::Serialize,
            ::cutweave_commands::__private::serde::Deserialize,
            ::cutweave_commands::__private::schemars::JsonSchema,
            ::cutweave_commands::__private::specta::Type
        )]
        #[serde(crate = "::cutweave_commands::__private::serde")]
        #[schemars(crate = "::cutweave_commands::__private::schemars")]
        #[specta(crate = ::cutweave_commands::__private::specta)]
    }
}

/// Parsed attribute arguments (each with its span, for errors).
#[derive(Default)]
struct Args {
    name: Option<LitStr>,
    summary: Option<LitStr>,
    mcp: Option<LitStr>,
    scope: Option<LitStr>,
    undoable: Option<LitBool>,
    view: Option<Span>,
    engine: Option<Span>,
    annotations: Option<Span>,
    destructive: Option<LitBool>,
    idempotent: Option<LitBool>,
    open_world: Option<LitBool>,
}

fn flag_value(meta: &ParseNestedMeta) -> syn::Result<LitBool> {
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

fn key_of(meta: &ParseNestedMeta) -> String {
    meta.path
        .get_ident()
        .map_or_else(String::new, ToString::to_string)
}

fn parse_args(kind: Kind, args: TokenStream) -> syn::Result<Args> {
    let mut out = Args::default();
    let parser = syn::meta::parser(|meta| {
        let key = key_of(&meta);
        match (key.as_str(), kind) {
            ("name", _) => set_once(&mut out.name, meta.value()?.parse()?, &meta, "name"),
            ("summary", _) => set_once(&mut out.summary, meta.value()?.parse()?, &meta, "summary"),
            ("mcp", _) => set_once(&mut out.mcp, meta.value()?.parse()?, &meta, "mcp"),
            ("scope", _) => set_once(&mut out.scope, meta.value()?.parse()?, &meta, "scope"),
            ("undoable", Kind::Command) => {
                let v = flag_value(&meta)?;
                set_once(&mut out.undoable, v, &meta, "undoable")
            }
            ("view", Kind::Command) => set_once(&mut out.view, meta.path.span(), &meta, "view"),
            ("engine", Kind::Command) => {
                set_once(&mut out.engine, meta.path.span(), &meta, "engine")
            }
            ("undoable" | "view" | "engine", Kind::Query) => Err(meta.error(format!(
                "queries are read-only: `{key}` only applies to #[command]"
            ))),
            ("annotations", _) => {
                set_once(&mut out.annotations, meta.path.span(), &meta, "annotations")?;
                meta.parse_nested_meta(|inner| parse_annotation(kind, &inner, &mut out))
            }
            _ => Err(meta.error(format!(
                "unknown #[{}] argument `{key}`; expected `name`, `summary`, `mcp`, `scope`, {}`annotations(...)`",
                kind.attr(),
                if kind == Kind::Command {
                    "`undoable`, `view`, `engine`, "
                } else {
                    ""
                }
            ))),
        }
    });
    syn::parse::Parser::parse(parser, args)?;
    Ok(out)
}

fn parse_annotation(kind: Kind, inner: &ParseNestedMeta, out: &mut Args) -> syn::Result<()> {
    let key = key_of(inner);
    match (key.as_str(), kind) {
        ("destructive" | "idempotent", Kind::Query) => Err(inner.error(
            "queries are always non-destructive and idempotent; only `open_world` may be set",
        )),
        ("destructive", _) => {
            let v = flag_value(inner)?;
            set_once(&mut out.destructive, v, inner, "destructive")
        }
        ("idempotent", _) => {
            let v = flag_value(inner)?;
            set_once(&mut out.idempotent, v, inner, "idempotent")
        }
        ("open_world", _) => {
            let v = flag_value(inner)?;
            set_once(&mut out.open_world, v, inner, "open_world")
        }
        ("read_only", _) => {
            Err(inner
                .error("`read_only` is derived: #[query] is read-only and #[command] never is"))
        }
        _ => {
            Err(inner
                .error("unknown annotation; expected `destructive`, `idempotent` or `open_world`"))
        }
    }
}

fn check_name(lit: &LitStr) -> syn::Result<String> {
    let name = lit.value();
    let err = |msg: String| syn::Error::new(lit.span(), msg);
    let Some((namespace, verb)) = name.split_once('.') else {
        return Err(err(
            "command name must be `namespace.verb_object`, e.g. \"clip.split\"".into(),
        ));
    };
    let ns_ok = !namespace.is_empty() && namespace.bytes().all(|b| b.is_ascii_lowercase());
    let verb_ok = !verb.is_empty() && verb.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
    if !ns_ok || !verb_ok {
        return Err(err(
            "command name must match `^[a-z]+\\.[a-z_]+$` (lowercase `namespace.verb_object`, no digits)"
                .into(),
        ));
    }
    if !NAMESPACES.contains(&namespace) {
        return Err(err(format!(
            "unknown command namespace `{namespace}`; new namespaces require an ADR (arch §5.1). Known: {}",
            NAMESPACES.join(", ")
        )));
    }
    Ok(namespace.to_owned())
}

fn check_summary(lit: &LitStr) -> syn::Result<()> {
    let summary = lit.value();
    let chars = summary.chars().count();
    if chars == 0 || chars > MAX_SUMMARY_CHARS || summary.contains(['\n', '\r']) {
        return Err(syn::Error::new(
            lit.span(),
            format!(
                "summary must be a single line of 1..={MAX_SUMMARY_CHARS} characters (it is the MCP \
                 tool description and counts against the tool-list budget); put details in the \
                 doc comment, which becomes the schema description"
            ),
        ));
    }
    Ok(())
}

/// The resolved, validated spec of an entry.
#[allow(clippy::struct_excessive_bools)] // mirrors `CommandSpec`
struct Spec {
    name: LitStr,
    summary: LitStr,
    kind: TokenStream2,
    register: TokenStream2,
    undoable: bool,
    view: bool,
    scope: TokenStream2,
    mcp: TokenStream2,
    read_only: bool,
    destructive: bool,
    idempotent: bool,
    open_world: bool,
}

#[allow(clippy::too_many_lines)] // one linear list of contract rules
fn resolve(kind: Kind, args: Args) -> syn::Result<Spec> {
    let attr = kind.attr();
    let missing =
        |what: &str| syn::Error::new(Span::call_site(), format!("#[{attr}] requires {what}"));
    let name = args
        .name
        .ok_or_else(|| missing("`name = \"namespace.verb_object\"`"))?;
    let namespace = check_name(&name)?;
    let summary = args.summary.ok_or_else(|| missing("`summary = \"...\"`"))?;
    check_summary(&summary)?;
    let mcp_lit = args
        .mcp
        .ok_or_else(|| missing("`mcp = \"core\" | \"catalog\" | \"hidden\"`"))?;
    let mcp = match mcp_lit.value().as_str() {
        "core" => quote!(::cutweave_commands::McpExposure::Core),
        "catalog" => quote!(::cutweave_commands::McpExposure::Catalog),
        "hidden" => quote!(::cutweave_commands::McpExposure::Hidden),
        _ => {
            return Err(syn::Error::new(
                mcp_lit.span(),
                "mcp must be \"core\", \"catalog\" or \"hidden\"",
            ));
        }
    };
    let ui_only = match &args.scope {
        None => false,
        Some(s) => match s.value().as_str() {
            "any" => false,
            "ui" | "ui_only" => true,
            _ => {
                return Err(syn::Error::new(
                    s.span(),
                    "scope must be \"any\" (the default) or \"ui\" (UI credential only, arch §6)",
                ));
            }
        },
    };
    if ui_only && mcp_lit.value() == "core" {
        return Err(syn::Error::new(
            mcp_lit.span(),
            "a `scope = \"ui\"` entry cannot be an MCP core tool: agent credentials can never call \
             it; use `mcp = \"catalog\"` or `mcp = \"hidden\"`",
        ));
    }
    let scope = if ui_only {
        quote!(::cutweave_commands::Scope::UiOnly)
    } else {
        quote!(::cutweave_commands::Scope::Any)
    };
    let open_world = args.open_world.is_some_and(|b| b.value);

    if kind == Kind::Query {
        return Ok(Spec {
            name,
            summary,
            kind: quote!(::cutweave_commands::EntryKind::Query),
            register: quote!(add_query),
            undoable: false,
            view: false,
            scope,
            mcp,
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world,
        });
    }

    let view = args.view.is_some();
    if let (Some(_), Some(engine)) = (args.view, args.engine) {
        return Err(syn::Error::new(
            engine,
            "`view` already makes this an engine command; remove `engine`",
        ));
    }
    let destructive = if let Some(view_span) = args.view {
        if namespace != "view" {
            return Err(syn::Error::new(
                view_span,
                "only `view.*` commands may be marked `view`",
            ));
        }
        if let Some(undoable) = &args.undoable {
            return Err(syn::Error::new(
                undoable.span(),
                "`view` commands are never undoable (or journaled); remove `undoable`",
            ));
        }
        match args.destructive {
            Some(d) if d.value => {
                return Err(syn::Error::new(
                    d.span(),
                    "`view` commands only change UI view state and cannot be destructive",
                ));
            }
            _ => false,
        }
    } else {
        if namespace == "view" {
            return Err(syn::Error::new(
                name.span(),
                "`view.*` commands must be marked `view` (not undoable, not journaled)",
            ));
        }
        args.destructive
            .ok_or_else(|| {
                syn::Error::new(
                    args.annotations.unwrap_or_else(Span::call_site),
                    "#[command] must declare `annotations(destructive = true|false)`: MCP's \
                     destructiveHint defaults to true, so leaving it implicit would mislabel the tool",
                )
            })?
            .value
    };
    let undoable = if view {
        false
    } else {
        args.undoable
            .ok_or_else(|| {
                syn::Error::new(
                    Span::call_site(),
                    "#[command] must state `undoable` or `undoable = false` explicitly",
                )
            })?
            .value
    };
    let engine = view || args.engine.is_some();
    Ok(Spec {
        name,
        summary,
        kind: if engine {
            quote!(::cutweave_commands::EntryKind::Engine)
        } else {
            quote!(::cutweave_commands::EntryKind::Command)
        },
        register: if engine {
            quote!(add_engine_command)
        } else {
            quote!(add_command)
        },
        undoable,
        view,
        scope,
        mcp,
        read_only: false,
        destructive,
        idempotent: args.idempotent.is_some_and(|b| b.value),
        open_world,
    })
}

fn is_option(ty: &Type) -> bool {
    // Syntactic: an alias of `Option` is not recognized (then mark it `#[serde(default)]`).
    match ty {
        Type::Path(p) if p.qself.is_none() => p
            .path
            .segments
            .last()
            .is_some_and(|seg| seg.ident == "Option"),
        _ => false,
    }
}

/// Whether the field already says `#[serde(default)]` / `#[serde(default = "…")]`.
fn has_serde_default(field: &syn::Field) -> syn::Result<bool> {
    let mut found = false;
    for attr in field.attrs.iter().filter(|a| a.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("default") {
                found = true;
            }
            skip_value(&meta)
        })?;
    }
    Ok(found)
}

/// Consumes a nested meta's value (`= expr` or `(...)`), if any.
fn skip_value(meta: &ParseNestedMeta) -> syn::Result<()> {
    if meta.input.peek(syn::Token![=]) {
        let _: syn::Expr = meta.value()?.parse()?;
    } else if meta.input.peek(syn::token::Paren) {
        meta.parse_nested_meta(|inner| skip_value(&inner))?;
    }
    Ok(())
}

/// The JSON name of a field: its `#[serde(rename = "…")]`, or its identifier.
fn json_name(field: &syn::Field) -> syn::Result<String> {
    let mut name = field
        .ident
        .as_ref()
        .map_or_else(String::new, |i| i.unraw().to_string());
    for attr in field.attrs.iter().filter(|a| a.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") && meta.input.peek(syn::Token![=]) {
                name = meta.value()?.parse::<LitStr>()?.value();
                Ok(())
            } else {
                skip_value(&meta)
            }
        })?;
    }
    Ok(name)
}

fn expand(kind: Kind, args: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as syn::Item);
    match expand_item(kind, args, item) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn check_shape(attr: &str, st: &ItemStruct) -> syn::Result<()> {
    if !st.generics.params.is_empty() {
        return Err(syn::Error::new(
            st.generics.span(),
            format!(
                "#[{attr}] structs cannot be generic: each registry entry is one concrete type"
            ),
        ));
    }
    match &st.fields {
        Fields::Named(_) => Ok(()),
        Fields::Unnamed(_) | Fields::Unit => Err(syn::Error::new(
            st.ident.span(),
            format!(
                "#[{attr}] params must be a struct with named fields (write `{{}}` for none): the \
                 JSON Schema root must be an object (MCP requires inputSchema type \"object\")"
            ),
        )),
    }
}

fn expand_item(kind: Kind, args: TokenStream, item: syn::Item) -> syn::Result<TokenStream2> {
    let attr = kind.attr();
    let syn::Item::Struct(mut st) = item else {
        return Err(syn::Error::new(
            item.span(),
            format!("#[{attr}] must be placed on a struct with named fields"),
        ));
    };
    let args = parse_args(kind, args)?;
    check_shape(attr, &st)?;
    let spec = resolve(kind, args)?;

    // `#[secret]` fields; `Option` fields become optional.
    let mut secrets = Vec::new();
    let mut debug_fields = Vec::new();
    if let Fields::Named(fields) = &mut st.fields {
        for field in &mut fields.named {
            let mut secret = false;
            let mut error = None;
            field.attrs.retain(|a| {
                if a.path().is_ident("secret") {
                    if !matches!(a.meta, syn::Meta::Path(_)) {
                        error = Some(syn::Error::new(a.span(), "`#[secret]` takes no arguments"));
                    }
                    secret = true;
                    false
                } else {
                    true
                }
            });
            if let Some(e) = error {
                return Err(e);
            }
            if is_option(&field.ty) && !has_serde_default(field)? {
                field.attrs.push(syn::parse_quote!(#[serde(default)]));
            }
            let ident = field
                .ident
                .clone()
                .ok_or_else(|| syn::Error::new(field.span(), "expected a named field"))?;
            let label = ident.unraw().to_string();
            if secret {
                secrets.push(json_name(field)?);
                debug_fields.push(quote!(.field(#label, &::cutweave_commands::REDACTED)));
            } else {
                debug_fields.push(quote!(.field(#label, &self.#ident)));
            }
        }
    }

    let ident = &st.ident;
    let rust_type = ident.to_string();
    let derives = wire_derives(&quote!());
    let Spec {
        name,
        summary,
        kind: kind_ts,
        register,
        undoable,
        view,
        scope,
        mcp,
        read_only,
        destructive,
        idempotent,
        open_world,
    } = spec;
    // Errors about a missing `impl Command` point at the struct name.
    let register = quote_spanned!(ident.span()=> registry.#register::<#ident>());
    Ok(quote! {
        #derives
        #[serde(deny_unknown_fields)]
        #st

        impl ::core::fmt::Debug for #ident {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.debug_struct(#rust_type) #(#debug_fields)* .finish()
            }
        }

        impl ::cutweave_commands::Described for #ident {
            const SPEC: ::cutweave_commands::CommandSpec = ::cutweave_commands::CommandSpec {
                name: #name,
                summary: #summary,
                kind: #kind_ts,
                undoable: #undoable,
                view: #view,
                scope: #scope,
                mcp: #mcp,
                annotations: ::cutweave_commands::Annotations {
                    read_only: #read_only,
                    destructive: #destructive,
                    idempotent: #idempotent,
                    open_world: #open_world,
                },
                secret_fields: &[#(#secrets),*],
                rust_type: #rust_type,
            };
        }

        impl ::cutweave_commands::Registrable for #ident {
            fn register_into(
                registry: &mut ::cutweave_commands::Registry,
            ) -> ::core::result::Result<(), ::cutweave_commands::RegistryError> {
                #register.map(|_| ())
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirrors_the_runtime_contract() {
        assert_eq!(NAMESPACES, cutweave_commands::NAMESPACES);
        assert_eq!(MAX_SUMMARY_CHARS, cutweave_commands::MAX_SUMMARY_CHARS);
    }

    #[test]
    fn name_checks_match_the_registry() {
        for name in [
            "clip.split",
            "edit.begin_tx",
            "Clip.split",
            "clip.split2",
            "clip",
            "widget.spin",
            "view.set_zoom",
            "clip._",
        ] {
            let lit = LitStr::new(name, Span::call_site());
            assert_eq!(
                check_name(&lit).is_ok(),
                cutweave_commands::check_name(name).is_ok(),
                "{name}"
            );
        }
    }
}
