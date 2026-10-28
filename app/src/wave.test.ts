import { describe, expect, it } from "vitest";
import { bars, dragTo, findRange, hitEdge, interrupted, playFrom, span, ticks, timeAt, xAt } from "./wave";

describe("wave geometry", () => {
  it("maps pointer x to time and back, clamped to the track", () => {
    expect(timeAt(200, 400, 272)).toBe(136);
    expect(timeAt(-20, 400, 272)).toBe(0);
    expect(timeAt(450, 400, 272)).toBe(272);
    expect(timeAt(10, 0, 272)).toBe(0);
    expect(xAt(136, 400, 272)).toBe(200);
  });

  it("finds the edge under the pointer within a 24 px hit area, selection first", () => {
    // 400 px for 400 s: 1 px per second.
    const cuts = [{ start: 0, end: 28 }, { start: 100, end: 130 }];
    expect(hitEdge(39, 400, 400, null, cuts)).toEqual({ target: "cut", index: 0, edge: "end" });
    expect(hitEdge(41, 400, 400, null, cuts)).toBeNull();
    expect(hitEdge(95, 400, 400, null, cuts)).toEqual({ target: "cut", index: 1, edge: "start" });
    expect(hitEdge(129, 400, 400, { start: 128, end: 200 }, cuts)).toEqual({ target: "sel", index: -1, edge: "start" });
    // Nearest edge wins.
    expect(hitEdge(127, 400, 400, null, cuts)).toEqual({ target: "cut", index: 1, edge: "end" });
  });

  it("dragging an edge never inverts the range", () => {
    expect(dragTo({ start: 10, end: 20 }, "end", 15, 272)).toEqual({ start: 10, end: 15 });
    expect(dragTo({ start: 10, end: 20 }, "end", 5, 272)).toEqual({ start: 10, end: 10.05 });
    expect(dragTo({ start: 10, end: 20 }, "start", -3, 272)).toEqual({ start: 0, end: 20 });
    expect(span(30, 12)).toEqual({ start: 12, end: 30 });
  });

  it("finds a range again after normalize sorted or merged it", () => {
    const ranges = [{ start: 0, end: 28 }, { start: 98, end: 128 }];
    expect(findRange(ranges, "end", 128)).toBe(1);
    expect(findRange(ranges, "start", 50)).toBe(-1);
  });

  it("ruler ticks: whole minutes for a song, and the end", () => {
    expect(ticks(272)).toEqual([0, 60, 120, 180, 240, 272]);
    expect(ticks(40)).toEqual([0, 10, 20, 30, 40]);
    // A tick too close to the end gives way to the end label.
    expect(ticks(245)).toEqual([0, 60, 120, 180, 245]);
    expect(ticks(0)).toEqual([]);
  });

  it("playback jumps over removed parts and stops when only removed audio is left", () => {
    const removed = [{ start: 0, end: 28 }, { start: 245, end: 272 }];
    expect(playFrom(0, removed, 272)).toBe(28);
    expect(playFrom(100, removed, 272)).toBeNull();
    expect(playFrom(250, removed, 272)).toBe("end");
  });

  it("tells an interrupted play() from one that failed", () => {
    expect(interrupted(new DOMException("The play() request was interrupted by a call to pause()", "AbortError"))).toBe(true);
    expect(interrupted(new DOMException("no supported source", "NotSupportedError"))).toBe(false);
    expect(interrupted(new Error("boom"))).toBe(false);
    expect(interrupted("AbortError")).toBe(false);
    expect(interrupted(null)).toBe(false);
  });

  it("reduces peaks to one bar per slot, by maximum", () => {
    expect(bars([0.1, 0.5, 0.2, 0.9], 2)).toEqual([0.5, 0.9]);
    expect(bars([0.3, 0.6], 4)).toEqual([0.3, 0.3, 0.6, 0.6]);
    expect(bars([], 3)).toEqual([0, 0, 0]);
  });
});
