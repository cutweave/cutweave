// SPDX-License-Identifier: GPL-3.0-or-later
// Test support (not built): the recorded transcripts of WP-2A.08. Each replays an event stream
// against the standard sequence of `world.ts` (25 fps, 4 px per frame, header 100 px):
//
//     L1#  [K 0-50]                                   locked
//     V3                                              empty
//     V2   [X 0-20]    [Y 40-80]    [Z 100-120]       X is a still; Y has keys at 50, 70
//     V1*  [A 0-30][B 30-60][C 60-90]                 the main track
//     A1   [a 0-30][b 30-60][c 60-90]                 linked audio of A, B, C
//     A2   [M 0-70]                                   music
//
// with a marker at 130 and the playhead at 150. Snap distance 8 px is 2 frames.
import type { StateName } from "../state.ts";
import type { Tool } from "../types.ts";
import {
  accepted,
  action,
  blur,
  by,
  clip,
  down,
  drag,
  engineCancelled,
  env,
  frame,
  frames,
  header,
  key,
  keyUp,
  lane,
  leave,
  mediaDrop,
  mediaLeave,
  mediaOver,
  move,
  onto,
  type Pos,
  point,
  pointerCancel,
  px,
  refused,
  ruler,
  type Step,
  send,
  type Transcript,
  up,
} from "./transcript.ts";
import { AUDIO, f, IMAGE, MODEL, VIDEO, without } from "./world.ts";

const sel = (...ids: string[]): string => `select [${ids.join(",")}]`;

// ---- selection ------------------------------------------------------------------------------

const selection: Transcript[] = [
  {
    name: "a click on a clip selects its linked group",
    steps: [down(clip("B")), up(clip("B"))],
    expect: [sel("B", "b")],
    end: "hover",
  },
  {
    name: "Alt+click selects only the clip",
    steps: [down(clip("B"), { alt: true }), up(clip("B"), { alt: true })],
    expect: [sel("B")],
    end: "hover",
  },
  {
    name: "Shift+click adds a group, and removes it again",
    steps: [
      down(clip("A")),
      up(clip("A")),
      down(clip("C"), { shift: true }),
      up(clip("C"), { shift: true }),
      down(clip("C"), { shift: true }),
      up(clip("C"), { shift: true }),
    ],
    expect: [sel("A", "a"), sel("A", "a", "C", "c"), sel("A", "a")],
    end: "hover",
  },
  {
    name: "a click in an empty lane clears the selection",
    world: { env: { selection: ["B", "b"] } },
    steps: [down(lane("V3", 50)), up(lane("V3", 50))],
    expect: [sel()],
    end: "hover",
  },
  {
    name: "a marquee selects what it touches with their groups",
    steps: [down(lane("V3", 85)), move(lane("V1", 105)), up(lane("V1", 105))],
    show: ["set_selection", "marquee"],
    expect: ["marquee 440,112 80x112", sel("Z", "C", "c"), "marquee none"],
    end: "hover",
  },
  {
    name: "an Alt marquee selects only the touched clips",
    steps: [
      down(lane("V3", 85), { alt: true }),
      move(lane("V1", 105), { alt: true }),
      up(lane("V1", 105), { alt: true }),
    ],
    expect: [sel("Z", "C")],
    end: "hover",
  },
  {
    name: "a Shift marquee adds to the selection",
    world: { env: { selection: ["A", "a"] } },
    steps: [
      down(lane("V3", 85), { shift: true }),
      move(lane("V1", 105), { shift: true }),
      up(lane("V1", 105), { shift: true }),
    ],
    expect: [sel("A", "a", "Z", "C", "c")],
    end: "hover",
  },
  {
    name: "a click inside a multi-selection keeps it until release, then selects the clip",
    world: { env: { selection: ["A", "a", "B", "b"] } },
    steps: [down(clip("B")), up(clip("B"))],
    expect: [sel("B", "b")],
    end: "hover",
  },
  {
    name: "with linked selection off a click selects only the clip",
    world: { env: { linkedSelection: false } },
    steps: [down(clip("B")), up(clip("B"))],
    expect: [sel("B")],
    end: "hover",
  },
  {
    name: "track headers are the host's: a press there starts nothing, the view keeps its cursor",
    world: { env: { selection: ["B", "b"] } },
    steps: [move(clip("B")), move(header("V2")), down(header("V2")), up(header("V2"))],
    show: ["set_selection", "set_cursor", "begin_tx"],
    expect: ["cursor move", "cursor auto"],
    end: "hover",
  },
  {
    name: "Esc during a marquee clears it and keeps the selection",
    world: { env: { selection: ["A", "a"] } },
    steps: [down(lane("V3", 85)), move(lane("V1", 105)), key("Escape")],
    show: ["set_selection", "marquee"],
    expect: ["marquee 440,112 80x112", "marquee none"],
    end: "idle",
  },
];

// ---- moving on free tracks ------------------------------------------------------------------

const moveZ = (args: string): string => `item.move items=[Z] ${args}`;

