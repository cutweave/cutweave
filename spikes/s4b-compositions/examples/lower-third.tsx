// The lower-third example of docs/01-architecture.md §12, unchanged below this comment.
import { defineComposition, p, interpolate, spring, Text, Rect, Group } from "@cutweave/compose";

export default defineComposition({
  id: "acme.lower-third",
  props: p.object({ name: p.string(), title: p.string(), accent: p.color().default("#7c5cff") }),
  durationInFrames: 120,
  render: ({ frame, fps, props, size }) => {
    const x = spring({ frame, fps, from: -400, to: 80 });
    const o = interpolate(frame, [100, 120], [1, 0], { clamp: true });
    return (
      <Group opacity={o} x={x} y={size.h - 220}>
        <Rect w={520} h={110} radius={18} fill={props.accent} />
        <Text x={24} y={18} font="Inter 700" size={42}>{props.name}</Text>
        <Text x={24} y={66} font="Inter 400" size={26} opacity={0.85}>{props.title}</Text>
      </Group>
    );
  },
});
