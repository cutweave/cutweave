// SPDX-License-Identifier: GPL-3.0-or-later
//! `item.apply_layout` and the query `item.layouts` (WP-2C.08; arch §8): place two to four
//! clips in the cells of a layout (side by side, top and bottom, picture in picture, columns,
//! one large and two small, a grid) in one undoable step.
//!
//! The result is plain `VisualProps`: position, scale, rotation, anchor and the four crop edges,
//! written exactly as `item.set_props` writes them (an animated property gets a key at `at`).
//! The layouts are data (`data/layouts.json`), so packs can add more later.
//!
//! # Cell maths
//!
//! Work in sequence pixels on the canvas `W x H`. A clip's whole picture has aspect `a` (the
//! media's upright picture; the canvas aspect without one). Its cell is the rectangle `c`.
//!
//! * `crop`: the picture is scaled to cover the cell (`Wd x Hd`, the larger of the two
//!   scalings), centred on it, and the overhang is cropped off the two opposite edges. Crop
//!   masks without moving the picture (WP-2C.04), so the position is the cell's centre and the
//!   visible rectangle is exactly the cell.
//! * `fit`: the picture is scaled to sit whole inside the cell, centred, with no crop.
//!
//! In both, `scale = Wd / fit_size(a, canvas).width` (uniform), `position` is the cell centre's
//! offset from the canvas centre in canvas fractions, the anchor is the picture's centre and the
//! rotation is 0.

use std::sync::LazyLock;

use cutweave_commands::{CmdError, Command, Query, QueryCtx, Tx, command, query, wire};
use cutweave_model::{Clip, EntityKind, ItemId, Project, TimeInput, TrackKind, Vec2};
use serde::Deserialize;

use super::{
    CropPatch, ItemChange, ItemProps, ItemSetProps, KeyedProp, SetPlan, TransformPatch,
};
use crate::{
    keyframe::{self, Target},
    sequence::reframe::{self, Canvas},
    timeline::edit::given_items,
};

/// The layout table's source, embedded at build time.
const TABLE: &str = include_str!("../../data/layouts.json");

/// One cell of a layout.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum CellDef {
    /// A cell on the layout's columns and rows.
    Grid {
        col: usize,
        row: usize,
        #[serde(default = "one")]
        col_span: usize,
        #[serde(default = "one")]
        row_span: usize,
    },
    /// A cell in fractions of the canvas (ignores the gap).
    Rect { x: f64, y: f64, w: f64, h: f64 },
}

fn one() -> usize {
    1
}

/// One layout of the table.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LayoutDef {
    id: String,
    label: String,
    #[serde(default = "one_weight")]
    columns: Vec<f64>,
    #[serde(default = "one_weight")]
    rows: Vec<f64>,
    cells: Vec<CellDef>,
}

fn one_weight() -> Vec<f64> {
    vec![1.0]
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Table {
    #[allow(dead_code)]
    version: u32,
    #[allow(dead_code)]
    note: Option<String>,
    layouts: Vec<LayoutDef>,
}

/// The parsed table. A malformed table is a build defect, caught by the unit tests.
static LAYOUTS: LazyLock<Vec<LayoutDef>> = LazyLock::new(|| {
    serde_json::from_str::<Table>(TABLE)
        .map(|t| t.layouts)
        .unwrap_or_default()
});

fn find(id: &str) -> Result<&'static LayoutDef, CmdError> {
    LAYOUTS.iter().find(|l| l.id == id).ok_or_else(|| {
        let ids: Vec<&str> = LAYOUTS.iter().map(|l| l.id.as_str()).collect();
        CmdError::invalid(
            "/layout",
            format!("unknown layout `{id}`; use one of {}", ids.join(", ")),
        )
    })
}

/// How a picture meets its cell.
#[wire]
#[derive(Copy, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LayoutFill {
    /// Fill the cell: the picture covers it and a centred crop removes the overhang.
    #[default]
    Crop,
    /// Fit the picture whole inside the cell (bars where the shapes differ).
    Fit,
}

