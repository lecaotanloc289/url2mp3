// Runs the UI in a plain browser with a fake backend, for looking at layouts without Tauri:
// `pnpm dev`, then open http://localhost:1420/dev/mock.html (add `#open` to expand playlists).

import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import type { EngineEvent, Settings } from "../src/api";

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

mockIPC(
  (cmd) => {
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
      default:
        return null;
    }
  },
  { shouldMockEvents: true },
);

// The app's markup lives in index.html: borrow its body.
const page = await (await fetch("/index.html")).text();
document.body.innerHTML = new DOMParser().parseFromString(page, "text/html").body.innerHTML;
await import("../src/main");

const thumb = "https://i.ytimg.com/vi/Llw9Q6akRo4/hqdefault.jpg";
const events: EngineEvent[] = [
  { kind: "probing", id: 0, parent: null, label: "https://www.youtube.com/watch?v=Llw9Q6akRo4" },
  { kind: "track", id: 0, parent: null, title: "LẠC TRÔI | OFFICIAL MUSIC VIDEO | SƠN TÙNG M-TP", artist: "Sơn Tùng M-TP Official", thumbnail: thumb },
  { kind: "progress", id: 0, done: 5_400_000, total: 8_600_000 },
  { kind: "probing", id: 1, parent: null, label: "https://www.youtube.com/playlist?list=PL1" },
  { kind: "playlist", id: 1, title: "Popular Music Videos", count: 4 },
  { kind: "probing", id: 2, parent: 1, label: "BELLAKEO (Video Oficial) - Peso Pluma, Anitta" },
  { kind: "track", id: 2, parent: 1, title: "BELLAKEO (Video Oficial) - Peso Pluma, Anitta", artist: "Peso Pluma", thumbnail: thumb },
  { kind: "done", id: 2, path: "/Users/me/Downloads/Popular Music Videos/BELLAKEO.mp3", bytes: 3_800_000, secs: 1.2 },
  { kind: "probing", id: 3, parent: 1, label: "Arcángel - FN8 ( Video Lyric )" },
  { kind: "track", id: 3, parent: 1, title: "Arcángel - FN8 ( Video Lyric )", artist: "Arcángel", thumbnail: null },
  { kind: "converting", id: 3 },
  { kind: "probing", id: 4, parent: 1, label: "Track that will fail" },
  { kind: "failed", id: 4, parent: 1, label: "Track that will fail", message: "Video unavailable. This video is private", network: false },
  { kind: "probing", id: 5, parent: null, label: "never gonna give you up" },
  { kind: "skipped", id: 5, parent: null, title: "Rick Astley - Never Gonna Give You Up", reason: { kind: "exists", path: "/Users/me/Downloads/Rick.mp3" } },
  { kind: "probing", id: 6, parent: null, label: "https://www.tiktok.com/@x/video/1" },
  { kind: "failed", id: 6, parent: null, label: "https://www.tiktok.com/@x/video/1", message: "Sign in to confirm you're not a bot", network: false },
];
// Spaced out so the UI renders between events, as with a real backend (a row is drawn before
// it turns into a playlist group).
for (const event of events) {
  await emit("engine", { gen: 1, event });
  await new Promise((r) => setTimeout(r, 40));
}

if (location.hash === "#open") {
  setTimeout(() => document.querySelector<HTMLElement>(".group > .meta")?.click(), 100);
}
