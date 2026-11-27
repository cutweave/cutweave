// SPDX-License-Identifier: GPL-3.0-or-later
//! `item.set_label_color` (issue #222; arch §4.2 `Item.color_label`; the clip menu's Color
//! label): marks items with a display colour, or clears it. The label changes nothing in the
//! picture or the sound.

use cutweave_commands::{CmdError, Command, QueryCtx, Tx, command, wire};
use cutweave_model::{Color, EntityKind, ItemId, Project};

use super::MAX_DELETE_ITEMS;

/// Result of `item.set_label_color`.
#[wire]
pub struct ItemSetLabelColorOutput {
    /// The items whose label changed, in the order given.
    pub items: Vec<ItemId>,
}

/// Sets the colour label of items, or clears it when `color` is left out. Only the listed
/// items change (name linked partners to colour them too). `Locked` on a locked track.
#[command(
    name = "item.set_label_color",
    summary = "Set or clear the colour label of items",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ItemSetLabelColor {
    /// The items.
    pub items: Vec<ItemId>,
    /// `#rrggbb`; leave out (or null) to clear the label.
    pub color: Option<Color>,
}

impl ItemSetLabelColor {
    /// The listed items once each, in the order given, that do not have the colour yet.
    fn plan(&self, doc: &Project) -> Result<Vec<ItemId>, CmdError> {
        if self.items.is_empty() {
            return Err(CmdError::invalid("/items", "give at least one item"));
        }
        if self.items.len() > MAX_DELETE_ITEMS {
            return Err(CmdError::invalid(
                "/items",
                format!("at most {MAX_DELETE_ITEMS} items"),
            ));
        }
        let mut out: Vec<ItemId> = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for id in &self.items {
            if !seen.insert(*id) {
                continue;
            }
            let at = doc
                .find_item(id)
                .ok_or_else(|| CmdError::not_found(EntityKind::Item, *id))?;
            if at.item.color_label() == self.color {
                continue;
            }
            if at.track.locked {
                return Err(CmdError::locked(EntityKind::Track, at.track.id));
            }
            out.push(*id);
        }
        Ok(out)
    }
}

impl Command for ItemSetLabelColor {
    type Output = ItemSetLabelColorOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ItemSetLabelColorOutput, CmdError> {
        let items = self.plan(tx.doc())?;
        for id in &items {
            tx.update_item(*id, |item| {
                item.set_color_label(self.color);
                Ok(())
            })?;
        }
        Ok(ItemSetLabelColorOutput { items })
    }
}
