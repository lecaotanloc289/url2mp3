import "@fontsource-variable/bricolage-grotesque";
import "@fontsource/be-vietnam-pro/400.css";
import "@fontsource/be-vietnam-pro/500.css";
import "@fontsource/be-vietnam-pro/600.css";
import "./style.css";

import { open as pickFolder } from "@tauri-apps/plugin-dialog";
import { api, type Format, type Language, type Settings, type ToolsStatus } from "./api";
import { hint, language, pickLanguage, setLanguage, t } from "./i18n";
import { sharedInput } from "./share";
import { empty, groupStats, isFinished, percent, reduce, saveItem, summary, type Group, type Msg, type Row } from "./state";

const $ = <T extends HTMLElement>(sel: string) => document.querySelector<T>(sel)!;

let state = empty();
let settings: Settings;
let tools: ToolsStatus = { phase: "missing" };
let appUpdate: { version: string; url: string } | null = null;
/** Inputs already retried once after a network failure. */
const autoRetried = new Set<string>();
let android = false;
/** Running tracks the Android notification last showed. */
let keptAlive = 0;

// ── Rendering ────────────────────────────────────────────────────────────────

let frame = 0;
function dispatch(msg: Msg) {
  state = reduce(state, msg);
  if (!frame) frame = requestAnimationFrame(() => ((frame = 0), renderList()));
  // Not inside the frame: Android pauses rendering in the background, which is exactly when
  // the foreground service matters.
  if (android) {
    const { active } = summary(state);
    if (active !== keptAlive) {
      keptAlive = active;
      void api.keepAlive(active > 0, t("keepAlive", { active }));
    }
  }
}

/**
 * Shared from another app: a link downloads straight away; other text (which any app could
 * send) only fills the input, for the user to confirm.
 */
async function takeShared() {
  const text = await api.takeShared().catch(() => null);
  const input = text && sharedInput(text);
  if (!input) return;
  if (/^https?:\/\//.test(input)) {
    void enqueue([input]);
  } else {
    $<HTMLTextAreaElement>("#input").value = input;
  }
}

const ICONS = {
  cancel: '<path d="M6 6l12 12M18 6L6 18"/>',
  retry: '<path d="M4 12a8 8 0 1 0 2.4-5.7M4 4v4h4"/>',
  folder: '<path d="M3 7h6l2 2h10v9a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1z"/>',
  remove: '<path d="M5 7h14M10 11v6M14 11v6M7 7l1 12h8l1-12M9 7V4h6v3"/>',
};

function iconButton(icon: keyof typeof ICONS, title: string, onClick: () => void): HTMLButtonElement {
  const b = document.createElement("button");
  b.type = "button";
  b.className = `icon-btn ${icon}`;
  b.title = title;
  b.setAttribute("aria-label", title);
  b.innerHTML = `<svg viewBox="0 0 24 24" aria-hidden="true">${ICONS[icon]}</svg>`;
  b.addEventListener("click", (e) => (e.stopPropagation(), onClick()));
  return b;
}

const nodes = new Map<string, HTMLElement>();

function renderList() {
  const list = $("#list");
  const keep = new Set<string>();
  state.order.forEach((key, i) => {
    const group = state.groups[key];
    const el = group ? renderGroup(group, i) : renderRow(state.rows[key], i);
    keep.add(key);
    if (list.children[i] !== el) list.insertBefore(el, list.children[i] ?? null);
  });
  for (const [key, el] of nodes) {
    if (!keep.has(key) && !state.rows[key]) {
      el.remove();
      nodes.delete(key);
    }
  }
  $("#empty").hidden = state.order.length > 0;
  const s = summary(state);
  $("#summary").textContent = state.order.length ? t("summary", s) : "";
  $("#clear").hidden = s.done === 0;
}

function shell(key: string, cls: "track" | "group"): HTMLElement {
  let el = nodes.get(key);
  // A link's row becomes a playlist group under the same key: build the group afresh.
  if (el && el.dataset.kind !== cls) {
    const old = el;
    el = undefined;
    nodes.delete(key);
    queueMicrotask(() => old.remove());
  }
  if (!el) {
    el = document.createElement(cls === "group" ? "section" : "article");
    el.className = cls;
    el.dataset.kind = cls;
    el.innerHTML = `
      <span class="no"></span>
      <div class="thumb"><img alt="" referrerpolicy="no-referrer" /></div>
      <div class="meta">
        <div class="title"></div>
        <div class="sub"><span class="artist"></span><span class="status"></span></div>
        <div class="meter"><i></i></div>
        <div class="hint"></div>
      </div>
      <div class="actions"></div>`;
    nodes.set(key, el);
  }
  return el;
}