const moving: Transcript[] = [
  {
    name: "a clip dragged along a free track moves there (one update, one undo step)",
    steps: drag(clip("Z"), by(clip("Z"), 20)),
    expect: [
      sel("Z"),
      "begin #1 move",
      `update #1 ${moveZ("start=120f mode=new_track")}`,
      "commit #1",
    ],
    history: [moveZ("start=120f mode=new_track")],
    end: "hover",
  },
  {
    name: "a dragged clip snaps to an edge within 8 px and shows the guide",
    steps: drag(clip("Z"), by(clip("Z"), -18)),
    show: ["update_tx", "commit_tx", "snap_guides"],
    expect: [
      "guides [edge@80f]",
      `update #1 ${moveZ("start=80f mode=new_track")}`,
      "commit #1",
      "guides []",
    ],
    end: "hover",
  },
  {
    name: "Ctrl held while dragging: the frame grid only",
    steps: drag(clip("Z"), by(clip("Z"), -18), { mods: { ctrl: true } }),
    show: ["update_tx"],
    expect: [`update #1 ${moveZ("start=82f mode=new_track")}`],
    end: "hover",
  },
  {
    name: "snapping off: the frame grid only",
    world: { env: { snapping: false } },
    steps: drag(clip("Z"), by(clip("Z"), -18)),
    show: ["update_tx"],
    expect: [`update #1 ${moveZ("start=82f mode=new_track")}`],
    end: "hover",
  },
  {
    name: "an overlap on a free track previews and asks for a new track",
    steps: drag(clip("Z"), by(clip("Z"), -50)),
    show: ["update_tx", "preview"],
    expect: [
      "preview V2 101f..121f place",
      "preview V2 50f..70f new_track",
      `update #1 ${moveZ("start=50f mode=new_track")}`,
      "preview none",
    ],
    end: "hover",
  },
  {
    name: "a clip dragged onto another track of its kind changes track",
    steps: drag(clip("Z"), onto(clip("Z"), "V3")),
    show: ["update_tx"],
    expect: [`update #1 ${moveZ("start=100f to_track=V3 mode=new_track")}`],
    history: [moveZ("start=100f to_track=V3 mode=new_track")],
    end: "hover",
  },
  {
    name: "over a track of another kind the clip stays on its own track",
    steps: drag(clip("Z"), onto(by(clip("Z"), 5), "A2")),
    show: ["update_tx"],
    expect: [`update #1 ${moveZ("start=105f mode=new_track")}`],
    end: "hover",
  },
  {
    name: "a locked track is never a target",
    steps: drag(clip("Z"), onto(by(clip("Z"), 5), "L1")),
    show: ["update_tx"],
    expect: [`update #1 ${moveZ("start=105f mode=new_track")}`],
    end: "hover",
  },
  {
    name: "Shift constrains a mostly horizontal drag to time: the track stays",
    steps: [
      down(clip("Z")),
      move(px(clip("Z"), 4)),
      move(px(clip("Z"), 80, -40), { shift: true }),
      frame,
      up(px(clip("Z"), 80, -40), { shift: true }),
    ],
    show: ["update_tx"],
    expect: [`update #1 ${moveZ("start=120f mode=new_track")}`],
    end: "hover",
  },
  {
    name: "Shift constrains a mostly vertical drag to the track: the time stays",
    steps: [
      down(clip("Z")),
      move(px(clip("Z"), 4)),
      move(px(clip("Z"), 12, -56), { shift: true }),
      frame,
      up(px(clip("Z"), 12, -56), { shift: true }),
    ],
    show: ["update_tx"],
    expect: [`update #1 ${moveZ("start=100f to_track=V3 mode=new_track")}`],
    end: "hover",
  },
  {
    name: "a multi-selection moves together, the pressed clip leading",
    world: { env: { selection: ["Y", "Z"] } },
    steps: drag(clip("Z"), by(clip("Z"), 10)),
    expect: [
      "begin #1 move",
      "update #1 item.move items=[Z,Y] start=110f mode=new_track",
      "commit #1",
    ],
    end: "hover",
  },
  {
    name: "a drag past the timeline start stops at 0",
    steps: drag(clip("Z"), by(clip("Z"), -120)),
    show: ["update_tx"],
    expect: [`update #1 ${moveZ("start=0f mode=new_track")}`],
    end: "hover",
  },
  {
    name: "dragged away and back: the clip shows at home again, and the release cancels as unchanged",
    steps: drag(clip("Z"), clip("Z"), { via: [by(clip("Z"), 20)] }),
    show: ["update_tx", "commit_tx", "cancel_tx"],
    expect: [
      `update #1 ${moveZ("start=120f mode=new_track")}`,
      `update #1 ${moveZ("start=100f mode=new_track")}`,
      "cancel #1 unchanged",
    ],
    history: [],
    end: "hover",
  },
  {
    name: "a release before the first frame flushes the position, then commits",
    steps: [
      down(clip("Z")),
      move(px(clip("Z"), 4)),
      move(by(clip("Z"), 20)),
      up(by(clip("Z"), 20)),
    ],
    expect: [
      sel("Z"),
      "begin #1 move",
      `update #1 ${moveZ("start=120f mode=new_track")}`,
      "commit #1",
    ],
    end: "hover",
  },
  {
    name: "a drag that never leaves its place is cancelled as unchanged",
    steps: [down(clip("B")), move(px(clip("B"), 4)), frame, up(px(clip("B"), 4))],
    expect: [sel("B", "b"), "begin #1 move", "cancel #1 unchanged"],
    history: [],
    end: "hover",
  },
];

// ---- the main track (CapCut-style) ----------------------------------------------------------

