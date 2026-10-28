// The queue as the UI sees it: a pure reducer over engine events and user actions.
// Rows and playlist groups are keyed by `String(id)`; the backend never reuses an id.
// A link first becomes a `ready` row (the tray); only an explicit Download turns it into a save.

import type { EngineEvent, SaveItem, TrayEvent } from "./api";
import { kept, normalize, type Range } from "./cut";

export type Status =
  | "probing"
  | "ready"
  | "downloading"
  | "fallback"
  | "converting"
  | "done"
  | "exists"
  | "duplicate"
  | "failed"
  | "cancelled";

export interface Row {
  key: string;
  id: number;
  /** Group key for a playlist track. */
  parent?: string;
  /** What the user typed: only on top-level rows, used by Retry. */
  input?: string;
  title: string;
  artist?: string;
  thumbnail?: string;
  status: Status;
  done: number;
  total: number;
  path?: string;
  message?: string;
  network?: boolean;
  /** Seconds; undefined when yt-dlp did not report it. */
  duration?: number;
  youtube?: boolean;
  /** Ranges the user cut out; empty means keep everything. */
  removed: Range[];
  /** Set once the row was saved from the tray: Retry then saves it again instead of re-adding. */
  saved?: boolean;
  /** Progress of the trim preview (fetch + encode) while the row is ready. */
  preview?: { phase: "loading" | "ready" | "failed"; done: number; total: number; message?: string };
}

export interface Group {
  key: string;
  id: number;
  input: string;
  title: string;
  count: number;
  children: string[];
  cancelled?: boolean;
}

export interface QueueState {
  rows: Record<string, Row>;
  groups: Record<string, Group>;
  /** Top-level rows and groups, oldest first. */
  order: string[];
}

export type Msg =
  | { kind: "engine"; event: EngineEvent }
  | { kind: "tray"; event: TrayEvent }
  | { kind: "cut"; key: string; removed: Range[]; duration?: number }
  | { kind: "preview"; key: string; phase: "loading" | "ready" | "failed"; message?: string }
  | { kind: "saving"; keys: string[] }
  | { kind: "remove"; key: string }
  | { kind: "clearFinished" };

export const empty = (): QueueState => ({ rows: {}, groups: {}, order: [] });

const FINISHED: ReadonlySet<Status> = new Set(["done", "exists", "duplicate", "failed", "cancelled"]);
export const isFinished = (s: Status) => FINISHED.has(s);
const isSuccess = (s: Status) => s === "done" || s === "exists" || s === "duplicate";

/** Keys of the rows waiting to be saved (playlist children included), in list order. */
export function ready(state: QueueState): string[] {
  const keys: string[] = [];
  for (const key of state.order) {
    const group = state.groups[key];
    for (const k of group ? group.children : [key]) {
      if (state.rows[k]?.status === "ready") keys.push(k);
    }
  }
  return keys;
}

/**
 * The `save` item for a row. Cuts need the duration to turn removed ranges into kept ones: the
 * trim dialog always stores the duration it learned from the audio on the row (`cut` Msg), so a
 * row without one has never been trimmed and is saved whole.
 */
export function saveItem(row: Row, label: string): SaveItem {
  const duration = row.duration;
  if (duration === undefined || normalize(row.removed, duration).length === 0) return { id: row.id, cut: null };
  return { id: row.id, cut: { keep: kept(row.removed, duration), label } };
}

export function percent(row: Row): number {
  if (row.status === "done") return 100;
  return row.total > 0 ? Math.min(100, Math.round((row.done / row.total) * 100)) : 0;
}

export function groupStats(state: QueueState, group: Group) {
  const rows = group.children.map((k) => state.rows[k]).filter(Boolean);
  return {
    finished: rows.filter((r) => isSuccess(r.status)).length,
    failed: rows.filter((r) => r.status === "failed").length,
    count: group.count,
  };
}

export function summary(state: QueueState) {
  const rows = Object.values(state.rows);
  return {
    done: rows.filter((r) => isSuccess(r.status)).length,
    failed: rows.filter((r) => r.status === "failed").length,
    active: rows.filter((r) => !isFinished(r.status) && r.status !== "ready").length,
  };
}

export function reduce(state: QueueState, msg: Msg): QueueState {
  switch (msg.kind) {
    case "engine":
      return onEvent(state, msg.event);
    case "tray":
      return onTray(state, msg.event);
    case "cut":
      return update(state, msg.key, (r) => ({ ...r, removed: msg.removed, duration: msg.duration ?? r.duration }));
    case "preview":
      return update(state, msg.key, (r) => ({
        ...r,
        preview: { done: r.preview?.done ?? 0, total: r.preview?.total ?? 0, phase: msg.phase, message: msg.message },
      }));
    case "saving": {
      let next = state;
      for (const key of msg.keys) {
        next = update(next, key, (r) =>
          r.status === "ready" || (r.saved && (r.status === "failed" || r.status === "cancelled"))
            ? { ...r, status: "downloading", done: 0, total: 0, message: undefined, saved: true } : r,
        );
      }
      return next;
    }
    case "remove":
      return removeTop(state, msg.key);
    case "clearFinished": {
      let next = state;
      for (const key of state.order) {
        const row = state.rows[key];
        const group = state.groups[key];
        const finished = row
          ? isSuccess(row.status)
          : group && groupStats(state, group).finished === group.count;
        if (finished) next = removeTop(next, key);
      }
      return next;
    }
  }
}

