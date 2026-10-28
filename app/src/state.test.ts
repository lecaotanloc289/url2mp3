import { describe, expect, it } from "vitest";
import type { EngineEvent, TrayEvent } from "./api";
import { formatTime } from "./cut";
import { canOpen, empty, groupStats, lengthText, percent, ready, reduce, saveItem, summary, tag, type QueueState } from "./state";

const run = (events: EngineEvent[], state: QueueState = empty()) =>
  events.reduce((s, event) => reduce(s, { kind: "engine", event }), state);
const tray = (events: TrayEvent[], state: QueueState = empty()) =>
  events.reduce((s, event) => reduce(s, { kind: "tray", event }), state);

const added = (id: number, over: Partial<Extract<TrayEvent, { kind: "added" }>> = {}): TrayEvent => ({
  kind: "added",
  id,
  parent: null,
  input: "https://youtu.be/x",
  title: "Song",
  artist: "Band",
  thumbnail: null,
  duration: 272,
  youtube: true,
  ...over,
});

describe("single track", () => {
  const events: EngineEvent[] = [
    { kind: "probing", id: 0, parent: null, label: "https://youtu.be/x" },
    { kind: "track", id: 0, parent: null, title: "Song", artist: "Band", thumbnail: "https://i/x.jpg" },
    { kind: "progress", id: 0, done: 50, total: 200 },
  ];

  it("goes from probing to downloading with progress", () => {
    const s = run(events);
    expect(s.order).toEqual(["0"]);
    const row = s.rows["0"];
    expect(row).toMatchObject({
      input: "https://youtu.be/x",
      title: "Song",
      artist: "Band",
      status: "downloading",
      done: 50,
      total: 200,
    });
    expect(percent(row)).toBe(25);
  });

  it("ends done with the file path", () => {
    const s = run([
      ...events,
      { kind: "converting", id: 0 },
      { kind: "done", id: 0, path: "/m/Song.mp3", bytes: 300, secs: 1 },
    ]);
    expect(s.rows["0"]).toMatchObject({ status: "done", path: "/m/Song.mp3" });
    expect(percent(s.rows["0"])).toBe(100);
    expect(summary(s)).toEqual({ done: 1, failed: 0, active: 0 });
  });

  it("keeps the yt-dlp fallback visible", () => {
    const s = run([...events, { kind: "fallback", id: 0, reason: "HTTP 403" }]);
    expect(s.rows["0"].status).toBe("fallback");
  });
});

describe("playlist", () => {
  const s = run([
    { kind: "probing", id: 0, parent: null, label: "https://youtube.com/playlist?list=1" },
    { kind: "playlist", id: 0, title: "Chill", count: 3 },
    { kind: "probing", id: 1, parent: 0, label: "A" },
    { kind: "track", id: 1, parent: 0, title: "A", artist: null, thumbnail: null },
    { kind: "done", id: 1, path: "/m/Chill/A.mp3", bytes: 1, secs: 1 },
    { kind: "skipped", id: 2, parent: 0, title: "B", reason: { kind: "exists", path: "/m/Chill/B.mp3" } },
    { kind: "failed", id: 3, parent: 0, label: "C", message: "Video unavailable", network: false },
  ]);

  it("turns the root row into a group holding its tracks", () => {
    expect(s.order).toEqual(["0"]);
    expect(s.rows["0"]).toBeUndefined();
    const group = s.groups["0"];
    expect(group).toMatchObject({ title: "Chill", count: 3, input: "https://youtube.com/playlist?list=1" });
    expect(group.children).toEqual(["1", "2", "3"]);
    expect(s.rows["2"]).toMatchObject({ status: "exists", path: "/m/Chill/B.mp3", parent: "0" });
    expect(s.rows["3"]).toMatchObject({ status: "failed", message: "Video unavailable", title: "C" });
  });

  it("counts finished and failed tracks", () => {
    expect(groupStats(s, s.groups["0"])).toEqual({ finished: 2, failed: 1, count: 3 });
    expect(summary(s)).toEqual({ done: 2, failed: 1, active: 0 });
  });

  it("cancelling the playlist stops its unfinished tracks", () => {
    const t = run(
      [
        { kind: "probing", id: 4, parent: 0, label: "D" },
        { kind: "cancelled", id: 0 },
      ],
      s,
    );
    expect(t.groups["0"].cancelled).toBe(true);
    expect(t.rows["4"].status).toBe("cancelled");
    expect(t.rows["1"].status).toBe("done");
  });
});

