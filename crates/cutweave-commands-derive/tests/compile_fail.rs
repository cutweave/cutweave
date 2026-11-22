// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.02: malformed `#[command]` / `#[query]` attributes fail to compile with a helpful
//! diagnostic on the offending token. After an intended change to the diagnostics, review and
//! regenerate the `.stderr` files with
//! `TRYBUILD=overwrite cargo test -p cutweave-commands-derive --test compile_fail`.

#[test]
fn malformed_attributes_are_rejected() {
    trybuild::TestCases::new().compile_fail("tests/ui/*.rs");
}
