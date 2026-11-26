// SPDX-License-Identifier: GPL-3.0-or-later

/** Placeholder output between queries (never written: `walk` runs only inside `query`). */
const NO_OUT: number[] = [];

/**
 * A growable list of item indices that is cleared without giving up its storage: the draw loop
 * refills the same lists every frame (a JS array emptied with `length = 0` drops its backing
 * store, so refilling it allocates).
 */
export class IndexList {
  data: Int32Array;
  length = 0;

  constructor(capacity = 64) {
    this.data = new Int32Array(capacity);
  }

  clear(): void {
    this.length = 0;
  }

  push(v: number): void {
    if (this.length === this.data.length) {
      const grown = new Int32Array(this.data.length * 2);
      grown.set(this.data);
      this.data = grown;
    }
    this.data[this.length++] = v;
  }

  at(i: number): number {
    return this.data[i] ?? -1;
  }

  includes(v: number): boolean {
    for (let i = 0; i < this.length; i++) if (this.data[i] === v) return true;
    return false;
  }
}

/**
 * Static interval tree over a set of items, used for visibility culling and hit-testing.
 *
 * Items are sorted by start; the sorted array is read as an implicit balanced binary tree (node =
 * midpoint of its range) with the maximum end time of each subtree stored at the node. A query
 * prunes subtrees whose max end is before the window and right subtrees whose start is after it,
 * so it costs O(log n + k) even when items overlap (a prefix-max scan degrades to O(n) behind one
 * long item). Results come out in ascending start order. No allocation per query.
 *
 * Intervals are closed: an item overlaps `[a, b]` when `start <= b && end >= a`.
 */
export class IntervalIndex {
  readonly size: number;
  private readonly ids: Int32Array;
  private readonly starts: Float64Array;
  private readonly ends: Float64Array;
  private readonly maxEnd: Float64Array;
  private out: number[] = NO_OUT;
  private list: IndexList | null = null;
  private qa = 0;
  private qb = 0;

  /**
   * @param ids item ids (indices into `allStart` / `allEnd`) that belong to this index
   * @param allStart start time per item id
   * @param allEnd end time per item id
   */
  constructor(ids: ArrayLike<number>, allStart: ArrayLike<number>, allEnd: ArrayLike<number>) {
    const sorted = Array.from(ids).sort((x, y) => (allStart[x] ?? 0) - (allStart[y] ?? 0) || x - y);
    this.size = sorted.length;
    this.ids = Int32Array.from(sorted);
    this.starts = Float64Array.from(sorted, (i) => allStart[i] ?? 0);
    this.ends = Float64Array.from(sorted, (i) => allEnd[i] ?? 0);
    this.maxEnd = new Float64Array(sorted.length);
    this.build(0, sorted.length);
  }

  private build(lo: number, hi: number): number {
    if (lo >= hi) return Number.NEGATIVE_INFINITY;
    const mid = (lo + hi) >>> 1;
    const m = Math.max(this.ends[mid] ?? 0, this.build(lo, mid), this.build(mid + 1, hi));
    this.maxEnd[mid] = m;
    return m;
  }

  /** Ids of items overlapping `[a, b]`, written into `out` (cleared first) and returned. */
  query(a: number, b: number, out: number[]): number[] {
    out.length = 0;
    this.out = out;
    this.qa = a;
    this.qb = b;
    this.walk(0, this.size);
    this.out = NO_OUT; // drop the caller's array without allocating a new one
    return out;
  }

  /** {@link query} into an {@link IndexList} (cleared first): allocation-free once it is big enough. */
  queryInto(a: number, b: number, out: IndexList): IndexList {
    out.clear();
    this.list = out;
    this.qa = a;
    this.qb = b;
    this.walk(0, this.size);
    this.list = null;
    return out;
  }

  /** Ids of items containing time `t`. */
  stab(t: number, out: number[]): number[] {
    return this.query(t, t, out);
  }

  private walk(lo: number, hi: number): void {
    if (lo >= hi) return;
    const mid = (lo + hi) >>> 1;
    if ((this.maxEnd[mid] ?? 0) < this.qa) return;
    this.walk(lo, mid);
    if ((this.starts[mid] ?? 0) > this.qb) return;
    if ((this.ends[mid] ?? 0) >= this.qa) {
      const id = this.ids[mid] ?? 0;
      if (this.list !== null) this.list.push(id);
      else this.out.push(id);
    }
    this.walk(mid + 1, hi);
  }
}