describe("rows", () => {
  it("a duplicate is shown as skipped", () => {
    const s = run([
      { kind: "probing", id: 0, parent: null, label: "x" },
      { kind: "skipped", id: 0, parent: null, title: "Song", reason: { kind: "duplicate" } },
    ]);
    expect(s.rows["0"]).toMatchObject({ status: "duplicate", title: "Song" });
  });

  it("ignores events for rows it never saw", () => {
    const s = run([{ kind: "progress", id: 9, done: 1, total: 2 }]);
    expect(s).toEqual(empty());
  });

  it("cancel marks a running root", () => {
    const s = run([
      { kind: "probing", id: 0, parent: null, label: "x" },
      { kind: "cancelled", id: 0 },
    ]);
    expect(s.rows["0"].status).toBe("cancelled");
  });

  it("remove and clear finished", () => {
    let s = run([
      { kind: "probing", id: 0, parent: null, label: "a" },
      { kind: "probing", id: 1, parent: null, label: "b" },
      { kind: "failed", id: 1, parent: null, label: "b", message: "x", network: true },
      { kind: "probing", id: 2, parent: null, label: "c" },
      { kind: "done", id: 2, path: "/c.mp3", bytes: 1, secs: 1 },
    ]);
    s = reduce(s, { kind: "clearFinished" });
    expect(s.order).toEqual(["0", "1"]);
    s = reduce(s, { kind: "remove", key: "1" });
    expect(s.order).toEqual(["0"]);
    expect(s.rows["1"]).toBeUndefined();
  });
});

