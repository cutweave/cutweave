// Spike S4b stress composition: exactly 200 scene nodes per frame, every one animated
// (spring with per-node delay, eased colour cycle via mixColor, seeded jitter, rotation, opacity).
import {
  defineComposition,
  Ellipse,
  easing,
  Group,
  Image,
  interpolate,
  mixColor,
  p,
  random,
  Rect,
  spring,
  Text,
} from "@cutweave/compose";

const ROWS = 8;
const COLS = 22;

type Size = { w: number; h: number };

/** A function component: called by the JSX factory, no state, no hooks. */
function Corner({ k, size, fill, frame }: { k: number; size: Size; fill: string; frame: number }) {
  return (
    <Ellipse
      x={k % 2 === 1 ? size.w - 40 : 40}
      y={k < 2 ? size.h - 40 : 40}
      w={24}
      h={24}
      anchor={0.5}
      fill={fill}
      opacity={0.5 + 0.5 * Math.sin(frame / 10 + k)}
    />
  );
}

export default defineComposition({
  id: "cutweave.stress-200",
  title: "200-node stress scene",
  props: p.object({
    from: p.color().default("#7c5cff").label("Start colour"),
    to: p.color().default("#14b8a6").label("End colour"),
    speed: p.number().min(0.1).max(4).step(0.1).default(1).animatable(),
    title: p.string().max(80).default("Cutweave S4b: 200 animated nodes"),
  }),
  durationInFrames: 240,
  fps: 60,
  render: ({ frame, fps, props, size }) => {
    const cell = Math.min((size.w - 160) / COLS, (size.h - 220) / ROWS);
    const rows = [];
    for (let r = 0; r < ROWS; r++) {
      const shapes = [];
      for (let c = 0; c < COLS; c++) {
        const i = r * COLS + c;
        const t = spring({
          frame: frame * props.speed,
          fps,
          delay: (r + c) * 2,
          stiffness: 140,
          damping: 11,
        });
        const jitter = (random(`j${i}`) - 0.5) * 8;
        const phase = ((frame / fps) * props.speed + random(i)) % 1;
        const mix = interpolate(phase, [0, 0.5, 1], [0, 1, 0], { easing: easing.inOutSine });
        const s = cell * 0.8 * t;
        const common = {
          x: c * cell + cell / 2,
          y: cell / 2 + jitter,
          w: s,
          h: s,
          anchor: 0.5,
          fill: mixColor(props.from, props.to, mix),
          opacity: 0.4 + 0.6 * Math.min(1, t),
          rotation: 90 * easing.outCubic(Math.min(1, t)),
        };
        shapes.push(
          c % 2 === 0 ? (
            <Rect key={`s${i}`} {...common} radius={s * 0.2} />
          ) : (
            <Ellipse key={`s${i}`} {...common} />
          ),
        );
      }
      rows.push(
        <Group
          key={`row${r}`}
          x={120}
          y={160 + r * cell}
          opacity={interpolate(frame, [r * 4, r * 4 + 20], [0, 1], { clamp: true })}
        >
          <Text x={-100} y={cell / 2 - 14} size={22} fill="#e5e7eb">{`row ${r + 1}`}</Text>
          {shapes}
        </Group>,
      );
    }
    return (
      <Group>
        <Rect w={size.w} h={size.h} fill="#0b1020" />
        <Text x={60} y={50} font="Inter 700" size={48}>
          {props.title}
        </Text>
        <Image src="logo" x={size.w - 92} y={88} w={96} h={96} anchor={0.5} rotation={frame * 2} />
        {[0, 1, 2, 3].map((k) => (
          <Corner key={k} k={k} size={size} fill={props.to} frame={frame} />
        ))}
        {rows}
      </Group>
    );
  },
});
