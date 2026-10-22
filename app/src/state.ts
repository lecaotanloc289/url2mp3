// The queue as the UI sees it: a pure reducer over engine events and user actions.
// Rows and playlist groups are keyed by `gen:id` because each engine generation restarts ids.

import type { EngineEvent } from "./api";

export type Status =
  | "probing"
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
  gen: number;
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
}

export interface Group {
  key: string;
  gen: number;
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
  | { kind: "engine"; gen: number; event: EngineEvent }
  | { kind: "remove"; key: string }
  | { kind: "clearFinished" };

export const empty = (): QueueState => ({ rows: {}, groups: {}, order: [] });

const FINISHED: ReadonlySet<Status> = new Set(["done", "exists", "duplicate", "failed", "cancelled"]);
export const isFinished = (s: Status) => FINISHED.has(s);
const isSuccess = (s: Status) => s === "done" || s === "exists" || s === "duplicate";

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
    active: rows.filter((r) => !isFinished(r.status)).length,
  };
}

export function reduce(state: QueueState, msg: Msg): QueueState {
  switch (msg.kind) {
    case "engine":
      return onEvent(state, msg.gen, msg.event);
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

function onEvent(state: QueueState, gen: number, e: EngineEvent): QueueState {
  const key = `${gen}:${e.id}`;
  switch (e.kind) {
    case "probing": {
      const row = state.rows[key];
      if (row) return putRow(state, { ...row, status: "probing" });
      return addRow(state, gen, e.id, e.parent, { title: e.label, status: "probing" }, e.label);
    }
    case "playlist": {
      const row = state.rows[key];
      if (!row) return state;
      const rows = { ...state.rows };
      delete rows[key];
      const group: Group = {
        key,
        gen,
        id: e.id,
        input: row.input ?? row.title,
        title: e.title,
        count: e.count,
        children: [],
      };
      return { ...state, rows, groups: { ...state.groups, [key]: group } };
    }
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
      return row ? putRow(state, { ...row, ...fields }) : addRow(state, gen, e.id, e.parent, fields);
    }
    case "progress":
      return update(state, key, (r) => ({
        ...r,
        done: e.done,
        total: e.total,
        status: r.status === "probing" ? "downloading" : r.status,
      }));
    case "fallback":
      return update(state, key, (r) => ({ ...r, status: "fallback" }));
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
      return row ? putRow(state, { ...row, ...fields }) : addRow(state, gen, e.id, e.parent, fields);
    }
    case "failed": {
      const fields: Partial<Row> = { status: "failed", message: e.message, network: e.network };
      const row = state.rows[key];
      return row
        ? putRow(state, { ...row, ...fields })
        : addRow(state, gen, e.id, e.parent, { ...fields, title: e.label });
    }
    case "cancelled": {
      const group = state.groups[key];
      if (group) {
        let next: QueueState = { ...state, groups: { ...state.groups, [key]: { ...group, cancelled: true } } };
        for (const child of group.children) {
          next = update(next, child, (r) => (isFinished(r.status) ? r : { ...r, status: "cancelled" }));
        }
        return next;
      }
      return update(state, key, (r) => (isFinished(r.status) ? r : { ...r, status: "cancelled" }));
    }
  }
}

function addRow(
  state: QueueState,
  gen: number,
  id: number,
  parent: number | null,
  fields: Partial<Row>,
  input?: string,
): QueueState {
  const key = `${gen}:${id}`;
  const row: Row = { key, gen, id, title: "", status: "probing", done: 0, total: 0, ...fields };
  if (parent === null) {
    return { ...state, rows: { ...state.rows, [key]: { ...row, input } }, order: [...state.order, key] };
  }
  const groupKey = `${gen}:${parent}`;
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
