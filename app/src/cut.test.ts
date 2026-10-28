import { describe, expect, it } from "vitest";
import { History, formatTime, kept, keptLength, nudge, normalize, skipTarget } from "./cut";

describe("kept", () => {
  it("keeps the complement and its length", () => {
    const removed = [{ start: 245, end: 280 }, { start: 0, end: 28 }, { start: 98, end: 128 }];
    expect(kept(removed, 272)).toEqual([{ start: 28, end: 98 }, { start: 128, end: 245 }]);
    expect(keptLength(removed, 272)).toBe(187); // 3:07
    expect(formatTime(187)).toBe("3:07");
    expect(formatTime(3725)).toBe("1:02:05");
    expect(formatTime(NaN)).toBe("–:––");
  });

  it("removing everything keeps nothing", () => {
    expect(kept([{ start: 0, end: 272 }], 272)).toEqual([]);
  });

  it("drops kept slivers shorter than 0.05 s", () => {
    expect(kept([{ start: 0, end: 9.98 }], 10)).toEqual([]);
    expect(kept([{ start: 0, end: 9.9 }], 10)).toEqual([{ start: 9.9, end: 10 }]);
  });
});

describe("normalize", () => {
  it("clamps, drops slivers, sorts and merges overlapping or touching ranges", () => {
    const removed = [
      { start: 50, end: 60 },
      { start: 55, end: 70 },
      { start: 70, end: 75 },
      { start: 90, end: 90.01 },
      { start: 95, end: 400 },
      { start: -5, end: 3 },
    ];
    expect(normalize(removed, 100)).toEqual([
      { start: 0, end: 3 },
      { start: 50, end: 75 },
      { start: 95, end: 100 },
    ]);
  });
});

describe("skipTarget", () => {
  it("preview jumps over removed parts", () => {
    expect(skipTarget(10, [{ start: 0, end: 28 }])).toBe(28);
    expect(skipTarget(30, [{ start: 0, end: 28 }])).toBeNull();
  });
});

describe("nudge", () => {
  it("nudge never inverts a range", () => {
    expect(nudge({ start: 10, end: 10.5 }, "start", 1, 272)).toEqual({ start: 10.45, end: 10.5 });
    expect(nudge({ start: 0.4, end: 5 }, "start", -1, 272)).toEqual({ start: 0, end: 5 });
  });

  it("clamps the end edge to the duration and keeps it after the start", () => {
    expect(nudge({ start: 10, end: 200 }, "end", 100, 272)).toEqual({ start: 10, end: 272 });
    expect(nudge({ start: 10, end: 20 }, "end", -100, 272)).toEqual({ start: 10, end: 10.05 });
  });
});

describe("formatTime", () => {
  it("formats m:ss and h:mm:ss, unknown durations as a placeholder", () => {
    expect(formatTime(0)).toBe("0:00");
    expect(formatTime(65.9)).toBe("1:05");
    expect(formatTime(Infinity)).toBe("–:––");
  });
});

describe("History", () => {
  it("undo walks back", () => {
    const h = new History();
    h.push([{ start: 0, end: 1 }]);
    h.push([]);
    expect(h.undo()).toEqual([{ start: 0, end: 1 }]);
    expect(h.undo()).toEqual([]);
    expect(h.undo()).toBeNull();
  });

  it("current is the last pushed state", () => {
    const h = new History();
    expect(h.current()).toEqual([]);
    h.push([{ start: 1, end: 2 }]);
    expect(h.current()).toEqual([{ start: 1, end: 2 }]);
  });
});
