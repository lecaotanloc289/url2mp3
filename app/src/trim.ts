// The trim dialog: listen to one tray row, remove any parts, save it whole or trimmed.
// The dialog owns the removed ranges while it is open and hands them back (`cut`) on close.
// Closing never cancels anything: a running `prepare` finishes in the background and its cache
// makes the next opening instant.

import { api, type Language, type Prepared, type SaveItem, type Segment } from "./api";
import { formatTime, History, kept, keptLength, normalize, nudge, type Range } from "./cut";
import { category, t } from "./i18n";
import { saveItem, type Msg, type Row } from "./state";
import { bars, dragTo, findRange, hitEdge, interrupted, playFrom, span, ticks, timeAt, xAt, type EdgeHit } from "./wave";

export interface TrimDeps {
  language: Language;
  dispatch: (m: Msg) => void;
  onSave: (item: SaveItem) => void;
}

/** One opening of the dialog; async work from an older opening checks `live(s)`. */
interface Session {
  row: Row;
  deps: TrimDeps;
  /**
   * Seconds: yt-dlp's (rounded) until `prepare` answers, then `prepared.duration` (measured from
   * the decoded audio) or else `audio.duration`.
   */
  duration?: number;
  prepared?: Prepared;
  /** Normalized whenever the duration is known, except in the middle of an edge drag. */
  removed: Range[];
  history: History;
  /** How many undo steps this opening made. */
  steps: number;
  suggestions: Segment[];
  sel: Range | null;
  /** The edge the ±1 s buttons move. */
  active: EdgeHit | null;
  phase: "loading" | "ready" | "failed";
  message?: string;
  audio: HTMLAudioElement;
  url?: string;
  canPlay: boolean;
  raf: number;
  closed: boolean;
}

/** Pointer travel, in px, before a press counts as a drag rather than a tap. */
const TAP = 4;
/** A drawn selection shorter than this is dropped. */
const MIN_SEL = 0.3;
const BAR = 3;

let session: Session | null = null;
let drag: { hit: EdgeHit | null; anchor: number; startX: number; moved: boolean } | null = null;
let wired = false;

const $ = <T extends HTMLElement>(sel: string) => document.querySelector<T>(sel)!;
const live = (s: Session) => session === s && !s.closed;

/** Key of the row the dialog shows, or null when closed. */
export function trimKey(): string | null {
  return session && !session.closed ? session.row.key : null;
}

/** The list re-rendered: refresh title, duration and preview progress; close if the row is gone. */
export function updateTrim(row: Row | undefined) {
  const s = session;
  if (!s || s.closed) return;
  if (!row) {
    $<HTMLDialogElement>("#trim").close();
    finish();
    return;
  }
  s.row = row;
  if (s.duration === undefined && row.duration !== undefined) setDuration(s, row.duration);
  render();
}

export function openTrim(row: Row, deps: TrimDeps) {
  wire();
  finish();
  const dialog = $<HTMLDialogElement>("#trim");
  const history = new History();
  if (row.removed.length) history.push(row.removed);
  const s: Session = {
    row,
    deps,
    duration: row.duration,
    removed: row.duration === undefined ? [...row.removed] : normalize(row.removed, row.duration),
    history,
    steps: 0,
    suggestions: [],
    sel: null,
    active: null,
    phase: "loading",
    audio: new Audio(),
    canPlay: false,
    raf: 0,
    closed: false,
  };
  session = s;
  dialog.lang = deps.language;
  watchAudio(s);
  if (!dialog.open) dialog.showModal();
  render();
  draw();
  void load(s);
}

/** Stop playback, free the audio and give the cuts back to the list. Safe to call twice. */
function finish() {
  const s = session;
  if (!s || s.closed) return;
  s.closed = true;
  session = null;
  drag = null;
  cancelAnimationFrame(s.raf);
  s.audio.pause();
  s.audio.removeAttribute("src");
  s.audio.load();
  if (s.url) URL.revokeObjectURL(s.url);
  s.deps.dispatch({ kind: "cut", key: s.row.key, removed: s.removed, duration: s.duration });
}

// ── Loading ──────────────────────────────────────────────────────────────────