const mainTrack: Transcript[] = [
  {
    name: "a reorder on the main track lands at the nearest cut",
    steps: drag(clip("B"), by(clip("B"), 40)),
    expect: [
      sel("B", "b"),
      "begin #1 move",
      "update #1 item.move items=[B] start=60f mode=insert",
      "commit #1",
    ],
    history: ["item.move items=[B] start=60f mode=insert"],
    end: "hover",
  },
  {
    name: "the first main-track clip dragged to the end goes after the last cut",
    steps: drag(clip("A"), by(clip("A"), 80)),
    show: ["update_tx"],
    expect: ["update #1 item.move items=[A] start=60f mode=insert"],
    end: "hover",
  },
  {
    name: "a clip from a free track goes into the main track at the nearest cut",
    steps: drag(clip("Z"), onto(by(clip("Z"), -68), "V1")),
    show: ["update_tx", "preview"],
    expect: [
      "preview V2 101f..121f place",
      "preview V1 30f..50f insert",
      "update #1 item.move items=[Z] start=30f to_track=V1 mode=insert",
      "preview none",
    ],
    end: "hover",
  },
  {
    name: "a main-track clip dragged up onto a free track leaves the main track",
    steps: drag(clip("C"), onto(clip("C"), "V3")),
    show: ["update_tx"],
    expect: ["update #1 item.move items=[C] start=60f to_track=V3 mode=new_track"],
    end: "hover",
  },
  {
    name: "the main track's guide marks the cut the clip goes into",
    steps: drag(clip("B"), by(clip("B"), 40)),
    show: ["snap_guides", "commit_tx"],
    expect: ["guides [edge@30f]", "guides [edge@60f]", "commit #1", "guides []"],
    end: "hover",
  },
];

// ---- duplicating (Alt-drag) -----------------------------------------------------------------

const duplicating: Transcript[] = [
  {
    name: "Alt-drag on a free track duplicates at the drop",
    steps: drag(clip("Z"), by(clip("Z"), 30), { mods: { alt: true } }),
    expect: [
      sel("Z"),
      "begin #1 duplicate",
      "update #1 item.duplicate items=[Z] offset=30f mode=new_track",
      "commit #1",
    ],
    history: ["item.duplicate items=[Z] offset=30f mode=new_track"],
    end: "hover",
  },
  {
    name: "Alt-drag on the main track inserts the copy after the clip",
    steps: drag(clip("B"), by(clip("B"), 30), { mods: { alt: true } }),
    expect: [sel("B"), "begin #1 duplicate", "update #1 item.duplicate items=[B]", "commit #1"],
    end: "hover",
  },
  {
    name: "Alt pressed after the drag started keeps it a move",
    steps: [
      down(clip("Z")),
      move(px(clip("Z"), 4)),
      move(by(clip("Z"), 20), { alt: true }),
      frame,
      up(by(clip("Z"), 20), { alt: true }),
    ],
    show: ["begin_tx", "update_tx"],
    expect: ["begin #1 move", `update #1 ${moveZ("start=120f mode=new_track")}`],
    end: "hover",
  },
];

// ---- trims ----------------------------------------------------------------------------------

const trimming: Transcript[] = [
  {
    name: "a free clip's end edge trims and snaps to the marker",
    steps: drag(clip("Z", "end"), by(clip("Z", "end"), 9)),
    expect: [
      sel("Z"),
      "begin #1 trim",
      "update #1 clip.trim item=Z edge=end to=130f clamp=true",
      "commit #1",
    ],
    history: ["clip.trim item=Z edge=end to=130f clamp=true"],
    end: "hover",
  },
  {
    name: "Alt on an edge trims only that member of its group",
    steps: drag(clip("b", "start"), by(clip("b", "start"), -5), { mods: { alt: true } }),
    expect: [
      sel("b"),
      "begin #1 trim",
      "update #1 clip.trim item=b edge=start to=25f clamp=true unlink=true",
      "commit #1",
    ],
    end: "hover",
  },
  {
    name: "a main-track trim is a plain trim request (the engine ripples)",
    steps: drag(clip("A", "end"), by(clip("A", "end"), 10)),
    show: ["update_tx"],
    expect: ["update #1 clip.trim item=A edge=end to=40f clamp=true"],
    end: "hover",
  },
  {
    name: "a trim the engine clamped drops its guide",
    steps: [...drag(clip("Z", "end"), by(clip("Z", "end"), 9), { release: false }), accepted(true)],
    show: ["update_tx", "snap_guides"],
    expect: [
      "guides [marker@130f]",
      "update #1 clip.trim item=Z edge=end to=130f clamp=true",
      "guides []",
    ],
    end: "trimming",
  },
  {
    name: "a refused trim shows not-allowed, keeps going, and commits the last good update",
    steps: [
      ...drag(clip("Z", "end"), by(clip("Z", "end"), 9), { release: false }),
      refused("invalid"),
      up(by(clip("Z", "end"), 9)),
    ],
    show: ["update_tx", "commit_tx", "set_cursor"],
    expect: [
      "cursor trim-end",
      "update #1 clip.trim item=Z edge=end to=130f clamp=true",
      "cursor not-allowed",
      "commit #1",
      "cursor default",
    ],
    end: "hover",
  },
  {
    name: "a clip on a locked track cannot be dragged",
    steps: drag(clip("K", "end"), by(clip("K", "end"), 5)),
    expect: [sel("K")],
    history: [],
    end: "hover",
  },
];