/** A playlist's probing row becomes its group, whichever event announces it. */
function toGroup(state: QueueState, key: string, id: number, title: string, count: number, input?: string): QueueState {
  const row = state.rows[key];
  if (!row && !input) return state;
  const rows = { ...state.rows };
  delete rows[key];
  const group: Group = { key, id, input: input ?? row?.input ?? row?.title ?? "", title, count, children: [] };
  const order = state.order.includes(key) ? state.order : [...state.order, key];
  return { ...state, rows, groups: { ...state.groups, [key]: group }, order };
}

function onTray(state: QueueState, e: TrayEvent): QueueState {
  const key = String(e.id);
  if (e.kind === "playlist") return toGroup(state, key, e.id, e.title, e.count, e.input);
  const fields = {
    title: e.title,
    artist: e.artist ?? undefined,
    thumbnail: e.thumbnail ?? undefined,
    duration: e.duration ?? undefined,
    youtube: e.youtube,
  };
  const row = state.rows[key];
  if (!row) return addRow(state, e.id, e.parent, { ...fields, status: "ready" }, e.input ?? undefined);
  // A re-send after `prepare` probed a playlist track: refresh in place, keep status, cuts, preview.
  return putRow(state, {
    ...row,
    ...fields,
    input: row.input ?? e.input ?? undefined,
    status: row.status === "probing" ? "ready" : row.status,
  });
}

function onEvent(state: QueueState, e: EngineEvent): QueueState {
  const key = String(e.id);
  switch (e.kind) {
    case "probing": {
      const row = state.rows[key];
      if (row) return row.status === "ready" ? state : putRow(state, { ...row, status: "probing" });
      return addRow(state, e.id, e.parent, { title: e.label, status: "probing" }, e.label);
    }
    case "playlist":
      return toGroup(state, key, e.id, e.title, e.count);
    case "track": {
      const fields = {
        title: e.title,
        artist: e.artist ?? undefined,
        thumbnail: e.thumbnail ?? undefined,
        status: "downloading" as Status,
        done: 0,
        total: 0,
      };
      const row = state.rows[key];
      return row ? putRow(state, { ...row, ...fields }) : addRow(state, e.id, e.parent, fields);
    }
    case "progress":
      return update(state, key, (r) =>
        r.status === "ready"
          ? { ...r, preview: { phase: "loading", done: e.done, total: e.total } }
          : { ...r, done: e.done, total: e.total, status: r.status === "probing" ? "downloading" : r.status },
      );
    case "fallback":
      return update(state, key, (r) => (r.status === "ready" ? r : { ...r, status: "fallback" }));
    case "converting":
      return update(state, key, (r) => ({ ...r, status: "converting" }));
    case "done":
      return update(state, key, (r) => ({ ...r, status: "done", path: e.path, done: e.bytes, total: e.bytes }));
    case "skipped": {
      const fields: Partial<Row> = {
        title: e.title,
        status: e.reason.kind,
        path: e.reason.kind === "exists" ? e.reason.path : undefined,
      };
      const row = state.rows[key];
      return row ? putRow(state, { ...row, ...fields }) : addRow(state, e.id, e.parent, fields);
    }
    case "failed": {
      const fields: Partial<Row> = { status: "failed", message: e.message, network: e.network };
      const row = state.rows[key];
      return row
        ? putRow(state, { ...row, ...fields })
        : addRow(state, e.id, e.parent, { ...fields, title: e.label });
    }
    case "cancelled": {
      const group = state.groups[key];
      if (group) {
        let next: QueueState = { ...state, groups: { ...state.groups, [key]: { ...group, cancelled: true } } };
        for (const child of group.children) {
          next = update(next, child, cancelRow);
        }
        return next;
      }
      return update(state, key, cancelRow);
    }
  }
}

/** A cancelled row stops; a ready one only loses its half-built preview. */
function cancelRow(r: Row): Row {
  if (r.status === "ready") return r.preview?.phase === "loading" ? { ...r, preview: undefined } : r;
  return isFinished(r.status) ? r : { ...r, status: "cancelled" };
}

function addRow(
  state: QueueState,
  id: number,
  parent: number | null,
  fields: Partial<Row>,
  input?: string,
): QueueState {
  const key = String(id);
  const row: Row = { key, id, title: "", status: "probing", done: 0, total: 0, removed: [], ...fields };
  if (parent === null) {
    return { ...state, rows: { ...state.rows, [key]: { ...row, input } }, order: [...state.order, key] };
  }
  const groupKey = String(parent);
  const group = state.groups[groupKey];
  if (!group) return state;
  return {
    ...state,
    rows: { ...state.rows, [key]: { ...row, parent: groupKey } },
    groups: { ...state.groups, [groupKey]: { ...group, children: [...group.children, key] } },
  };
}

function putRow(state: QueueState, row: Row): QueueState {
  return { ...state, rows: { ...state.rows, [row.key]: row } };
}

function update(state: QueueState, key: string, fn: (r: Row) => Row): QueueState {
  const row = state.rows[key];
  return row ? putRow(state, fn(row)) : state;
}

function removeTop(state: QueueState, key: string): QueueState {
  const rows = { ...state.rows };
  const groups = { ...state.groups };
  for (const child of groups[key]?.children ?? []) delete rows[child];
  delete rows[key];
  delete groups[key];
  return { rows, groups, order: state.order.filter((k) => k !== key) };
}