async function load(s: Session) {
  const { key, id } = s.row;
  const { dispatch } = s.deps;
  // Retry: drop the audio of the failed attempt; play stays off until the new one loads.
  if (s.url) {
    s.audio.pause();
    s.audio.removeAttribute("src");
    s.audio.load();
    URL.revokeObjectURL(s.url);
    s.url = undefined;
  }
  s.canPlay = false;
  s.phase = "loading";
  s.message = undefined;
  render();
  dispatch({ kind: "preview", key, phase: "loading" });
  let prepared: Prepared;
  try {
    prepared = await api.prepare(id);
  } catch (e) {
    fail(s, String(e));
    return;
  }
  // Also when the dialog was closed meanwhile: the tray row shows the suggestions.
  dispatch({ kind: "suggestions", key, segments: prepared.suggestions });
  dispatch({ kind: "preview", key, phase: "ready" });
  if (!live(s)) return;
  s.prepared = prepared;
  if (prepared.duration !== null) setDuration(s, prepared.duration);
  s.suggestions = s.row.suggested ?? prepared.suggestions;
  render();
  draw();
  let buf: ArrayBuffer;
  try {
    buf = await api.previewAudio(id);
  } catch (e) {
    fail(s, String(e));
    return;
  }
  if (!live(s)) return;
  s.url = URL.createObjectURL(new Blob([buf], { type: "audio/mp4" }));
  s.audio.src = s.url;
  s.phase = "ready";
  render();
}

function fail(s: Session, message: string) {
  s.deps.dispatch({ kind: "preview", key: s.row.key, phase: "failed", message });
  if (!live(s)) return;
  s.phase = "failed";
  s.message = message;
  render();
}

function watchAudio(s: Session) {
  const a = s.audio;
  a.preload = "auto";
  a.addEventListener("loadedmetadata", () => {
    if (!live(s)) return;
    if (s.prepared?.duration == null && Number.isFinite(a.duration) && a.duration > 0) setDuration(s, a.duration);
    s.canPlay = true;
    render();
  });
  a.addEventListener("play", () => live(s) && (loop(s), render()));
  a.addEventListener("timeupdate", () => live(s) && !a.paused && skipRemoved(s));
  a.addEventListener("pause", () => live(s) && render());
  a.addEventListener("ended", () => live(s) && render());
  a.addEventListener("error", () => {
    if (live(s) && a.getAttribute("src")) fail(s, a.error?.message || "audio");
  });
}

function setDuration(s: Session, d: number) {
  s.duration = d;
  s.removed = normalize(s.removed, d);
  if (s.sel) s.sel = normalize([s.sel], d)[0] ?? null;
}

// ── Editing ──────────────────────────────────────────────────────────────────

function commit(s: Session, removed: Range[]) {
  s.removed = s.duration === undefined ? removed : normalize(removed, s.duration);
  s.history.push(s.removed);
  s.steps++;
}

/** After a cut edge moved: keep pointing at it even if normalize re-sorted the ranges. */
function commitEdge(s: Session, hit: EdgeHit, cuts: Range[]) {
  const value = cuts[hit.index][hit.edge];
  commit(s, cuts);
  const index = findRange(s.removed, hit.edge, value);
  s.active = index >= 0 ? { ...hit, index } : null;
}

function removeSelection(s: Session) {
  if (!s.sel) return;
  commit(s, [...s.removed, s.sel]);
  s.sel = null;
  s.active = null;
}

function undo(s: Session) {
  if (!s.steps) return;
  const prev = s.history.undo();
  if (!prev) return;
  s.steps--;
  s.removed = s.duration === undefined ? [...prev] : normalize(prev, s.duration);
  s.active = null;
}

function nudgeActive(s: Session, delta: number) {
  const a = s.active;
  const d = s.duration;
  if (!a || d === undefined) return;
  if (a.target === "sel") {
    if (s.sel) s.sel = nudge(s.sel, a.edge, delta, d);
    return;
  }
  const cuts = [...s.removed];
  if (!cuts[a.index]) return;
  cuts[a.index] = nudge(cuts[a.index], a.edge, delta, d);
  commitEdge(s, a, cuts);
}

function acceptSuggestions(s: Session) {
  commit(s, [...s.removed, ...s.suggestions.map(({ start, end }) => ({ start, end }))]);
  s.suggestions = [];
  s.deps.dispatch({ kind: "suggestionsHandled", key: s.row.key });
}

function dismissSuggestions(s: Session) {
  s.suggestions = [];
  s.deps.dispatch({ kind: "suggestionsHandled", key: s.row.key });
}

// ── Playback: skips removed parts, so the preview is what will be saved ─────