// ---- the ripple, roll, slip and slide tools -------------------------------------------------

const tool = (t: Tool): Step => env({ tool: t });

const editTools: Transcript[] = [
  {
    name: "the ripple tool trims an edge with a ripple trim",
    steps: [tool("ripple"), ...drag(clip("Y", "end"), by(clip("Y", "end"), 10))],
    expect: [
      sel("Y"),
      "begin #1 ripple_trim",
      "update #1 clip.ripple_trim item=Y edge=end to=90f clamp=true",
      "commit #1",
    ],
    end: "hover",
  },
  {
    name: "the ripple tool's start edge is read as if the clip did not move",
    steps: [tool("ripple"), ...drag(clip("Y", "start"), by(clip("Y", "start"), 5))],
    show: ["update_tx"],
    expect: ["update #1 clip.ripple_trim item=Y edge=start to=45f clamp=true"],
    end: "hover",
  },
  {
    name: "the ripple tool on a clip body only selects",
    steps: [tool("ripple"), ...drag(clip("Y"), by(clip("Y"), 10))],
    expect: [sel("Y")],
    end: "hover",
  },
  {
    name: "the roll tool rolls the cut between two clips",
    steps: [tool("roll"), ...drag(clip("A", "end"), by(clip("A", "end"), 8))],
    expect: [
      sel("A", "a"),
      "begin #1 roll",
      "update #1 clip.roll left=A right=B to=40f clamp=true",
      "commit #1",
    ],
    end: "hover",
  },
  {
    name: "the roll tool on a start edge rolls with the clip before",
    steps: [tool("roll"), ...drag(clip("B", "start"), by(clip("B", "start"), -10))],
    show: ["update_tx"],
    expect: ["update #1 clip.roll left=A right=B to=20f clamp=true"],
    end: "hover",
  },
  {
    name: "the roll tool on an edge with a gap beside it trims",
    steps: [tool("roll"), ...drag(clip("Z", "start"), by(clip("Z", "start"), -3))],
    show: ["begin_tx", "update_tx"],
    expect: ["begin #1 trim", "update #1 clip.trim item=Z edge=start to=97f clamp=true"],
    end: "hover",
  },
  {
    name: "the slip tool: dragging right shows earlier media",
    steps: [tool("slip"), ...drag(clip("Z"), by(clip("Z"), 10))],
    expect: [
      sel("Z"),
      "begin #1 slip",
      "update #1 clip.slip item=Z delta=-10f clamp=true",
      "commit #1",
    ],
    end: "hover",
  },
  {
    name: "the slip tool on a still has nothing to slip",
    steps: [tool("slip"), ...drag(clip("X"), by(clip("X"), 10))],
    expect: [sel("X")],
    end: "hover",
  },
  {
    name: "the slide tool moves a clip between its neighbours",
    steps: [tool("slide"), ...drag(clip("B"), by(clip("B"), 5))],
    expect: [
      sel("B", "b"),
      "begin #1 slide",
      "update #1 clip.slide item=B delta=5f clamp=true",
      "commit #1",
    ],
    end: "hover",
  },
  {
    name: "a slide snaps the clip's start to a nearby edge",
    steps: [tool("slide"), ...drag(clip("B"), by(clip("B"), 8))],
    show: ["update_tx"],
    expect: ["update #1 clip.slide item=B delta=10f clamp=true"],
    end: "hover",
  },
  {
    name: "a partly selected group cannot be slid: one notice, no gesture",
    world: { env: { selection: ["B"] } },
    steps: [
      tool("slide"),
      down(clip("B")),
      move(px(clip("B"), 4)),
      move(by(clip("B"), 5)),
      up(by(clip("B"), 5)),
    ],
    expect: ["notice partial_group"],
    end: "hover",
  },
];

// ---- razor ----------------------------------------------------------------------------------

const razor: Transcript[] = [
  {
    name: "a razor click splits the clip under the blade at the snapped time",
    steps: [tool("razor"), down(clip("Y")), up(clip("Y"))],
    expect: ["command clip.split items=[Y] at=60f"],
    history: ["clip.split items=[Y] at=60f"],
    end: "hover",
  },
  {
    name: "Shift+razor splits every clip at that time, one per linked group",
    steps: [
      tool("razor"),
      down(lane("V2", 45), { shift: true }),
      up(lane("V2", 45), { shift: true }),
    ],
    expect: ["command clip.split items=[Y,B,M] at=45f"],
    end: "hover",
  },
  {
    name: "Esc before the release splits nothing",
    steps: [tool("razor"), down(clip("Y")), key("Escape"), up(clip("Y"))],
    expect: [],
    history: [],
    end: "idle",
  },
  {
    name: "the razor on a locked clip splits nothing",
    steps: [tool("razor"), down(clip("K")), up(clip("K"))],
    expect: [],
    end: "hover",
  },
  {
    name: "the razor's blade follows the pointer and goes when it leaves",
    steps: [tool("razor"), move(lane("V2", 45)), move(lane("V2", 47)), leave],
    show: ["snap_guides", "set_cursor"],
    expect: ["cursor razor", "guides [@45f]", "guides [@47f]", "guides []", "cursor default"],
    end: "idle",
  },
];

