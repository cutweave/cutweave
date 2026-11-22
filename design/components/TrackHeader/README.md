TrackHeader is the left-hand label cell of a timeline track: type icon, name, lock/mute/hide/solo toggles and the magnetic main-track marker.

**When to use**
- Beside every track lane in the timeline.
- Collapsed (`track-sm`) or expanded (`track-lg`) variants use the same anatomy.
- Not for panel headers (use Panel).

**Anatomy**
`.cw-track` (`track-md` tall, `surface-1`, `border-subtle` bottom rule, `space-2` padding and gaps) > `__icon` (`icon-md`, colored by `--clip-color`) > `__name` (type: label, ellipsis) > optional `__main` marker (magnet icon + "Magnetic", `marigold-text`) > `__toggles` (IconButton `--sm`: lock, mute or hide, solo). `data-main="true"` adds a 2px marigold edge on the main track.

**The consumer provides**
Track id and name, track kind (sets the icon and `--clip-color` to the matching `clip-<type>` token), main-track flag, and the lock/mute/hide/solo booleans.

**States**
Default; toggles pressed (`aria-pressed="true"`: `marigold-subtle` fill, `marigold-text` icon); locked tracks show the lock pressed and the lane dims; hover on buttons (`surface-3`); focus-visible; disabled track name uses `text-disabled`.

**Keyboard & accessibility**
Each toggle is a button with `aria-pressed` and a label naming the track ("Mute track A1"). Solo uses the letter S plus its label. Magnetic state is text plus icon. Rows use roving focus in the track list.

**Command mapping**
`track.setLocked`, `track.setMuted`, `track.setHidden`, `track.setSolo {trackId, value}`; the main-track magnet maps to `track.setMagnetic {trackId}`. All are MCP-reachable.

**Do**
- Color only the type icon with `clip-<type>`; keep the name `text-primary`.
- Keep toggles `control-sm` with a `hit-min` target.
- Show the magnetic marker as icon + word, never icon alone.

**Don't**
- Don't use marigold for lock or mute state beyond the pressed fill.
- Don't put more than four toggles in a header; overflow into a menu.
- Don't use agent blue for tracks created by an agent; add an ActorBadge in history instead.