function toggle(s: Session) {
  const a = s.audio;
  if (!a.paused) {
    a.pause();
    return;
  }
  if (s.duration === undefined) return;
  const removed = normalize(s.removed, s.duration);
  let to = playFrom(a.currentTime, removed, s.duration);
  if (to === "end" || a.ended) {
    a.currentTime = 0;
    to = playFrom(0, removed, s.duration);
  }
  if (typeof to === "number") a.currentTime = to;
  if (to === "end") return;
  a.play().catch((e) => {
    if (!interrupted(e)) fail(s, String(e));
  });
}

/** Jump over a removed part under the playhead, or stop when only removed audio is left. */
function skipRemoved(s: Session) {
  if (s.duration === undefined) return;
  const to = playFrom(s.audio.currentTime, normalize(s.removed, s.duration), s.duration);
  if (to === "end") s.audio.pause();
  else if (to !== null) s.audio.currentTime = to;
}

/**
 * Checks every frame while the window is drawn. A hidden or minimised window (or Android app
 * in the background) gets no frames but keeps playing: `timeupdate` (see `watchAudio`) skips
 * there, a few times a second.
 */
function loop(s: Session) {
  cancelAnimationFrame(s.raf);
  const tick = () => {
    if (!live(s) || s.audio.paused) return;
    skipRemoved(s);
    placePlayhead(s);
    s.raf = requestAnimationFrame(tick);
  };
  s.raf = requestAnimationFrame(tick);
}

function seek(s: Session, time: number) {
  if (!s.canPlay) return;
  s.audio.currentTime = time;
  placePlayhead(s);
}

// ── Saving ───────────────────────────────────────────────────────────────────

function save(s: Session, whole: boolean) {
  const label = t("trimmedSuffix");
  const item: SaveItem = whole
    ? { id: s.row.id, cut: null }
    : saveItem({ ...s.row, removed: s.removed, duration: s.duration, whole: false }, label);
  const { onSave } = s.deps;
  $<HTMLDialogElement>("#trim").close();
  finish();
  onSave(item);
}

// ── Pointer: drag to select, drag an edge to adjust, tap to seek ────────────

function pointerX(e: PointerEvent): { x: number; width: number } {
  const rect = $("#trim-wave").getBoundingClientRect();
  return { x: e.clientX - rect.left, width: rect.width };
}

function onDown(e: PointerEvent) {
  const s = session;
  if (!s || s.duration === undefined || e.button > 0) return;
  const { x, width } = pointerX(e);
  const hit = hitEdge(x, width, s.duration, s.sel, s.removed);
  $("#trim-wave").setPointerCapture(e.pointerId);
  drag = { hit, anchor: timeAt(x, width, s.duration), startX: x, moved: false };
  if (hit) s.active = hit;
  render();
}

function onMove(e: PointerEvent) {
  const s = session;
  const wave = $("#trim-wave");
  if (!s || s.duration === undefined) return;
  const { x, width } = pointerX(e);
  if (!drag) {
    wave.classList.toggle("on-edge", !!hitEdge(x, width, s.duration, s.sel, s.removed));
    return;
  }
  if (!drag.moved && Math.abs(x - drag.startX) < TAP) return;
  drag.moved = true;
  const time = timeAt(x, width, s.duration);
  const hit = drag.hit;
  if (hit?.target === "sel" && s.sel) {
    s.sel = dragTo(s.sel, hit.edge, time, s.duration);
  } else if (hit?.target === "cut" && s.removed[hit.index]) {
    const cuts = [...s.removed];
    cuts[hit.index] = dragTo(cuts[hit.index], hit.edge, time, s.duration);
    s.removed = cuts;
  } else {
    s.sel = span(drag.anchor, time);
    s.active = { target: "sel", index: -1, edge: time >= drag.anchor ? "end" : "start" };
  }
  render();
}

function onUp(e: PointerEvent, cancelled: boolean) {
  const s = session;
  const d = drag;
  drag = null;
  if (!s || !d || s.duration === undefined) return;
  if ($("#trim-wave").hasPointerCapture(e.pointerId)) $("#trim-wave").releasePointerCapture(e.pointerId);
  if (!d.moved) {
    if (!d.hit && !cancelled) {
      const inside = s.sel && d.anchor >= s.sel.start && d.anchor <= s.sel.end;
      if (!inside) {
        s.sel = null;
        s.active = null;
      }
      seek(s, d.anchor);
    }
  } else if (d.hit?.target === "cut") {
    commitEdge(s, d.hit, s.removed);
  } else if (s.sel && s.sel.end - s.sel.start < MIN_SEL) {
    s.sel = null;
    s.active = null;
  }
  render();
}

