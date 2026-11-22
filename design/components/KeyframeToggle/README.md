KeyframeToggle is the diamond button that adds or removes a keyframe for a property at the playhead.

**When to use**
- At the right edge of any animatable inspector row (Slider, NumberScrubber).
- Together with previous/next keyframe arrows when a property is animated.
- Not as a general marker or bookmark control.

**Anatomy**
`.cw-kf` (`control-sm` square, `hit-min` safe) > diamond `.cw-icon`. `data-state`: `off` (outlined, `text-muted`), `on` (filled, `marigold-text`; a keyframe exists exactly at the playhead), `elsewhere` (outlined `marigold-text`; keyframes exist at other times). `.cw-kfgroup` wraps prev/next IconButtons (`--sm`) around it.

**The consumer provides**
The property id, keyframe state computed from the playhead time, and `aria-label` text matching the state ("Add keyframe", "Remove keyframe at playhead").

**States**
Off, on at playhead, has-keyframes-elsewhere, hover (`surface-3`), focus-visible, disabled (`text-disabled`, property not animatable).

**Keyboard & accessibility**
Native button with `aria-pressed` reflecting "keyframe at playhead". Space/Enter toggles. Prev/next arrows have shortcuts in their labels. Filled vs outlined shape carries the state, not color alone.

**Command mapping**
Toggle dispatches `clip.keyframe.toggle {clipId, prop, time}`; arrows dispatch `playhead.seekToKeyframe {direction}`. Agents reach the same commands over MCP.

**Do**
- Use `marigold-text` for the icon so contrast holds on `surface-1` in both themes.
- Keep the diamond in the same column on every row so the slot never shifts.
- Disable instead of hiding when a property cannot animate.

**Don't**
- Don't use a circle or other glyph; the diamond is the keyframe symbol everywhere (also in TimelineClip ticks).
- Don't show the filled state when the keyframe is merely nearby.
- Don't use agent blue for keyframes set by an agent; show provenance in history via ActorBadge.