describe("tray", () => {
  const probing: EngineEvent = { kind: "probing", id: 0, parent: null, label: "https://youtu.be/x" };
  const one = () => tray([added(0)], run([probing]));

  it('"added" puts a ready row with duration and no cuts', () => {
    const s = one();
    expect(s.rows["0"]).toMatchObject({
      status: "ready",
      duration: 272,
      removed: [],
      input: "https://youtu.be/x",
      youtube: true,
    });
    expect(ready(s)).toEqual(["0"]);
  });

  it("playlist then children gives a group of ready rows", () => {
    const s = tray(
      [
        { kind: "playlist", id: 0, input: "https://youtube.com/playlist?list=1", title: "Chill", count: 3 },
        added(1, { parent: 0, input: null, title: "A" }),
        added(2, { parent: 0, input: null, title: "B" }),
        added(3, { parent: 0, input: null, title: "C" }),
      ],
      run([{ kind: "probing", id: 0, parent: null, label: "https://youtube.com/playlist?list=1" }]),
    );
    expect(s.rows["0"]).toBeUndefined();
    expect(s.groups["0"]).toMatchObject({ title: "Chill", count: 3, input: "https://youtube.com/playlist?list=1" });
    expect(s.groups["0"].children).toEqual(["1", "2", "3"]);
    expect(ready(s)).toEqual(["1", "2", "3"]);
  });

  it("a re-sent added keeps status, cuts and preview", () => {
    let s = tray([added(0, { title: "?", duration: null })]);
    s = reduce(s, { kind: "cut", key: "0", removed: [{ start: 1, end: 2 }] });
    s = run([{ kind: "progress", id: 0, done: 1, total: 4 }], s);
    s = tray([added(0, { input: null, title: "Full", duration: 100 })], s);
    expect(s.rows["0"]).toMatchObject({
      title: "Full",
      duration: 100,
      status: "ready",
      input: "https://youtu.be/x",
      removed: [{ start: 1, end: 2 }],
      preview: { phase: "loading", done: 1, total: 4 },
    });
  });

  it("cut stores removed ranges", () => {
    const s = reduce(one(), { kind: "cut", key: "0", removed: [{ start: 10, end: 20 }] });
    expect(s.rows["0"].removed).toEqual([{ start: 10, end: 20 }]);
  });

  it("progress on a ready row fills preview, status stays ready", () => {
    const s = run([{ kind: "progress", id: 0, done: 50, total: 200 }, { kind: "fallback", id: 0, reason: null }], one());
    expect(s.rows["0"].status).toBe("ready");
    expect(s.rows["0"].preview).toMatchObject({ phase: "loading", done: 50, total: 200 });
  });

  it("previewFailed keeps the row ready with the message", () => {
    const s = reduce(one(), { kind: "preview", key: "0", phase: "failed", message: "boom" });
    expect(s.rows["0"].status).toBe("ready");
    expect(s.rows["0"].preview).toMatchObject({ phase: "failed", message: "boom" });
    const t = reduce(s, { kind: "preview", key: "0", phase: "ready" });
    expect(t.rows["0"].preview).toMatchObject({ phase: "ready" });
    expect(t.rows["0"].preview?.message).toBeUndefined();
  });

  it("saving then done ends done with the path", () => {
    let s = reduce(one(), { kind: "saving", keys: ["0"] });
    expect(s.rows["0"]).toMatchObject({ status: "downloading", done: 0, total: 0 });
    expect(ready(s)).toEqual([]);
    s = run(
      [
        { kind: "progress", id: 0, done: 5, total: 10 },
        { kind: "converting", id: 0 },
        { kind: "done", id: 0, path: "/m/Song.mp3", bytes: 9, secs: 1 },
      ],
      s,
    );
    expect(s.rows["0"]).toMatchObject({ status: "done", path: "/m/Song.mp3" });
  });

  it("added without duration renders as unknown", () => {
    const s = tray([added(0, { duration: null })]);
    expect(s.rows["0"].duration).toBeUndefined();
    expect(formatTime(s.rows["0"].duration ?? NaN)).toBe("–:––");
  });

  it("ready rows are not active, and can be removed", () => {
    let s = one();
    expect(summary(s).active).toBe(0);
    s = reduce(s, { kind: "remove", key: "0" });
    expect(s.order).toEqual([]);
  });

  it("cancelled leaves a ready row ready", () => {
    const s = run([{ kind: "cancelled", id: 0 }], one());
    expect(s.rows["0"].status).toBe("ready");
  });

  it("saving marks rows saved, and a failed saved row keeps its cuts and can be saved again", () => {
    let s = reduce(one(), { kind: "cut", key: "0", removed: [{ start: 10, end: 20 }] });
    s = reduce(s, { kind: "saving", keys: ["0"] });
    expect(s.rows["0"].saved).toBe(true);
    s = run([{ kind: "failed", id: 0, parent: null, label: "x", message: "net", network: true }], s);
    expect(s.rows["0"]).toMatchObject({ status: "failed", removed: [{ start: 10, end: 20 }], saved: true });
    s = reduce(s, { kind: "saving", keys: ["0"] });
    expect(s.rows["0"]).toMatchObject({ status: "downloading", removed: [{ start: 10, end: 20 }] });
  });

  it("saveItem: no cuts is whole, cuts become kept ranges, cuts without duration are whole", () => {
    const row = one().rows["0"];
    expect(saveItem(row, "trimmed")).toEqual({ id: 0, cut: null });
    const cut = reduce(one(), { kind: "cut", key: "0", removed: [{ start: 10, end: 20 }], duration: 272 }).rows["0"];
    expect(saveItem(cut, "trimmed")).toEqual({
      id: 0,
      cut: { keep: [{ start: 0, end: 10 }, { start: 20, end: 272 }], label: "trimmed" },
    });
    expect(saveItem({ ...cut, duration: undefined }, "trimmed").cut).toBeNull();
  });
});