/// A rectangle in sequence pixels.
#[wire]
pub struct CellRect {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

/// The cell of one item after the layout.
#[wire]
pub struct PlacedCell {
    /// The item.
    pub item: ItemId,
    /// Its cell, in sequence pixels.
    pub cell: CellRect,
}

/// Result of `item.apply_layout`.
#[wire]
pub struct ItemApplyLayoutOutput {
    /// The layout that was applied.
    pub layout: String,
    /// Where each item went, in cell order.
    pub cells: Vec<PlacedCell>,
    /// The properties whose value changed on any item.
    pub changed: Vec<String>,
    /// The keys written on animated properties.
    pub keyed: Vec<KeyedProp>,
}

/// Places 2 to 4 clips in the cells of a layout, in one undo step. Example:
/// `{items:[a,b], layout:"side_by_side"}`.
///
/// `layout` is one of `item.layouts`. Only video clips count (the audio of a linked clip is
/// ignored); their number must be what the layout takes, else `Invalid`. `order` lists the clips
/// for the cells (without it the clip on the top track comes first, ties by start time).
/// `fill` is `crop` (the default: every cell is filled, the overhang cropped) or `fit` (the
/// whole picture inside the cell). `gap` is the space between cells in sequence pixels (grid
/// layouts only). The result is ordinary properties (transform and crop) that can be adjusted
/// afterwards; animated ones are keyed at `at`.
#[command(
    name = "item.apply_layout",
    summary = "Arrange 2-4 clips in a layout: side by side, top and bottom, picture in picture, columns, grid",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ItemApplyLayout {
    /// The clips to arrange.
    pub items: Vec<ItemId>,
    /// A layout ID from `item.layouts`.
    pub layout: String,
    /// `crop` (default) or `fit`.
    pub fill: Option<LayoutFill>,
    /// Space between cells, in sequence pixels (default 0).
    pub gap: Option<f64>,
    /// The clips in cell order (default: top track first).
    pub order: Option<Vec<ItemId>>,
    /// Sequence time (the playhead) to key animated properties at.
    pub at: Option<TimeInput>,
}

/// A layout, as `item.layouts` lists it.
#[wire]
pub struct LayoutInfo {
    /// The ID to give `item.apply_layout`.
    pub id: String,
    /// English name.
    pub label: String,
    /// How many clips the layout takes.
    pub count: u32,
    /// The cells in fractions of the canvas at gap 0, in cell order (for diagrams).
    pub cells: Vec<CellFraction>,
}

/// A cell in fractions of the canvas.
#[wire]
pub struct CellFraction {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

/// Result of `item.layouts`.
#[wire]
pub struct ItemLayoutsOutput {
    /// The layouts, in table order.
    pub layouts: Vec<LayoutInfo>,
}

/// Lists the layouts `item.apply_layout` knows, with how many clips each takes.
#[query(
    name = "item.layouts",
    summary = "List the layouts item.apply_layout offers",
    mcp = "catalog"
)]
pub struct ItemLayouts {
    /// Only the layouts for this many clips.
    pub count: Option<u32>,
}

impl Query for ItemLayouts {
    type Output = ItemLayoutsOutput;

    fn run(&self, _ctx: &QueryCtx<'_>) -> Result<ItemLayoutsOutput, CmdError> {
        let canvas = Canvas {
            w: 1.0,
            h: 1.0,
        };
        let layouts = LAYOUTS
            .iter()
            .filter(|l| {
                self.count
                    .is_none_or(|c| usize::try_from(c).is_ok_and(|c| c == l.cells.len()))
            })
            .map(|l| LayoutInfo {
                id: l.id.clone(),
                label: l.label.clone(),
                count: u32::try_from(l.cells.len()).unwrap_or(u32::MAX),
                cells: cells_of(l, canvas, 0.0)
                    .into_iter()
                    .map(|r| CellFraction {
                        x: r[0],
                        y: r[1],
                        w: r[2] - r[0],
                        h: r[3] - r[1],
                    })
                    .collect(),
            })
            .collect();
        Ok(ItemLayoutsOutput { layouts })
    }
}