// ---- scrubbing, panning, zooming ------------------------------------------------------------

const view: Transcript[] = [
  {
    name: "a press on the ruler seeks; moves coalesce to one seek per frame; release flushes",
    steps: [
      down(ruler(40)),
      move(ruler(55)),
      move(ruler(57)),
      frame,
      move(ruler(80)),
      up(ruler(80)),
    ],
    expect: ["seek 40f", "seek 57f", "seek 80f"],
    end: "hover",
  },
  {
    name: "Esc during a scrub puts the playhead back",
    steps: [down(ruler(40)), move(ruler(70)), frame, key("Escape")],
    expect: ["seek 40f", "seek 70f", "seek 150f"],
    end: "idle",
  },
  {
    name: "a press on the playhead flag scrubs too",
    steps: [down(ruler(150)), up(ruler(150))],
    expect: ["seek 150f"],
    end: "hover",
  },
  {
    name: "the hand tool drags the view",
    world: { env: { scrollX: f(50) } },
    steps: [tool("hand"), down(point(500, 168)), move(point(460, 168)), up(point(460, 168))],
    expect: ["scroll_to x=60f y=0"],
    end: "hover",
  },
  {
    name: "the middle button pans with any tool",
    world: { env: { scrollX: f(50) } },
    steps: [
      down(point(500, 168), { button: 1 }),
      move(point(540, 168)),
      up(point(540, 168), { button: 1 }),
    ],
    expect: ["scroll_to x=40f y=0"],
    end: "hover",
  },
  {
    name: "Esc during a pan puts the view back",
    world: { env: { scrollX: f(50) } },
    steps: [tool("hand"), down(point(500, 168)), move(point(460, 168)), key("Escape")],
    expect: ["scroll_to x=60f y=0", "scroll_to x=50f y=0"],
    end: "idle",
  },
  {
    name: "a zoom-tool click zooms in at the pointer",
    steps: [tool("zoom"), down(point(500, 168)), up(point(500, 168))],
    expect: ["zoom_by 2 at x=500"],
    end: "hover",
  },
  {
    name: "an Alt+click with the zoom tool zooms out",
    steps: [tool("zoom"), down(point(500, 168), { alt: true }), up(point(500, 168), { alt: true })],
    expect: ["zoom_by 0.5 at x=500"],
    end: "hover",
  },
  {
    name: "a zoom-tool drag zooms to the range",
    steps: [tool("zoom"), down(lane("V2", 40)), move(lane("V2", 80)), up(lane("V2", 80))],
    show: ["zoom_to_range", "marquee"],
    expect: ["marquee 260,28 160x352", "zoom_to 40f..80f", "marquee none"],
    end: "hover",
  },
  {
    name: "Esc during a zoom-tool drag zooms nowhere",
    steps: [tool("zoom"), down(lane("V2", 40)), move(lane("V2", 80)), key("Escape")],
    show: ["zoom_to_range", "zoom_by", "marquee"],
    expect: ["marquee 260,28 160x352", "marquee none"],
    end: "idle",
  },
];

// ---- dropping media from the bin ------------------------------------------------------------

const insert = (args: string): string => `timeline.insert sequence=S1 ${args}`;

const dropping: Transcript[] = [
  {
    name: "a video dropped on free space is inserted there (one gesture)",
    steps: [mediaOver(lane("V3", 50), VIDEO), frame, mediaDrop(lane("V3", 50), VIDEO)],
    expect: ["begin #1 insert", `update #1 ${insert("track=V3 media=mv at=50f")}`, "commit #1"],
    history: [insert("track=V3 media=mv at=50f")],
    end: "idle",
  },
  {
    name: "a card dropped onto a clip outside the main track overwrites (the default preference)",
    steps: [mediaOver(clip("Y"), VIDEO), frame, mediaDrop(clip("Y"), VIDEO)],
    show: ["update_tx"],
    expect: [`update #1 ${insert("track=V2 media=mv at=60f mode=overwrite")}`],
    end: "idle",
  },
  {
    name: "with the insert preference a card dropped onto a clip inserts",
    world: { env: { dropOnClip: "insert" } },
    steps: [mediaOver(clip("Y"), VIDEO), frame, mediaDrop(clip("Y"), VIDEO)],
    show: ["update_tx"],
    expect: [`update #1 ${insert("track=V2 media=mv at=60f mode=insert")}`],
    end: "idle",
  },
  {
    name: "a card over the main track goes in at the nearest cut",
    steps: [
      move(lane("V1", 40)),
      mediaOver(lane("V1", 40), VIDEO),
      frame,
      mediaDrop(lane("V1", 40), VIDEO),
    ],
    show: ["update_tx", "preview"],
    expect: [
      "preview V1 30f..50f insert",
      `update #1 ${insert("track=V1 media=mv at=30f mode=insert")}`,
      "preview none",
    ],
    end: "idle",
  },
  {
    name: "a card dragged away again is cancelled",
    steps: [mediaOver(lane("V3", 50), VIDEO), frame, mediaLeave],
    expect: [
      "begin #1 insert",
      `update #1 ${insert("track=V3 media=mv at=50f")}`,
      "cancel #1 media_leave",
    ],
    history: [],
    end: "idle",
  },
  {
    name: "a sound over a video track lands on the first audio track",
    steps: [mediaOver(clip("Z"), AUDIO), mediaDrop(clip("Z"), AUDIO)],
    show: ["update_tx"],
    expect: [`update #1 ${insert("track=A1 media=ma at=110f")}`],
    end: "idle",
  },
  {
    name: "a still over free space too short for it asks for a new track",
    steps: [mediaOver(lane("V2", 85), IMAGE), mediaDrop(lane("V2", 85), IMAGE)],
    show: ["update_tx", "preview"],
    expect: [
      "preview V2 85f..210f new_track",
      `update #1 ${insert("track=V2 media=mi at=85f mode=new_track")}`,
      "preview none",
    ],
    end: "idle",
  },
  {
    name: "Esc during a card drag cancels it",
    steps: [mediaOver(lane("V3", 50), VIDEO), key("Escape")],
    expect: ["begin #1 insert", "cancel #1 escape"],
    end: "idle",
  },
  {
    name: "a drop with no frame before it still sends its update",
    steps: [mediaDrop(lane("V3", 50), VIDEO)],
    expect: ["begin #1 insert", `update #1 ${insert("track=V3 media=mv at=50f")}`, "commit #1"],
    end: "idle",
  },
  {
    name: "the engine cancelling a card drag ends it",
    steps: [mediaOver(lane("V3", 50), VIDEO), frame, engineCancelled()],
    show: ["cancel_tx"],
    expect: ["cancel #1 engine"],
    end: "idle",
  },
];

