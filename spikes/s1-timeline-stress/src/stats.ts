// SPDX-License-Identifier: GPL-3.0-or-later
export function percentile(sorted: ArrayLike<number>, p: number): number {
  if (sorted.length === 0) return 0;
  const i = Math.min(sorted.length - 1, Math.max(0, Math.ceil((p / 100) * sorted.length) - 1));
  return sorted[i] ?? 0;
}

export function sortedCopy(xs: readonly number[]): number[] {
  return xs.slice().sort((a, b) => a - b);
}

export function round(x: number, digits = 2): number {
  const f = 10 ** digits;
  return Math.round(x * f) / f;
}

/** Fixed-size ring of the most recent samples (for the live HUD). */
export class Ring {
  private readonly buf: number[] = [];
  private pos = 0;
  constructor(private readonly cap: number) {}
  push(x: number): void {
    if (this.buf.length < this.cap) this.buf.push(x);
    else {
      this.buf[this.pos] = x;
      this.pos = (this.pos + 1) % this.cap;
    }
  }
  values(): number[] {
    return this.buf;
  }
}
