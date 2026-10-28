// Waveform geometry for the trim dialog, as pure functions of pixels and seconds.

import { nudge, skipTarget, type Range } from "./cut";

/** Pixels either side of an edge that still grab it: a 24 px hit area for fingers. */
export const HIT = 12;

export function timeAt(x: number, width: number, duration: number): number {
  if (!(width > 0) || !(duration > 0)) return 0;
  return Math.min(duration, Math.max(0, (x / width) * duration));
}

export function xAt(t: number, width: number, duration: number): number {
  return duration > 0 ? (t / duration) * width : 0;
}

/** An edge of the selection (`index` -1) or of a removed range. */
export interface EdgeHit {
  target: "sel" | "cut";
  index: number;
  edge: "start" | "end";
}

/** The edge nearest to `x` within `slop` pixels; the selection wins a tie. */
export function hitEdge(
  x: number,
  width: number,
  duration: number,
  sel: Range | null,
  cuts: Range[],
  slop = HIT,
): EdgeHit | null {
  const candidates: EdgeHit[] = [];
  if (sel) candidates.push({ target: "sel", index: -1, edge: "start" }, { target: "sel", index: -1, edge: "end" });
  cuts.forEach((_, index) => candidates.push({ target: "cut", index, edge: "start" }, { target: "cut", index, edge: "end" }));
  let best: EdgeHit | null = null;
  let bestDist = slop;
  for (const c of candidates) {
    const r = c.target === "sel" ? sel! : cuts[c.index];
    const dist = Math.abs(xAt(r[c.edge], width, duration) - x);
    if (dist <= bestDist && (!best || dist < bestDist)) {
      best = c;
      bestDist = dist;
    }
  }
  return best;
}

/** Move one edge to `t`, keeping the range inside the track and never inverted. */
export function dragTo(r: Range, edge: "start" | "end", t: number, duration: number): Range {
  return nudge(r, edge, t - r[edge], duration);
}

/** The range between two pointer positions, in either order. */
export function span(a: number, b: number): Range {
  return { start: Math.min(a, b), end: Math.max(a, b) };
}

/** Index of the range whose `edge` sits at `value`, or -1 (e.g. merged away). */
export function findRange(ranges: Range[], edge: "start" | "end", value: number): number {
  return ranges.findIndex((r) => Math.abs(r[edge] - value) < 1e-6);
}

const STEPS = [5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600];

/** Ruler labels: 0, round steps, and the end; at most `max` of them. */
export function ticks(duration: number, max = 6): number[] {
  if (!(duration > 0)) return [];
  const step = STEPS.find((s) => duration / s <= max - 1) ?? Math.ceil(duration / (max - 1));
  const out: number[] = [];
  for (let t = 0; t < duration - step / 2; t += step) out.push(t);
  out.push(duration);
  return out;
}

/**
 * Where playback at `t` must go: a later time when `t` is inside a removed range, `"end"` when
 * only removed audio is left, null to keep playing. `removed` must be normalized.
 */
export function playFrom(t: number, removed: Range[], duration: number): number | "end" | null {
  const to = skipTarget(t, removed);
  if (to === null) return null;
  return to >= duration - 0.05 ? "end" : to;
}

/**
 * A rejected `play()` that only means it was interrupted (a pause, a seek, a new source), not
 * that the audio can't play.
 */
export function interrupted(e: unknown): boolean {
  return typeof e === "object" && e !== null && (e as { name?: unknown }).name === "AbortError";
}

/** One value per bar: the loudest peak in that bar's slice. */
export function bars(peaks: number[], n: number): number[] {
  const out = new Array<number>(n).fill(0);
  if (!peaks.length) return out;
  for (let i = 0; i < n; i++) {
    const from = Math.floor((i * peaks.length) / n);
    const to = Math.max(from + 1, Math.floor(((i + 1) * peaks.length) / n));
    let m = 0;
    for (let j = from; j < to && j < peaks.length; j++) m = Math.max(m, peaks[j]);
    out[i] = m;
  }
  return out;
}
