TimelineClip is a block on a track lane, colored by clip type, that can be selected, trimmed, faded, keyframed or shown as an agent proposal.

**When to use**
- Every item on a timeline track: media, audio, graphics, captions, effects.
- Ghost clips that an agent proposes before the user accepts them.
- Not for bin assets (use AssetTile).

**Anatomy**
`.cw-clip` + type modifier (`--media`, `--audio`, `--graphics`, `--captions`, `--effects`) > `__header` (3px `clip-<type>` strip, then a `space-0-5` gap) > `__label` (type: caption, `icon-sm` type icon in `clip-<type>`) > `__body` (`clip-<type>-body`) holding `__wave` (audio stroke in `clip-audio`), `__trim` start/end handles, `__fade` corner handle, `__fx` badge (`clip-effects`) and `__kf` keyframe ticks. 1px `clip-<type>` border, `radius-sm`, height `track-md`.

**The consumer provides**
Clip type, label, start and width (`--x`, `--w`), waveform path for audio, effect and keyframe data, and selection/disabled/proposed flags.

**States**
Default; hover and selected reveal trim and fade handles; selected (`aria-selected="true"`: 2px `marigold` outline); proposed (`data-state="proposed"`: dashed `agent` border, `agent-subtle` fill, diagonal hatch); disabled (`aria-disabled="true"`, 50% opacity); focus-visible uses `focus-ring`.

**Keyboard & accessibility**
`role="option"` in a track listbox, `tabindex` roving; arrows move selection, Shift extends, `[`/`]` trim, Alt+Left/Right nudge. Label is text; type is also conveyed by icon, not color alone. Proposed clips announce "proposed by agent".

**Command mapping**
Split -> `clip.split`, trim -> `clip.trim {edge, time}`, move -> `clip.move`, fade -> `clip.setFade`. Proposed clips are part of an open `proposal` transaction; Accept/Reject on ProposalBar commits or discards them.

**Do**
- Keep `text-primary` on every `clip-*-body`.
- Use `agent` only for proposed ghosts and never for real clips.
- Animate nothing while playback runs.

**Don't**
- Don't tint the body with marigold; selection is the outline only.
- Don't use effect color for the badge on non-effect text.
- Don't shrink labels below caption size; hide them when the clip is too narrow.