function setText(el: HTMLElement, sel: string, text: string) {
  const node = el.querySelector<HTMLElement>(sel)!;
  if (node.textContent !== text) node.textContent = text;
}

function setActions(el: HTMLElement, buttons: HTMLButtonElement[]) {
  const box = el.querySelector(".actions")!;
  const sig = buttons.map((b) => b.className).join();
  if (box.getAttribute("data-sig") !== sig) {
    box.replaceChildren(...buttons);
    box.setAttribute("data-sig", sig);
  }
}

function statusText(row: Row): string {
  if (row.status === "downloading" && row.total > 0) return `${percent(row)}%`;
  return t(`status.${row.status}`, { ext: settings.format.toUpperCase() });
}

function renderRow(row: Row, index?: number): HTMLElement {
  const el = shell(row.key, "track");
  el.dataset.status = row.status;
  setText(el, ".no", index === undefined ? "·" : String(index + 1).padStart(2, "0"));
  setText(el, ".title", row.title);
  setText(el, ".artist", row.artist ?? "");
  setText(el, ".status", statusText(row));
  const img = el.querySelector("img")!;
  if (row.thumbnail && img.getAttribute("src") !== row.thumbnail) img.src = row.thumbnail;
  el.classList.toggle("has-thumb", !!row.thumbnail);
  el.querySelector<HTMLElement>(".meter i")!.style.setProperty("--p", `${percent(row)}%`);
  setText(el, ".hint", row.status === "failed" ? `${row.message ?? ""} — ${t(hint(row.message ?? "", !!row.network))}` : "");

  const top = !row.parent;
  const buttons: HTMLButtonElement[] = [];
  if (!isFinished(row.status) && top) buttons.push(iconButton("cancel", t("cancel"), () => api.cancel(row.id)));
  if (row.path && !android) buttons.push(iconButton("folder", t("reveal"), () => api.reveal(row.path!)));
  if ((row.status === "failed" || row.status === "cancelled") && top) buttons.push(iconButton("retry", t("retry"), () => retry(row.key)));
  if (isFinished(row.status) && top) buttons.push(iconButton("remove", t("remove"), () => removeTop(row.key)));
  setActions(el, buttons);
  return el;
}

function renderGroup(group: Group, index: number): HTMLElement {
  const el = shell(group.key, "group");
  const stats = groupStats(state, group);
  const running = group.children.some((k) => !isFinished(state.rows[k]?.status ?? "done"));
  const finished = !running && (group.cancelled || stats.finished + stats.failed >= group.count);
  el.dataset.status = group.cancelled ? "cancelled" : finished ? (stats.failed ? "failed" : "done") : "downloading";
  setText(el, ".no", String(index + 1).padStart(2, "0"));
  setText(el, ".title", group.title);
  setText(el, ".artist", "Playlist");
  setText(el, ".status", [t("playlistCount", stats), stats.failed ? t("playlistFailed", stats) : ""].filter(Boolean).join(" · "));
  el.querySelector<HTMLElement>(".meter i")!.style.setProperty("--p", `${group.count ? Math.round(((stats.finished + stats.failed) / group.count) * 100) : 0}%`);
  setText(el, ".hint", "");

  const buttons: HTMLButtonElement[] = [];
  if (!finished) buttons.push(iconButton("cancel", t("cancel"), () => api.cancel(group.id)));
  if (finished && (stats.failed || group.cancelled)) buttons.push(iconButton("retry", t("retry"), () => retry(group.key)));
  if (finished) buttons.push(iconButton("remove", t("remove"), () => removeTop(group.key)));
  setActions(el, buttons);

  let children = el.querySelector<HTMLElement>(".children");
  if (!children) {
    children = document.createElement("div");
    children.className = "children";
    el.append(children);
    el.querySelector(".meta")!.addEventListener("click", () => el.classList.toggle("open"));
  }
  group.children.forEach((key, i) => {
    const child = renderRow(state.rows[key]);
    if (children!.children[i] !== child) children!.insertBefore(child, children!.children[i] ?? null);
  });
  return el;
}

