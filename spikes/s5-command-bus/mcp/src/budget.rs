// SPDX-License-Identifier: GPL-3.0-or-later
//! `tools/list` size measurement against the arch §14 budgets.
//!
//! ## Token heuristic
//!
//! No tokenizer ships offline for Claude, so sizes are estimated (tests must not use the
//! network):
//!
//! - **estimate** = `ceil(bytes / 4)`: the common rule of thumb (and what WP-0H.01 used). For
//!   English prose it is close; for minified JSON Schema it tends to *under*-count, because
//!   punctuation (`{"`, `":"`, `"],"`), short keys and regex patterns (`^itm_[0-9a-f]{32}$`) are
//!   dense in tokens.
//! - **upper** = `ceil(bytes / 3)`: a deliberately pessimistic bound for JSON-heavy text.
//!
//! **Budgets are asserted against `upper`**, so the CI gate keeps ~33% headroom over the rule of
//! thumb. Measured bytes are the compact JSON of the `tools` array exactly as `tools/list`
//! returns it (name, description, inputSchema, outputSchema, annotations). The "model-visible"
//! figure counts only name + description + inputSchema, which is what clients typically turn
//! into model tool definitions. Calibrate against a real client in WP-0H.03 (Claude Code's
//! `/context` shows MCP tool tokens).

use rmcp::model::Tool;
use serde_json::json;

use crate::catalog::{Catalog, Profile};

/// `ceil(bytes / 4)`.
#[must_use]
pub fn estimate_tokens(bytes: usize) -> usize {
    bytes.div_ceil(4)
}

/// `ceil(bytes / 3)`.
#[must_use]
pub fn upper_tokens(bytes: usize) -> usize {
    bytes.div_ceil(3)
}

/// The measured size of one profile's `tools/list`.
#[derive(Clone, Debug)]
pub struct ListSize {
    /// Profile.
    pub profile: Profile,
    /// Listed tools.
    pub tools: usize,
    /// Compact JSON bytes of the `tools` array.
    pub bytes: usize,
    /// Bytes of name + description + inputSchema only.
    pub model_bytes: usize,
    /// Bytes of all `outputSchema`s.
    pub output_schema_bytes: usize,
}

impl ListSize {
    /// `estimate_tokens(bytes)`.
    #[must_use]
    pub fn estimate(&self) -> usize {
        estimate_tokens(self.bytes)
    }

    /// `upper_tokens(bytes)`: the figure budgets are checked against.
    #[must_use]
    pub fn upper(&self) -> usize {
        upper_tokens(self.bytes)
    }

    /// Markdown table row.
    #[must_use]
    pub fn row(&self) -> String {
        let budget = self.profile.budget();
        format!(
            "| {} | {} / {} | {} | {} | {} / {} | {} | {} |",
            self.profile.as_str(),
            self.tools,
            budget.max_tools,
            self.bytes,
            self.estimate(),
            self.upper(),
            budget.max_tokens,
            estimate_tokens(self.model_bytes),
            estimate_tokens(self.output_schema_bytes),
        )
    }
}

/// Markdown table header for [`ListSize::row`].
pub const TABLE_HEADER: &str = "| profile | tools / max | bytes | tokens ~bytes/4 | tokens upper (bytes/3) / max | model-visible ~tokens | outputSchema ~tokens |\n|---|---|---|---|---|---|---|";

fn compact_len(value: &serde_json::Value) -> usize {
    value.to_string().len()
}

/// Measures `catalog`'s `tools/list`.
#[must_use]
pub fn measure(catalog: &Catalog) -> ListSize {
    let tools: &[Tool] = catalog.tools();
    let bytes = compact_len(&json!(tools));
    let model_bytes = tools
        .iter()
        .map(|t| {
            compact_len(&json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.input_schema,
            }))
        })
        .sum();
    let output_schema_bytes = tools
        .iter()
        .filter_map(|t| t.output_schema.as_ref())
        .map(|s| compact_len(&json!(s)))
        .sum();
    ListSize {
        profile: catalog.profile(),
        tools: tools.len(),
        bytes,
        model_bytes,
        output_schema_bytes,
    }
}