/// The cells of a layout on a canvas, as `[left, top, right, bottom]`.
fn cells_of(layout: &LayoutDef, canvas: Canvas, gap: f64) -> Vec<[f64; 4]> {
    let tracks = |weights: &[f64], total: f64| -> Vec<(f64, f64)> {
        let n = weights.len();
        let avail = total - gap * (n.saturating_sub(1)) as f64;
        let sum: f64 = weights.iter().sum();
        let mut at = 0.0;
        weights
            .iter()
            .map(|w| {
                let size = w / sum * avail;
                let span = (at, at + size);
                at += size + gap;
                span
            })
            .collect()
    };
    let cols = tracks(&layout.columns, canvas.w);
    let rows = tracks(&layout.rows, canvas.h);
    layout
        .cells
        .iter()
        .map(|cell| match cell {
            CellDef::Grid {
                col,
                row,
                col_span,
                row_span,
            } => {
                let last = |spans: &[(f64, f64)], i: usize, n: usize| {
                    spans[(i + n.max(1) - 1).min(spans.len() - 1)].1
                };
                let (c, r) = (
                    (*col).min(cols.len() - 1),
                    (*row).min(rows.len() - 1),
                );
                [
                    cols[c].0,
                    rows[r].0,
                    last(&cols, c, *col_span),
                    last(&rows, r, *row_span),
                ]
            }
            CellDef::Rect { x, y, w, h } => [
                x * canvas.w,
                y * canvas.h,
                (x + w) * canvas.w,
                (y + h) * canvas.h,
            ],
        })
        .collect()
}

/// Rounds away float noise so stored values are tidy and a repeat is byte-identical.
fn tidy(v: f64) -> f64 {
    let r = (v * 1e9).round() / 1e9;
    if r == 0.0 { 0.0 } else { r }
}

/// The properties that put a picture of aspect `a` into `cell` on `canvas`: transform and crop.
fn place(a: f64, canvas: Canvas, cell: [f64; 4], fill: LayoutFill) -> ItemProps {
    let (cw, ch) = (cell[2] - cell[0], cell[3] - cell[1]);
    let (dw, crop_x, crop_y) = match fill {
        LayoutFill::Fit => {
            let (w, _) = reframe::fit_size(a, Canvas { w: cw, h: ch });
            (w, 0.0, 0.0)
        }
        LayoutFill::Crop => {
            // The picture covers the cell; the overhang is cropped off both opposite edges.
            let (w, h) = if a >= cw / ch {
                (ch * a, ch)
            } else {
                (cw, cw / a)
            };
            (w, (w - cw) / (2.0 * w), (h - ch) / (2.0 * h))
        }
    };
    let fit_w = reframe::fit_size(a, canvas).0;
    let s = dw / fit_w;
    let centre = ((cell[0] + cell[2]) / 2.0, (cell[1] + cell[3]) / 2.0);
    let (cx, cy) = (tidy(crop_x.clamp(0.0, 1.0)), tidy(crop_y.clamp(0.0, 1.0)));
    ItemProps {
        name: None,
        enabled: None,
        opacity: None,
        audio_gain: None,
        audio_pan: None,
        transform: Some(TransformPatch {
            position: Some(Vec2::new(
                tidy((centre.0 - canvas.w / 2.0) / canvas.w),
                tidy((centre.1 - canvas.h / 2.0) / canvas.h),
            )),
            scale: Some(Vec2::new(tidy(s), tidy(s))),
            rotation: Some(0.0),
            anchor: Some(Vec2::new(0.5, 0.5)),
        }),
        crop: Some(CropPatch {
            left: Some(cx),
            top: Some(cy),
            right: Some(cx),
            bottom: Some(cy),
        }),
        flip: None,
        blend_mode: None,
    }
}

