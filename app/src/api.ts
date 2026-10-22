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
  gen: number;
  event: EngineEvent;
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
  enqueue: (inputs: string[]) => invoke<number>("enqueue", { inputs }),
  cancel: (generation: number, id: number) => invoke<void>("cancel", { generation, id }),
  reveal: (path: string) => invoke<void>("reveal", { path }),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  checkAppUpdate: () => invoke<NewVersion | null>("check_app_update"),
  readLog: () => invoke<string>("read_log"),
  onEngine: (fn: (p: EnginePayload) => void): Promise<UnlistenFn> =>
    listen<EnginePayload>("engine", (e) => fn(e.payload)),
  onTools: (fn: (s: ToolsStatus) => void): Promise<UnlistenFn> =>
    listen<ToolsStatus>("tools", (e) => fn(e.payload)),
};