// ---- keys and one-shot commands -------------------------------------------------------------

const keys: Transcript[] = [
  {
    name: "S splits the selected clips at the playhead",
    world: { env: { playhead: f(45), selection: ["B", "b", "Y"] } },
    steps: [key("s")],
    expect: ["command clip.split items=[B,Y] at=45f"],
    end: "idle",
  },
  {
    name: "S with nothing selected splits the topmost clip under the playhead",
    world: { env: { playhead: f(45) } },
    steps: [key("s")],
    expect: ["command clip.split items=[Y] at=45f"],
    end: "idle",
  },
  {
    name: "Delete deletes the selection, one id per linked group",
    world: { env: { selection: ["B", "b", "Z"] } },
    steps: [key("Delete")],
    expect: ["command item.delete items=[B,Z]"],
    history: ["item.delete items=[B,Z]"],
    end: "idle",
  },
  {
    name: "Shift+Delete ripple-deletes the selection",
    world: { env: { selection: ["Z"] } },
    steps: [key("Delete", { shift: true })],
    expect: ["command timeline.ripple_delete items=[Z]"],
    end: "idle",
  },
  {
    name: "Delete with a partly selected group is refused with a notice",
    world: { env: { selection: ["B"] } },
    steps: [key("Backspace")],
    expect: ["notice partial_group"],
    history: [],
    end: "idle",
  },
  {
    name: "S and Delete wait while a drag is in progress",
    steps: [
      ...drag(clip("Z"), by(clip("Z"), 20), { release: false }),
      key("s"),
      key("Delete"),
      up(by(clip("Z"), 20)),
    ],
    expect: [
      sel("Z"),
      "begin #1 move",
      `update #1 ${moveZ("start=120f mode=new_track")}`,
      "commit #1",
    ],
    end: "hover",
  },
  {
    name: "period and comma nudge by a frame, with Shift by ten",
    world: { env: { selection: ["Z"] } },
    steps: [key("."), key(">", { shift: true }), key(",")],
    expect: [
      "command item.move items=[Z] delta=1f",
      "command item.move items=[Z] delta=10f",
      "command item.move items=[Z] delta=-1f",
    ],
    end: "idle",
  },
  {
    name: "a nudge leaves main-track clips alone",
    world: { env: { selection: ["B", "b"] } },
    steps: [key(".")],
    expect: [],
    end: "idle",
  },
  {
    name: "tool keys switch tools; the host's keymap has the last word",
    steps: [
      key("c"),
      send({ type: "key.down", key: "v", action: null }),
      send({ type: "key.down", key: "x", action: "tool.hand" }),
    ],
    expect: ["tool razor", "tool hand"],
    end: "idle",
  },
  {
    name: "N toggles snapping",
    steps: [key("n"), key("n")],
    expect: ["snapping off", "snapping on"],
    end: "idle",
  },
  {
    name: "select all and deselect",
    steps: [action("select_all"), action("deselect")],
    expect: [sel(...MODEL.items.map((it) => it.id)), sel()],
    end: "idle",
  },
  {
    name: "Ctrl+S is not the split key",
    world: { env: { playhead: f(45) } },
    steps: [key("s", { ctrl: true })],
    expect: [],
    end: "idle",
  },
];

// ---- the engine and other actors ------------------------------------------------------------