// ── Actions ──────────────────────────────────────────────────────────────────

async function enqueue(inputs: string[]): Promise<boolean> {
  if (!inputs.length) return false;
  try {
    await api.add(inputs);
    return true;
  } catch (e) {
    toast(String(e) === "ytdlp-missing" ? t("ytdlpMissing") : String(e));
    return false;
  }
}

/** Removes a row or group and drops its tray entry and cache in the backend. */
function removeTop(key: string) {
  const id = state.groups[key]?.id ?? state.rows[key]?.id;
  dispatch({ kind: "remove", key });
  if (id !== undefined) void api.discard([id]).catch(() => {});
}

/** Saves rows again from the tray's cache, keeping their cuts. */
function resave(rows: Row[]) {
  dispatch({ kind: "saving", keys: rows.map((r) => r.key) });
  api.save(rows.map((r) => saveItem(r, t("trimmedSuffix")))).catch((e) => toast(String(e)));
}

function retry(key: string) {
  const group = state.groups[key];
  const rows = group ? group.children.map((k) => state.rows[k]) : [state.rows[key]];
  const again = rows.filter((r) => r?.saved && (r.status === "failed" || r.status === "cancelled"));
  if (again.length) {
    resave(again);
    return;
  }
  const input = state.groups[key]?.input ?? state.rows[key]?.input;
  if (!input) return;
  removeTop(key);
  void enqueue([input]);
}

async function submit() {
  const box = $<HTMLTextAreaElement>("#input");
  const inputs = box.value.split("\n").map((s) => s.trim()).filter(Boolean);
  if (await enqueue([...new Set(inputs)])) box.value = "";
  box.focus();
}

let toastTimer = 0;
function toast(text: string) {
  const el = $("#toast");
  el.textContent = text;
  el.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => el.classList.remove("show"), 4000);
}

// ── Banners: yt-dlp and app updates ─────────────────────────────────────────

function renderBanners() {
  const box = $("#banners");
  box.replaceChildren();
  const banner = (cls: string, text: string, action?: [string, () => void]) => {
    const div = document.createElement("div");
    div.className = `banner ${cls}`;
    const span = document.createElement("span");
    span.textContent = text;
    div.append(span);
    if (action) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "text-btn";
      b.textContent = action[0];
      b.addEventListener("click", action[1]);
      div.append(b);
    }
    box.append(div);
  };
  if (tools.phase === "missing") banner("info", t("tools.missing"), [t("tools.install"), () => updateTools(true)]);
  if (tools.phase === "downloading") banner("busy", t("tools.downloading", { version: tools.version }));
  if (tools.phase === "error") banner("error", t("tools.error", { message: tools.message }), [t("tools.retry"), () => updateTools(true)]);
  if (appUpdate) {
    const { url } = appUpdate;
    banner("info", t("appUpdate", { version: appUpdate.version }), [t("appUpdateOpen"), () => api.openUrl(url)]);
  }
  const version = tools.phase === "ready" || tools.phase === "downloading" ? tools.version : tools.phase === "error" ? tools.version : null;
  $("#ytdlp-version").textContent = version ?? t("set.ytdlpNone");
}

async function updateTools(force: boolean) {
  try {
    tools = await api.updateYtdlp(force);
  } catch (e) {
    // Other errors already came as a `tools` event.
    if (String(e) === "ytdlp-busy") toast(t("ytdlpBusy"));
  }
  renderBanners();
}

// ── Settings ─────────────────────────────────────────────────────────────────

function applyLanguage() {
  setLanguage(pickLanguage(settings.language, navigator.languages));
  document.documentElement.lang = language();
  for (const el of document.querySelectorAll<HTMLElement>("[data-i18n]")) el.textContent = t(el.dataset.i18n as never);
  for (const el of document.querySelectorAll<HTMLElement>("[data-i18n-placeholder]"))
    el.setAttribute("placeholder", t(el.dataset.i18nPlaceholder as never));
  for (const el of document.querySelectorAll<HTMLElement>("[data-i18n-title]")) el.title = t(el.dataset.i18nTitle as never);
  renderBanners();
  for (const el of nodes.values()) el.querySelector(".actions")?.removeAttribute("data-sig");
  renderList();
}