/// One item's step of a validated plan.
struct Step {
    target: Target,
    request: ItemSetProps,
    plan: SetPlan,
    cell: [f64; 4],
}

impl ItemApplyLayout {
    /// The video clips among `items`, as `(id, top-first sort key)`.
    fn visual(doc: &Project, items: &[ItemId]) -> Result<Vec<ItemId>, CmdError> {
        let (sequence, ids) = given_items(doc, items)?;
        let seq = doc
            .sequences
            .get(&sequence)
            .ok_or_else(|| CmdError::not_found(EntityKind::Sequence, sequence))?;
        let mut visual: Vec<(usize, i64, ItemId)> = Vec::new();
        for id in ids {
            let Some((track, item)) = seq.find_item(&id) else {
                continue;
            };
            if track.kind != TrackKind::Video || item.as_clip().is_none() {
                continue;
            }
            let row = seq
                .tracks_in_order()
                .position(|t| t.id == track.id)
                .unwrap_or(usize::MAX);
            visual.push((row, item.start().flicks(), id));
        }
        visual.sort();
        Ok(visual.into_iter().map(|(_, _, id)| id).collect())
    }

    fn steps(&self, doc: &Project) -> Result<(&'static LayoutDef, Vec<Step>), CmdError> {
        let layout = find(&self.layout)?;
        let gap = self.gap.unwrap_or(0.0);
        if !gap.is_finite() || gap < 0.0 {
            return Err(CmdError::invalid("/gap", "the gap must be a number >= 0"));
        }
        let mut ids = Self::visual(doc, &self.items)?;
        let want = layout.cells.len();
        if ids.len() != want {
            return Err(CmdError::invalid(
                "/items",
                format!(
                    "`{}` takes {want} video clips, but {} were given",
                    layout.id,
                    ids.len()
                ),
            ));
        }
        if let Some(order) = &self.order {
            let mut ordered = Vec::with_capacity(order.len());
            for (i, id) in order.iter().enumerate() {
                if !ids.contains(id) || ordered.contains(id) {
                    return Err(CmdError::invalid(
                        format!("/order/{i}"),
                        "`order` must list each of the video clips in `items` once",
                    ));
                }
                ordered.push(*id);
            }
            if ordered.len() != ids.len() {
                return Err(CmdError::invalid(
                    "/order",
                    "`order` must list each of the video clips in `items` once",
                ));
            }
            ids = ordered;
        }
        let (sequence, _) = given_items(doc, &ids)?;
        let seq = doc
            .sequences
            .get(&sequence)
            .ok_or_else(|| CmdError::not_found(EntityKind::Sequence, sequence))?;
        let canvas = Canvas::new(seq.format.width, seq.format.height);
        let cells = cells_of(layout, canvas, gap);
        let fill = self.fill.unwrap_or_default();
        let mut steps = Vec::with_capacity(ids.len());
        for (id, cell) in ids.into_iter().zip(cells) {
            if cell[2] - cell[0] < 1.0 || cell[3] - cell[1] < 1.0 {
                return Err(CmdError::invalid(
                    "/gap",
                    "the gap leaves a cell smaller than one pixel",
                ));
            }
            let clip = doc
                .find_item(&id)
                .and_then(|at| at.item.as_clip())
                .ok_or_else(|| CmdError::unsupported("only clips can be arranged"))?;
            let a = aspect_of(doc, clip, canvas);
            let request = ItemSetProps {
                item: Some(id),
                items: None,
                props: place(a, canvas, cell, fill),
                auto_key: Some(true),
                at: self.at,
            };
            let (plan, mut targets) = request.plan(doc)?;
            steps.push(Step {
                target: targets.remove(0),
                request,
                plan,
                cell,
            });
        }
        Ok((layout, steps))
    }
}

