// The trim model: which parts of a track are removed, as pure functions over seconds.
// Ranges are clamped to [0, duration], slivers under MIN_LEN are dropped, overlaps merge.

export interface Range {
  start: number;
  end: number;
}

const MIN_LEN = 0.05;
const UNKNOWN_TIME = "–:––";

/** Clamp to [0, duration], drop slivers, sort, and merge overlapping or touching ranges. */
export function normalize(removed: Range[], duration: number): Range[] {
  if (!(duration > 0)) return [];
  const clamped = removed
    .map((r) => ({ start: Math.max(0, r.start), end: Math.min(duration, r.end) }))
    .filter((r) => r.end - r.start >= MIN_LEN)
    .sort((a, b) => a.start - b.start);
  const out: Range[] = [];
  for (const r of clamped) {
    const last = out[out.length - 1];
    if (last && r.start <= last.end) last.end = Math.max(last.end, r.end);
    else out.push(r);
  }
  return out;
}

/** The complement of the removed ranges within [0, duration], without kept slivers. */
export function kept(removed: Range[], duration: number): Range[] {
  if (!(duration > 0)) return [];
  const out: Range[] = [];
  let cursor = 0;
  for (const r of normalize(removed, duration)) {
    if (r.start - cursor >= MIN_LEN) out.push({ start: cursor, end: r.start });
    cursor = r.end;
  }
  if (duration - cursor >= MIN_LEN) out.push({ start: cursor, end: duration });
  return out;
}

export function keptLength(removed: Range[], duration: number): number {
  return kept(removed, duration).reduce((sum, r) => sum + (r.end - r.start), 0);
}

/** Where preview playback should jump when it reaches `t` inside a removed range, else null. */
export function skipTarget(t: number, removed: Range[]): number | null {
  let cur = t;
  let hit = false;
  let changed = true;
  while (changed) {
    changed = false;
    for (const r of removed) {
      if (cur >= r.start && cur < r.end) {
        cur = r.end;
        hit = true;
        changed = true;
      }
    }
  }
  return hit ? cur : null;
}

/** Move one edge of a range by `delta`, clamped so the range stays within [0, duration] and at least MIN_LEN long. */
export function nudge(r: Range, edge: "start" | "end", delta: number, duration: number): Range {
  if (edge === "start") {
    const start = Math.min(Math.max(r.start + delta, 0), Math.max(0, r.end - MIN_LEN));
    return { start, end: r.end };
  }
  const end = Math.max(Math.min(r.end + delta, duration), r.start + MIN_LEN);
  return { start: r.start, end };
}

/** `m:ss`, or `h:mm:ss` from an hour up. Unknown or non-finite input shows a placeholder. */
export function formatTime(sec: number): string {
  if (!Number.isFinite(sec)) return UNKNOWN_TIME;
  const s = Math.max(0, Math.floor(sec));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/** Undo stack of removed-range states. The first state is the untrimmed track. */
export class History {
  private states: Range[][] = [[]];

  push(removed: Range[]): void {
    this.states.push([...removed]);
  }

  /** Drop the latest state and return the one before it, or null at the start. */
  undo(): Range[] | null {
    if (this.states.length <= 1) return null;
    this.states.pop();
    return this.current();
  }

  current(): Range[] {
    return this.states[this.states.length - 1];
  }
}