const engine: Transcript[] = [
  {
    name: "a conflict with another actor's gesture shows the locked cursor and keeps the drag",
    steps: [
      ...drag(clip("Z"), by(clip("Z"), 20), { release: false }),
      refused("conflict", "item_in_gesture", "agent:claude"),
      move(by(clip("Z"), 30)),
      frame,
      accepted(),
      up(by(clip("Z"), 30)),
    ],
    show: ["update_tx", "commit_tx", "set_cursor"],
    expect: [
      "cursor move",
      `update #1 ${moveZ("start=120f mode=new_track")}`,
      "cursor locked",
      `update #1 ${moveZ("start=130f mode=new_track")}`,
      "cursor move",
      "commit #1",
      "cursor default",
    ],
    end: "hover",
  },
  {
    name: "a refused update is never sent again on its own",
    steps: [
      ...drag(clip("Z"), by(clip("Z"), 20), { release: false }),
      refused("invalid"),
      frames(3),
      move(by(clip("Z"), 20)),
      frame,
      up(by(clip("Z"), 20)),
    ],
    show: ["update_tx", "commit_tx"],
    expect: [`update #1 ${moveZ("start=120f mode=new_track")}`, "commit #1"],
    end: "hover",
  },
  {
    name: "the engine cancels an idle gesture: back to idle, the release does nothing",
    steps: [
      ...drag(clip("Z"), by(clip("Z"), 20), { release: false }),
      engineCancelled(),
      move(by(clip("Z"), 25)),
      up(by(clip("Z"), 25)),
    ],
    expect: [
      sel("Z"),
      "begin #1 move",
      `update #1 ${moveZ("start=120f mode=new_track")}`,
      "cancel #1 engine",
    ],
    history: [],
    end: "hover",
  },
  {
    name: "a lost transaction ends the gesture",
    steps: [...drag(clip("Z"), by(clip("Z"), 20), { release: false }), refused("not_found")],
    show: ["cancel_tx"],
    expect: ["cancel #1 engine"],
    end: "idle",
  },
  {
    name: "a gesture the engine could not open ends at once",
    steps: [down(clip("Z")), move(px(clip("Z"), 4)), refused("unavailable"), frame],
    expect: [sel("Z"), "begin #1 move", "cancel #1 engine"],
    end: "idle",
  },
  {
    name: "an agent deletes the dragged clip before the drag locked it",
    steps: [down(clip("Z")), move(px(clip("Z"), 4)), (w) => w.setModel(without(MODEL, "Z")), frame],
    expect: [sel("Z"), "begin #1 move", "cancel #1 target_gone"],
    end: "idle",
  },
  {
    name: "an agent deletes a pressed clip before the drag starts",
    steps: [down(clip("Z")), (w) => w.setModel(without(MODEL, "Z")), move(lane("V2", 120))],
    expect: [sel("Z")],
    end: "hover",
  },
  {
    name: "an agent's edit elsewhere does not disturb the drag",
    steps: [
      down(clip("Z")),
      move(px(clip("Z"), 4)),
      (w) => w.setModel(without(MODEL, "M")),
      move(by(clip("Z"), 20)),
      frame,
      up(by(clip("Z"), 20)),
    ],
    show: ["update_tx", "commit_tx", "cancel_tx"],
    expect: [`update #1 ${moveZ("start=120f mode=new_track")}`, "commit #1"],
    end: "hover",
  },
  {
    name: "an engine answer about an earlier gesture is ignored",
    steps: [
      ...drag(clip("Z"), by(clip("Z"), 20)),
      send({ type: "engine.result", gesture: 1, ok: false, code: "invalid" }),
      send({ type: "engine.cancelled", gesture: 1, reason: "idle_timeout" }),
    ],
    show: ["cancel_tx", "commit_tx"],
    expect: ["commit #1"],
    end: "hover",
  },
  {
    name: "a clip another actor holds shows the locked cursor on hover",
    world: { env: { heldBy: new Map([["Z", "agent:claude"]]) } },
    steps: [move(clip("Y")), move(clip("Z"))],
    show: ["set_cursor"],
    expect: ["cursor move", "cursor locked"],
    end: "hover",
  },
];

// ---- cancelling -----------------------------------------------------------------------------

const cancelling: Transcript[] = [
  {
    name: "a press released by Esc before the threshold opens nothing",
    steps: [down(clip("Z")), key("Escape"), up(clip("Z"))],
    expect: [sel("Z")],
    end: "idle",
  },
  {
    name: "the pointer leaving the canvas does not end a drag",
    steps: [
      ...drag(clip("Z"), by(clip("Z"), 20), { release: false }),
      leave,
      move(by(clip("Z"), 22)),
      frame,
      up(by(clip("Z"), 22)),
    ],
    show: ["update_tx", "commit_tx"],
    expect: [
      `update #1 ${moveZ("start=120f mode=new_track")}`,
      `update #1 ${moveZ("start=122f mode=new_track")}`,
      "commit #1",
    ],
    end: "hover",
  },
  {
    name: "a lost window while hovering goes idle; Esc while hovering does nothing",
    steps: [move(clip("Y")), key("Escape"), blur],
    show: ["set_cursor"],
    expect: ["cursor move", "cursor default"],
    end: "idle",
  },
];

// ---- every gesture rolls back --------------------------------------------------------------

interface GestureCase {
  readonly state: StateName;
  readonly steps: readonly Step[];
}

/** A press and a travel that open each kind of gesture, with one update sent. */
const OPENERS: readonly GestureCase[] = [
  { state: "moving", steps: drag(clip("Z"), by(clip("Z"), 10), { release: false }) },
  { state: "trimming", steps: drag(clip("Z", "end"), by(clip("Z", "end"), 5), { release: false }) },
  {
    state: "ripple_trimming",
    steps: [tool("ripple"), ...drag(clip("Y", "end"), by(clip("Y", "end"), 5), { release: false })],
  },
  {
    state: "rolling",
    steps: [tool("roll"), ...drag(clip("A", "end"), by(clip("A", "end"), 5), { release: false })],
  },
  {
    state: "slipping",
    steps: [tool("slip"), ...drag(clip("Z"), by(clip("Z"), 5), { release: false })],
  },
  {
    state: "sliding",
    steps: [tool("slide"), ...drag(clip("B"), by(clip("B"), 5), { release: false })],
  },
  {
    state: "keyframe_dragging",
    steps: drag(clip("Y", 0), by(clip("Y", 0), 5), { release: false }),
  },
];

