// SPDX-License-Identifier: GPL-3.0-or-later
use cutweave_commands::wire;

#[wire]
pub struct Bad {
    #[ui(widget = "knob")]
    pub a: f64,
    #[ui(minimum = 1)]
    pub b: f64,
    #[ui(min = "one")]
    pub c: f64,
}

fn main() {}
