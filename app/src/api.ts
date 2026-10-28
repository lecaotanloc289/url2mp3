// Typed wrappers around the Tauri commands and events in app/src-tauri/src/lib.rs.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Format = "mp3" | "m4a" | "opus";
export type Language = "vi" | "en";

export type Skip = { kind: "duplicate" } | { kind: "exists"; path: string };

/** url2mp3_core::Event, plus `cancelled` which the app adds. */
export type EngineEvent =
  | { kind: "probing"; id: number; parent: number | null; label: string }
  | { kind: "playlist"; id: number; title: string; count: number }
  | {
      kind: "track";
      id: number;
      parent: number | null;
      title: string;
      artist: string | null;
      thumbnail: string | null;
    }
  | { kind: "progress"; id: number; done: number; total: number }
  | { kind: "fallback"; id: number; reason: string | null }
  | { kind: "converting"; id: number }
  | { kind: "done"; id: number; path: string; bytes: number; secs: number }
  | { kind: "skipped"; id: number; parent: number | null; title: string; reason: Skip }
  | {
      kind: "failed";
      id: number;
      parent: number | null;
      label: string;
      message: string;
      network: boolean;
    }
  | { kind: "cancelled"; id: number };

export interface EnginePayload {
  event: EngineEvent;
}

/** The tray: what `add` found, before anything is saved. */
export type TrayEvent =
  | {
      kind: "added";
      id: number;
      parent: number | null;
      /** What the user typed; null for playlist tracks and for a re-send after `prepare`. */
      input: string | null;
      title: string;
      artist: string | null;
      thumbnail: string | null;
      duration: number | null;
      youtube: boolean;
    }
  | { kind: "playlist"; id: number; input: string; title: string; count: number };

export interface Segment {
  start: number;
  end: number;
  category: string;
}

export interface Prepared {
  duration: number | null;
  /** 1200 buckets, 0..1. */
  peaks: number[];
  suggestions: Segment[];
  /** The whole-file destination already exists. */
  exists: boolean;
}

/** A kept part as sent to `save`. `end: null` runs to the end of the audio, whatever its exact length. */
export interface KeepRange {
  start: number;
  end: number | null;
}

export interface SaveItem {
  id: number;
  cut: { keep: KeepRange[]; label: string } | null;
}

export type ToolsStatus =
  | { phase: "missing" }
  | { phase: "downloading"; version: string }
  | { phase: "ready"; version: string }
  | { phase: "error"; message: string; version: string | null };

export interface Settings {
  format: Format;
  bitrate: number | null;
  outDir: string | null;
  jobs: number;
  connections: number;
  playlistLimit: number | null;
  cover: boolean;
  cookiesBrowser: string | null;
  language: Language | null;
}

export interface NewVersion {
  version: string;
  url: string;
}

export const api = {
  getSettings: () => invoke<Settings>("get_settings"),
  setSettings: (settings: Settings) => invoke<void>("set_settings", { settings }),
  defaultOutDir: () => invoke<string | null>("default_out_dir"),
  toolsStatus: () => invoke<ToolsStatus>("tools_status"),
  updateYtdlp: (force: boolean) => invoke<ToolsStatus>("update_ytdlp", { force }),
  add: (inputs: string[]) => invoke<number>("add", { inputs }),
  prepare: (id: number) => invoke<Prepared>("prepare", { id }),
  /** AAC in MP4: wrap in a Blob of type audio/mp4. */
  previewAudio: (id: number) => invoke<ArrayBuffer>("preview_audio", { id }),
  save: (items: SaveItem[]) => invoke<void>("save", { items }),
  discard: (ids: number[]) => invoke<void>("discard", { ids }),
  cancel: (id: number) => invoke<void>("cancel", { id }),
  reveal: (path: string) => invoke<void>("reveal", { path }),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  checkAppUpdate: () => invoke<NewVersion | null>("check_app_update"),
  readLog: () => invoke<string>("read_log"),
  platform: () => invoke<"android" | "desktop">("platform"),
  keepAlive: (active: boolean, text: string) => invoke<void>("keep_alive", { active, text }),
  takeShared: () => invoke<string | null>("take_shared"),
  onEngine: (fn: (p: EnginePayload) => void): Promise<UnlistenFn> =>
    listen<EnginePayload>("engine", (e) => fn(e.payload)),
  onTray: (fn: (e: TrayEvent) => void): Promise<UnlistenFn> =>
    listen<TrayEvent>("tray", (e) => fn(e.payload)),
  onTools: (fn: (s: ToolsStatus) => void): Promise<UnlistenFn> =>
    listen<ToolsStatus>("tools", (e) => fn(e.payload)),
};