describe("tray rows in the list", () => {
  const one = () => tray([added(0)]);
  const segs = [
    { start: 0, end: 28, category: "intro" },
    { start: 245, end: 272, category: "outro" },
  ];

  it("shows the length, or the length before and after cuts", () => {
    expect(lengthText(one().rows["0"])).toBe("4:32");
    const cut = reduce(one(), { kind: "cut", key: "0", removed: [{ start: 0, end: 28 }, { start: 98, end: 128 }, { start: 245, end: 272 }], duration: 272 });
    expect(lengthText(cut.rows["0"])).toBe("4:32 → 3:07");
    expect(lengthText(tray([added(0, { duration: null })]).rows["0"])).toBe("–:––");
  });

  it("rounds the kept length instead of truncating float sums", () => {
    const s = reduce(one(), { kind: "cut", key: "0", removed: [{ start: 0, end: 28.004 }, { start: 245.003, end: 272 }], duration: 272 });
    // kept = 216.999…: truncating would show 3:36
    expect(lengthText(s.rows["0"])).toBe("4:32 → 3:37");
  });

  it("tags: whole, then suggested once prepare found parts, then removed", () => {
    let s = one();
    expect(tag(s.rows["0"])).toEqual({ kind: "whole" });
    s = reduce(s, { kind: "suggestions", key: "0", segments: segs });
    expect(tag(s.rows["0"])).toEqual({ kind: "suggested", secs: 55 });
    s = reduce(s, { kind: "cut", key: "0", removed: [{ start: 0, end: 28 }], duration: 272 });
    expect(tag(s.rows["0"])).toEqual({ kind: "removed", n: 1 });
  });

  it("suggestions are stored once; handling them clears them for good", () => {
    let s = reduce(one(), { kind: "suggestions", key: "0", segments: segs });
    s = reduce(s, { kind: "suggestionsHandled", key: "0" });
    expect(s.rows["0"].suggested).toEqual([]);
    s = reduce(s, { kind: "suggestions", key: "0", segments: segs });
    expect(s.rows["0"].suggested).toEqual([]);
    expect(tag(s.rows["0"])).toEqual({ kind: "whole" });
  });

  it("a rejected save puts the rows back to failed with the message", () => {
    let s = reduce(one(), { kind: "saving", keys: ["0"] });
    s = reduce(s, { kind: "saveFailed", keys: ["0", "9"], message: "engine gone" });
    expect(s.rows["0"]).toMatchObject({ status: "failed", message: "engine gone", network: false, saved: true });
    expect(s.rows["9"]).toBeUndefined();
  });

  it("a whole save is retried whole even when the row has cuts", () => {
    let s = reduce(one(), { kind: "cut", key: "0", removed: [{ start: 0, end: 28 }], duration: 272 });
    s = reduce(s, { kind: "saving", keys: ["0"], whole: true });
    expect(saveItem(s.rows["0"], "trimmed").cut).toBeNull();
    s = run([{ kind: "failed", id: 0, parent: null, label: "x", message: "m", network: false }], s);
    s = reduce(s, { kind: "saving", keys: ["0"] });
    expect(saveItem(s.rows["0"], "trimmed").cut).not.toBeNull();
  });

  it("a saved row that finished can be opened and saved again", () => {
    let s = one();
    expect(canOpen(s.rows["0"])).toBe(true);
    s = reduce(s, { kind: "saving", keys: ["0"] });
    expect(canOpen(s.rows["0"])).toBe(false);
    s = run([{ kind: "done", id: 0, path: "/m/Song.mp3", bytes: 9, secs: 1 }], s);
    expect(canOpen(s.rows["0"])).toBe(true);
    s = reduce(s, { kind: "saving", keys: ["0"] });
    expect(s.rows["0"].status).toBe("downloading");
    // A row that never came through the tray has no cache to open.
    const plain = run([{ kind: "probing", id: 5, parent: null, label: "x" }, { kind: "done", id: 5, path: "/x", bytes: 1, secs: 1 }]);
    expect(canOpen(plain.rows["5"])).toBe(false);
  });

  it("removing a playlist track drops it from its group; the last one takes the group", () => {
    let s = tray([
      { kind: "playlist", id: 0, input: "p", title: "Chill", count: 2 },
      added(1, { parent: 0, input: null }),
      added(2, { parent: 0, input: null }),
    ]);
    s = reduce(s, { kind: "remove", key: "1" });
    expect(s.rows["1"]).toBeUndefined();
    expect(s.groups["0"]).toMatchObject({ children: ["2"], count: 1 });
    expect(ready(s)).toEqual(["2"]);
    s = reduce(s, { kind: "remove", key: "2" });
    expect(s.groups["0"]).toBeUndefined();
    expect(s.order).toEqual([]);
  });
});