// ── Drawing ──────────────────────────────────────────────────────────────────

function pct(time: number, d: number): string {
  return `${(xAt(time, 100, d)).toFixed(3)}%`;
}

/** The waveform bars; the ranges on top of them are DOM layers (see `render`). */
function draw() {
  const s = session;
  const canvas = $<HTMLCanvasElement>("#trim-wave canvas");
  const { width, height } = canvas.getBoundingClientRect();
  if (!width || !height) return;
  const dpr = window.devicePixelRatio || 1;
  canvas.width = Math.round(width * dpr);
  canvas.height = Math.round(height * dpr);
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, width, height);
  const css = getComputedStyle($("#trim-wave"));
  const peaks = s?.prepared?.peaks ?? [];
  if (!peaks.length) {
    ctx.fillStyle = css.getPropertyValue("--line");
    ctx.fillRect(0, height / 2 - 0.5, width, 1);
    return;
  }
  ctx.fillStyle = css.getPropertyValue("--amber");
  const n = Math.max(1, Math.floor(width / BAR));
  const values = bars(peaks, n);
  const room = height - 10;
  values.forEach((v, i) => {
    const h = Math.max(1.5, v * room);
    ctx.fillRect(i * BAR, (height - h) / 2, BAR - 1, h);
  });
}

function edgeClass(s: Session, target: "sel" | "cut", index: number): string {
  const a = s.active;
  return a && a.target === target && a.index === index ? ` active-${a.edge}` : "";
}

function renderLayers(s: Session) {
  const box = $("#trim-layers");
  const d = s.duration;
  if (d === undefined) {
    box.replaceChildren();
    return;
  }
  const html: string[] = [];
  for (const r of s.suggestions) {
    html.push(`<div class="suggest" style="left:${pct(r.start, d)};width:${pct(r.end - r.start, d)}"></div>`);
  }
  s.removed.forEach((r, i) => {
    const label = r.end - r.start >= d * 0.07 ? `<span>−${formatTime(Math.round(r.end - r.start))}</span>` : "";
    html.push(`<div class="cut${edgeClass(s, "cut", i)}" style="left:${pct(r.start, d)};width:${pct(r.end - r.start, d)}">${label}<i class="h start"></i><i class="h end"></i></div>`);
  });
  if (s.sel) {
    html.push(`<div class="sel${edgeClass(s, "sel", -1)}" style="left:${pct(s.sel.start, d)};width:${pct(s.sel.end - s.sel.start, d)}"><i class="h start"></i><i class="h end"></i></div>`);
  }
  box.innerHTML = html.join("");
}

function renderRuler(s: Session) {
  const ruler = $("#trim-ruler");
  const d = s.duration;
  // At most one label per 80 px, so a phone-width ruler does not crowd.
  const marks = d === undefined ? [] : ticks(d, Math.max(3, Math.min(6, Math.floor(ruler.clientWidth / 80))));
  const sig = marks.join();
  if (ruler.dataset.sig === sig && ruler.childElementCount) return;
  ruler.dataset.sig = sig;
  if (d === undefined) {
    ruler.innerHTML = `<span style="left:0">0:00</span><span style="left:100%">${formatTime(NaN)}</span>`;
    return;
  }
  ruler.innerHTML = marks.map((m) => `<span style="left:${pct(m, d)}">${formatTime(m)}</span>`).join("");
}

function placePlayhead(s: Session) {
  const head = $("#trim-playhead");
  head.hidden = !s.canPlay || s.duration === undefined;
  if (!head.hidden) head.style.left = pct(Math.min(s.audio.currentTime, s.duration!), s.duration!);
}