fn aspect_of(doc: &Project, clip: &Clip, canvas: Canvas) -> f64 {
    doc.media
        .get(&clip.media)
        .and_then(reframe::picture_aspect)
        .unwrap_or(canvas.w / canvas.h)
}

impl Command for ItemApplyLayout {
    type Output = ItemApplyLayoutOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        let (_, steps) = self.steps(ctx.doc())?;
        for step in steps {
            let at = ctx.item(step.target.id)?;
            let mut clip = at
                .item
                .as_clip()
                .ok_or_else(|| CmdError::unsupported("only clips can be arranged"))?
                .clone();
            step.request
                .update_clip(&mut clip, &step.plan, &step.target)?;
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ItemApplyLayoutOutput, CmdError> {
        let (layout, steps) = self.steps(tx.doc())?;
        let mut changed: Vec<String> = Vec::new();
        let mut keyed = Vec::new();
        let mut cells = Vec::with_capacity(steps.len());
        for step in &steps {
            let done: ItemChange = tx.update_item(step.target.id, |item| {
                let Some(clip) = item.as_clip_mut() else {
                    return Err(CmdError::unsupported("only clips can be arranged"));
                };
                step.request
                    .update_clip(clip, &step.plan, &step.target)
            })?;
            for c in done.changed {
                if !changed.contains(&c) {
                    changed.push(c);
                }
            }
            keyed.extend(done.keyed.into_iter().map(|(prop, t)| KeyedProp {
                item: step.target.id,
                prop,
                t,
                at: keyframe::sequence_time(&step.target, t),
            }));
            cells.push(PlacedCell {
                item: step.target.id,
                cell: CellRect {
                    x: tidy(step.cell[0]),
                    y: tidy(step.cell[1]),
                    w: tidy(step.cell[2] - step.cell[0]),
                    h: tidy(step.cell[3] - step.cell[1]),
                },
            });
        }
        Ok(ItemApplyLayoutOutput {
            layout: layout.id.clone(),
            cells,
            changed,
            keyed,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn the_table_parses_and_every_layout_is_well_formed() {
        let table: Table = serde_json::from_str(TABLE).expect("layouts.json parses");
        assert_eq!(table.version, 1);
        assert!(!table.layouts.is_empty());
        let mut seen = std::collections::BTreeSet::new();
        for l in &table.layouts {
            assert!(seen.insert(l.id.clone()), "duplicate id {}", l.id);
            assert!((2..=4).contains(&l.cells.len()), "{}", l.id);
            assert!(l.columns.iter().chain(&l.rows).all(|w| *w > 0.0));
            for c in &l.cells {
                match c {
                    CellDef::Grid {
                        col,
                        row,
                        col_span,
                        row_span,
                    } => {
                        assert!(col + col_span <= l.columns.len(), "{}", l.id);
                        assert!(row + row_span <= l.rows.len(), "{}", l.id);
                    }
                    CellDef::Rect { x, y, w, h } => {
                        assert!(*x >= 0.0 && *y >= 0.0 && x + w <= 1.0 && y + h <= 1.0);
                    }
                }
            }
        }
        assert_eq!(LAYOUTS.len(), table.layouts.len());
    }

    #[test]
    fn cells_do_not_overlap_in_grids_and_keep_the_gap() {
        let canvas = Canvas::new(1920, 1080);
        let l = find("grid_2x2").unwrap();
        let c = cells_of(l, canvas, 20.0);
        assert_eq!(c[0], [0.0, 0.0, 950.0, 530.0]);
        assert_eq!(c[3], [970.0, 550.0, 1920.0, 1080.0]);
        let one_two = cells_of(find("one_large_two_small").unwrap(), canvas, 0.0);
        assert_eq!(one_two[0], [0.0, 0.0, 1280.0, 1080.0]);
        assert_eq!(one_two[2], [1280.0, 540.0, 1920.0, 1080.0]);
    }
}
