//! trybuild: malformed `#[command]` / `#[query]` uses fail to compile with a helpful message.
//! Expected diagnostics live next to each case in `tests/ui/*.stderr` (regenerate with
//! `TRYBUILD=overwrite cargo test -p s5-ops --test compile_fail`, then review the diff).

#[test]
fn macro_misuse_is_a_compile_error() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
    t.pass("tests/ui-pass/*.rs");
}