function render() {
  const s = session;
  if (!s) return;
  const { row } = s;
  const d = s.duration;
  $("#trim-title").textContent = row.title;
  $("#trim-artist").textContent = row.artist ?? "";
  $("#trim-length").textContent = formatTime(d ?? NaN);
  const img = $<HTMLImageElement>("#trim .thumb img");
  if (row.thumbnail && img.getAttribute("src") !== row.thumbnail) img.src = row.thumbnail;
  $("#trim .trim-head").classList.toggle("has-thumb", !!row.thumbnail);

  $("#trim-exists").hidden = !s.prepared?.exists;

  const chip = $("#trim-chip");
  chip.hidden = !s.suggestions.length;
  if (s.suggestions.length) {
    const list = s.suggestions
      .map((g) => `${formatTime(g.start)}–${formatTime(g.end)} (${category(g.category)})`)
      .join(" · ");
    $("#trim-chip-text").textContent = `✂ ${t("trim.suggest", { n: s.suggestions.length, list })}`;
  }

  const state = $("#trim-state");
  const preview = row.preview;
  state.hidden = s.phase === "ready";
  state.classList.toggle("failed", s.phase === "failed");
  $("#trim-retry").hidden = s.phase !== "failed";
  if (s.phase === "loading") {
    const p = preview?.phase === "loading" && preview.total > 0 ? ` ${Math.min(100, Math.round((preview.done / preview.total) * 100))}%` : "";
    $("#trim-state-text").textContent = t("trim.loading") + p;
  } else if (s.phase === "failed") {
    $("#trim-state-text").textContent = t("trim.failed", { message: s.message ?? "" });
  }

  const wave = $("#trim-wave");
  wave.classList.toggle("loading", !s.prepared && s.phase === "loading");
  wave.classList.toggle("disabled", d === undefined);
  renderLayers(s);
  renderRuler(s);
  placePlayhead(s);

  const playing = !s.audio.paused;
  const play = $<HTMLButtonElement>("#trim-play");
  play.disabled = !s.canPlay || d === undefined;
  play.classList.toggle("playing", playing);
  $("#trim-play-text").textContent = t(playing ? "trim.pause" : "trim.play");
  $<HTMLButtonElement>("#trim-back").disabled = !s.active;
  $<HTMLButtonElement>("#trim-forward").disabled = !s.active;
  $<HTMLButtonElement>("#trim-remove").disabled = !s.sel;
  $("#trim-sel").textContent = s.sel ? ` (${formatTime(s.sel.start)}–${formatTime(s.sel.end)})` : "";
  $<HTMLButtonElement>("#trim-undo").disabled = !s.steps;

  const removed = d === undefined ? [] : normalize(s.removed, d);
  const keptSecs = d === undefined ? NaN : Math.round(keptLength(removed, d));
  $("#trim .trim-sum").hidden = !removed.length;
  if (d !== undefined && removed.length) {
    $("#trim-kept").textContent = t("trim.kept", { kept: formatTime(keptSecs), cut: formatTime(Math.round(d) - keptSecs) });
  }
  $("#trim-save-whole").textContent = t("trim.saveWhole", { time: formatTime(d ?? NaN) });
  const cut = $<HTMLButtonElement>("#trim-save-cut");
  cut.textContent = t("trim.saveCut", { time: formatTime(removed.length ? keptSecs : NaN) });
  cut.disabled = d === undefined || !removed.length || kept(removed, d).length === 0;
}

// ── Wiring (once) ────────────────────────────────────────────────────────────

function wire() {
  if (wired) return;
  wired = true;
  const dialog = $<HTMLDialogElement>("#trim");
  const on = (sel: string, fn: (s: Session) => void) =>
    $(sel).addEventListener("click", () => {
      if (session) {
        fn(session);
        render();
      }
    });
  // Escape, the close button and the backdrop all end in `close`; reopening keeps it open.
  dialog.addEventListener("close", () => !dialog.open && finish());
  dialog.addEventListener("click", (e) => e.target === dialog && dialog.close());
  $("#trim-close").addEventListener("click", () => dialog.close());
  on("#trim-play", toggle);
  on("#trim-back", (s) => nudgeActive(s, -1));
  on("#trim-forward", (s) => nudgeActive(s, 1));
  on("#trim-remove", removeSelection);
  on("#trim-undo", undo);
  on("#trim-accept", acceptSuggestions);
  on("#trim-dismiss", dismissSuggestions);
  on("#trim-retry", (s) => void load(s));
  $("#trim-save-whole").addEventListener("click", () => session && save(session, true));
  $("#trim-save-cut").addEventListener("click", () => session && save(session, false));

  const wave = $("#trim-wave");
  wave.addEventListener("pointerdown", onDown);
  wave.addEventListener("pointermove", onMove);
  wave.addEventListener("pointerup", (e) => onUp(e, false));
  wave.addEventListener("pointercancel", (e) => onUp(e, true));
  new ResizeObserver(() => {
    if (!session) return;
    draw();
    render();
  }).observe(wave);
  matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => session && draw());
}
