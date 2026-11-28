// SPDX-License-Identifier: GPL-3.0-or-later
//! A count of the expensive step of a document edit, for regression tests that must not depend
//! on how busy the machine is (issues #230, #231): every conversion of a part of the document
//! (an item, a track's own fields, a sequence's, a value of an op) to or from its JSON form.
//! An edit that touches 2,000 items should convert a handful of values, not thousands.
//!
//! Counted per thread, so tests running side by side do not see each other's work. Not part
//! of the API.

use std::cell::Cell;

thread_local! {
    static CONVERSIONS: Cell<u64> = const { Cell::new(0) };
}

/// Notes one conversion to or from JSON.
pub(crate) fn converted() {
    CONVERSIONS.with(|count| count.set(count.get() + 1));
}

/// Runs `f` without counting what it converts: the cross-checks of debug builds, which take
/// the general path next to a shortcut to compare the two.
#[cfg(debug_assertions)]
pub(crate) fn uncounted<R>(f: impl FnOnce() -> R) -> R {
    let before = json_conversions();
    let out = f();
    CONVERSIONS.with(|count| count.set(before));
    out
}

/// The conversions this thread has made so far; tests take the difference around an edit.
#[must_use]
pub fn json_conversions() -> u64 {
    CONVERSIONS.with(Cell::get)
}