function showFormat() {
  for (const b of document.querySelectorAll<HTMLButtonElement>("#formats button")) {
    b.setAttribute("aria-checked", String(b.dataset.format === settings.format));
  }
}

async function saveSettings(next: Settings) {
  try {
    await api.setSettings(next);
    const langChanged = next.language !== settings.language;
    settings = next;
    showFormat();
    if (langChanged) applyLanguage();
  } catch (e) {
    toast(String(e));
  }
}

let draftOutDir: string | null = null;

async function openSettings() {
  const form = $<HTMLFormElement>("#settings-form");
  const field = (name: string) => form.elements.namedItem(name) as HTMLInputElement;
  draftOutDir = settings.outDir;
  $("#out-dir").textContent = settings.outDir ?? (await api.defaultOutDir()) ?? "";
  field("format").value = settings.format;
  field("bitrate").value = settings.bitrate?.toString() ?? "";
  field("jobs").value = String(settings.jobs);
  field("connections").value = String(settings.connections);
  field("playlistLimit").value = settings.playlistLimit?.toString() ?? "";
  field("language").value = settings.language ?? "";
  field("cover").checked = settings.cover;
  field("cookiesBrowser").value = settings.cookiesBrowser ?? "";
  $<HTMLDialogElement>("#settings").showModal();
}

function readSettingsForm(): Settings {
  const form = $<HTMLFormElement>("#settings-form");
  const value = (name: string) => (form.elements.namedItem(name) as HTMLInputElement).value;
  const num = (name: string) => (value(name) ? Number(value(name)) : null);
  return {
    format: value("format") as Format,
    bitrate: num("bitrate"),
    outDir: draftOutDir,
    jobs: num("jobs") ?? settings.jobs,
    connections: num("connections") ?? settings.connections,
    playlistLimit: num("playlistLimit") || null,
    cover: (form.elements.namedItem("cover") as HTMLInputElement).checked,
    cookiesBrowser: value("cookiesBrowser") || null,
    language: (value("language") || null) as Language | null,
  };
}

// ── Wiring ───────────────────────────────────────────────────────────────────

async function main() {
  android = (await api.platform()) === "android";
  document.body.classList.toggle("android", android);
  settings = await api.getSettings();
  applyLanguage();
  showFormat();

  await api.onTray((event) => dispatch({ kind: "tray", event }));
  await api.onEngine(({ event }) => {
    dispatch({ kind: "engine", event });
    // One automatic retry for a link that failed on a network hiccup.
    if (event.kind === "failed" && event.network && event.parent === null) {
      const key = String(event.id);
      const input = state.rows[key]?.input;
      if (input && !autoRetried.has(input)) {
        autoRetried.add(input);
        setTimeout(() => retry(key), 1500);
      }
    }
  });
  await api.onTools((status) => {
    tools = status;
    renderBanners();
  });

  $("#go").addEventListener("click", submit);
  $("#input").addEventListener("keydown", (e) => {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      void submit();
    }
  });
  for (const b of document.querySelectorAll<HTMLButtonElement>("#formats button")) {
    b.addEventListener("click", () => saveSettings({ ...settings, format: b.dataset.format as Format }));
  }
  $("#clear").addEventListener("click", () => dispatch({ kind: "clearFinished" }));
  $("#open-settings").addEventListener("click", openSettings);
  $("#close-settings").addEventListener("click", () => $<HTMLDialogElement>("#settings").close());
  $("#choose-dir").addEventListener("click", async () => {
    const dir = await pickFolder({ directory: true, defaultPath: draftOutDir ?? undefined });
    if (typeof dir === "string") {
      draftOutDir = dir;
      $("#out-dir").textContent = dir;
    }
  });
  $("#settings-form").addEventListener("submit", () => saveSettings(readSettingsForm()));
  $("#update-ytdlp").addEventListener("click", () => updateTools(true));
  $("#copy-log").addEventListener("click", async () => {
    await navigator.clipboard.writeText(await api.readLog());
    toast(t("set.copied"));
  });

  renderList();
  tools = await api.toolsStatus();
  renderBanners();
  void updateTools(false);
  api.checkAppUpdate().then((v) => ((appUpdate = v), renderBanners()), () => {});
  if (android) {
    // Share → url2mp3 opens the app (or brings it back): pick the link up each time.
    void takeShared();
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState === "visible") void takeShared();
    });
  } else {
    $("#input").focus();
  }
}

void main();
