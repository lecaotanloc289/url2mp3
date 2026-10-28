// Runs the UI in a plain browser with a fake backend, for looking at layouts without Tauri:
// `pnpm dev`, then open http://localhost:1420/dev/mock.html (add `#open` to expand playlists).
// Inputs containing "playlist" give a 3-track playlist; "noduration" a track yt-dlp reports no
// length for; "fail" a track whose first preview fails.

import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import type { EngineEvent, Prepared, SaveItem, Settings, TrayEvent } from "../src/api";

const settings: Settings = {
  format: "mp3",
  bitrate: null,
  outDir: null,
  jobs: 8,
  connections: 8,
  playlistLimit: null,
  cover: true,
  cookiesBrowser: null,
  language: "vi",
};

const DURATION = 272;
const thumb = "https://i.ytimg.com/vi/Llw9Q6akRo4/hqdefault.jpg";
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const engine = (event: EngineEvent) => emit("engine", { event });
const tray = (event: TrayEvent) => emit("tray", event);

let nextId = 0;
const titles = new Map<number, string>();
const parents = new Map<number, number>();
const noDuration = new Set<number>();
const failOnce = new Set<number>();

/** A 272 s mono 8 kHz quiet sine (WAV), built once. The real backend returns AAC in MP4. */
let wav: ArrayBuffer | undefined;
function previewWav(): ArrayBuffer {
  if (wav) return wav;
  const rate = 8000;
  const n = rate * DURATION;
  const buf = new ArrayBuffer(44 + n * 2);
  const v = new DataView(buf);
  const text = (at: number, s: string) => [...s].forEach((c, i) => v.setUint8(at + i, c.charCodeAt(0)));
  text(0, "RIFF");
  v.setUint32(4, 36 + n * 2, true);
  text(8, "WAVEfmt ");
  v.setUint32(16, 16, true);
  v.setUint16(20, 1, true);
  v.setUint16(22, 1, true);
  v.setUint32(24, rate, true);
  v.setUint32(28, rate * 2, true);
  v.setUint16(32, 2, true);
  v.setUint16(34, 16, true);
  text(36, "data");
  v.setUint32(40, n * 2, true);
  for (let i = 0; i < n; i++) v.setInt16(44 + i * 2, Math.round(Math.sin((2 * Math.PI * 220 * i) / rate) * 1500), true);
  return (wav = buf);
}

function peaks(): number[] {
  return Array.from({ length: 1200 }, (_, i) => {
    const x = i / 1200;
    const swell = 0.35 + 0.45 * Math.abs(Math.sin(x * Math.PI * 7));
    return Math.min(1, Math.max(0.03, swell * (0.75 + 0.25 * Math.sin(i * 1.7))));
  });
}

async function add(inputs: string[]): Promise<number> {
  const clean = inputs.map((s) => s.trim()).filter(Boolean);
  void (async () => {
    for (const input of clean) {
      const id = nextId++;
      await engine({ kind: "probing", id, parent: null, label: input });
      await sleep(300);
      if (input.includes("playlist")) {
        await tray({ kind: "playlist", id, input, title: "Popular Music Videos", count: 3 });
        for (const name of ["BELLAKEO - Peso Pluma, Anitta", "Arcángel - FN8 (Video Lyric)", "Track without duration"]) {
          const child = nextId++;
          titles.set(child, name);
          parents.set(child, id);
          await tray({ kind: "added", id: child, parent: id, input: null, title: name, artist: null, thumbnail: child % 2 ? thumb : null, duration: null, youtube: true });
        }
      } else {
        const title = "LẠC TRÔI | OFFICIAL MUSIC VIDEO | SƠN TÙNG M-TP";
        titles.set(id, title);
        if (input.includes("noduration")) noDuration.add(id);
        if (input.includes("fail")) failOnce.add(id);
        const duration = noDuration.has(id) ? null : DURATION;
        await tray({ kind: "added", id, parent: null, input, title, artist: "Sơn Tùng M-TP Official", thumbnail: thumb, duration, youtube: true });
      }
    }
  })();
  return clean.length;
}

async function prepare(id: number): Promise<Prepared> {
  const parent = parents.get(id);
  if (parent !== undefined) {
    await tray({ kind: "added", id, parent, input: null, title: titles.get(id) ?? "", artist: null, thumbnail: null, duration: DURATION, youtube: true });
  }
  for (let i = 1; i <= 4; i++) {
    await engine({ kind: "progress", id, done: i * 100_000, total: 400_000 });
    await sleep(120);
  }
  if (failOnce.delete(id)) throw "fetch: HTTP Error 403: Forbidden";
  return {
    duration: noDuration.has(id) ? null : DURATION,
    peaks: peaks(),
    // The spec's example: 0:00–0:28 intro, 4:05–4:32 outro → 3:37 kept.
    suggestions: [
      { start: 0, end: 28, category: "intro" },
      { start: 245, end: 272, category: "outro" },
    ],
    exists: id === 0,
  };
}

async function save(items: SaveItem[]): Promise<null> {
  void (async () => {
    for (const { id } of items) {
      const title = titles.get(id) ?? "Track";
      const parent = parents.get(id) ?? null;
      await engine({ kind: "track", id, parent, title, artist: null, thumbnail: null });
      for (let i = 1; i <= 5; i++) {
        await engine({ kind: "progress", id, done: i * 700_000, total: 3_500_000 });
        await sleep(150);
      }
      await engine({ kind: "converting", id });
      await sleep(300);
      await engine({ kind: "done", id, path: `/Users/me/Downloads/${title}.mp3`, bytes: 3_500_000, secs: 1.4 });
    }
  })();
  return null;
}

mockIPC(
  (cmd, args) => {
    const a = (args ?? {}) as Record<string, any>;
    switch (cmd) {
      case "get_settings":
        return settings;
      case "tools_status":
      case "update_ytdlp":
        return { phase: "ready", version: "2026.10.21" };
      case "check_app_update":
        return { version: "0.2.0", url: "https://github.com/lecaotanloc289/url2mp3/releases" };
      case "default_out_dir":
        return "/Users/me/Downloads";
      case "add":
        return add(a.inputs);
      case "prepare":
        return prepare(a.id);
      case "preview_audio":
        return previewWav();
      case "save":
        return save(a.items);
      default:
        // discard, cancel, ...
        return null;
    }
  },
  { shouldMockEvents: true },
);

// The app's markup lives in index.html: borrow its body.
const page = await (await fetch("/index.html")).text();
document.body.innerHTML = new DOMParser().parseFromString(page, "text/html").body.innerHTML;
await import("../src/main");

// Seed the list: a link and a playlist in the tray, plus rows in the states a save ends in.
await add(["https://www.youtube.com/watch?v=Llw9Q6akRo4", "https://www.youtube.com/playlist?list=PL1"]);
await sleep(1500);
const finished: EngineEvent[] = [
  { kind: "probing", id: 900, parent: null, label: "never gonna give you up" },
  { kind: "skipped", id: 900, parent: null, title: "Rick Astley - Never Gonna Give You Up", reason: { kind: "exists", path: "/Users/me/Downloads/Rick.mp3" } },
  { kind: "probing", id: 901, parent: null, label: "https://www.tiktok.com/@x/video/1" },
  { kind: "failed", id: 901, parent: null, label: "https://www.tiktok.com/@x/video/1", message: "Sign in to confirm you're not a bot", network: false },
];
for (const event of finished) {
  await engine(event);
  await sleep(40);
}

if (location.hash === "#open") {
  setTimeout(() => document.querySelector<HTMLElement>(".group > .meta")?.click(), 100);
}