const ENDINGS: readonly { readonly name: string; readonly step: Step; readonly reason: string }[] =
  [
    { name: "Esc", step: key("Escape"), reason: "escape" },
    { name: "a lost window", step: blur, reason: "blur" },
    { name: "a lost pointer", step: pointerCancel, reason: "pointer_cancel" },
    { name: "the engine's idle timeout", step: engineCancelled(), reason: "engine" },
  ];

const rollbacks: Transcript[] = OPENERS.flatMap((g) => [
  {
    name: `${g.state}: the opened gesture is the expected state`,
    world: { keyframes: true },
    steps: g.steps,
    show: ["begin_tx", "commit_tx", "cancel_tx"],
    expect: [`begin #1 ${editOf(g.state)}`],
    end: g.state,
  },
  ...ENDINGS.map(
    (e): Transcript => ({
      name: `${g.state}: ${e.name} rolls the gesture back`,
      world: { keyframes: true },
      steps: [...g.steps, e.step],
      show: ["commit_tx", "cancel_tx"],
      expect: [`cancel #1 ${e.reason}`],
      history: [],
      end: "idle",
    }),
  ),
  {
    name: `${g.state}: the release commits one undo step`,
    world: { keyframes: true },
    steps: [...g.steps, (w) => up(pointerOf(w))(w)],
    show: ["commit_tx", "cancel_tx"],
    expect: ["commit #1"],
    end: "hover",
  },
]);

function editOf(state: StateName): string {
  switch (state) {
    case "moving":
      return "move";
    case "trimming":
      return "trim";
    case "ripple_trimming":
      return "ripple_trim";
    case "rolling":
      return "roll";
    case "slipping":
      return "slip";
    case "sliding":
      return "slide";
    default:
      return "keyframe";
  }
}

const pointerOf =
  (w: { state: { pointer: { x: number; y: number } | null } }): Pos =>
  () =>
    w.state.pointer ?? { x: 0, y: 0 };

// ---- auto-scroll, keyframes -----------------------------------------------------------------

const edges: Transcript[] = [
  {
    name: "a drag into the right edge scrolls the view and the clip follows",
    steps: [
      down(clip("Z")),
      move(px(clip("Z"), 4)),
      move(point(990, 168)),
      frame,
      frame,
      up(point(990, 168)),
    ],
    show: ["update_tx", "commit_tx", "scroll_by"],
    expect: [
      `update #1 ${moveZ("start=213f mode=new_track")}`,
      "scroll_by 17,0",
      `update #1 ${moveZ("start=217f mode=new_track")}`,
      "scroll_by 17,0",
      `update #1 ${moveZ("start=221f mode=new_track")}`,
      "commit #1",
    ],
    end: "hover",
  },
  {
    name: "no auto-scroll before the timeline start",
    steps: [down(clip("Z")), move(px(clip("Z"), 4)), move(point(110, 168)), frame, frame, frame],
    show: ["update_tx", "scroll_by"],
    expect: [`update #1 ${moveZ("start=0f mode=new_track")}`],
    end: "moving",
  },
  {
    name: "a dragged keyframe moves through the keyframe handler",
    world: { keyframes: true },
    steps: drag(clip("Y", 0), by(clip("Y", 0), 7)),
    expect: [
      sel("Y"),
      "begin #1 keyframe",
      "update #1 keyframe.move item=Y keys=[{prop=opacity t=50f}] to_at=57f",
      "commit #1",
    ],
    end: "hover",
  },
  {
    name: "without a keyframe handler a press on a keyframe moves the clip",
    steps: drag(clip("Y", 0), by(clip("Y", 0), 10)),
    show: ["begin_tx", "update_tx"],
    expect: ["begin #1 move", "update #1 item.move items=[Y] start=50f mode=new_track"],
    end: "hover",
  },
  {
    name: "pressing Ctrl mid-drag lets go of the snap at once",
    steps: [
      down(clip("Z")),
      move(px(clip("Z"), 4)),
      move(by(clip("Z"), -18)),
      frame,
      key("Control", { ctrl: true }),
      frame,
      keyUp("Control"),
      frame,
      up(by(clip("Z"), -18)),
    ],
    show: ["update_tx", "commit_tx"],
    expect: [
      `update #1 ${moveZ("start=80f mode=new_track")}`,
      `update #1 ${moveZ("start=82f mode=new_track")}`,
      `update #1 ${moveZ("start=80f mode=new_track")}`,
      "commit #1",
    ],
    end: "hover",
  },
];

export const TRANSCRIPTS: readonly Transcript[] = [
  ...selection,
  ...moving,
  ...mainTrack,
  ...duplicating,
  ...trimming,
  ...editTools,
  ...razor,
  ...view,
  ...dropping,
  ...keys,
  ...engine,
  ...cancelling,
  ...rollbacks,
  ...edges,
];
