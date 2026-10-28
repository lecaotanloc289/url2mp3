# Listen, trim and join before saving — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every input goes into a "ready to download" tray first; one track opens a trim screen
(waveform, preview that skips removed parts, SponsorBlock suggestions) and saves either the whole
track or the kept parts joined into one file.

**Architecture:** `crates/core` gains stages the app can drive one by one (resolve → fetch audio →
preview/peaks → save with optional cut) and `Engine::process` is rebuilt on top of them, so the
CLI is unchanged. `app/src-tauri/src/jobs.rs` keeps the tray (resolved tracks, cached audio in
the app cache dir) behind new commands. The UI turns tray items into rows of the existing list
(status `ready`) and adds a trim dialog.

**Tech Stack:** Rust 2024 (tokio, reqwest, serde, sha2), ffmpeg filters `atrim`/`asetpts`/`afade`/
`concat`, Tauri 2 (`tauri::ipc::Response` for preview bytes), TypeScript + Vite, vitest.

**Spec:** `docs/superpowers/specs/2026-10-28-trim-before-download-design.md`

## Global Constraints

- CLI (`crates/cli`) behaviour and output unchanged; every existing `cargo test --workspace` case passes.
- Nothing is written to the output folder before the user presses a Download button.
- No double download: a track whose audio was fetched for preview is saved from that cached audio, also after a settings change or yt-dlp update.
- Settings (format, bitrate, cover, output folder) are read when Download is pressed.
- Whole-track saves keep today's rules (skip existing unless `force`, same video once per session, `fit` names, playlist subfolder).
- Trimmed save: never overwrites, never skipped; name `<title> (đã cắt).<ext>` (vi UI) / `<title> (trimmed).<ext>` (en UI); taken → `(đã cắt 2)`, `(đã cắt 3)`…; bypasses the video-id claim, still claims its path.
- A trimmed save is always re-encoded; joins get a 15 ms fade-out/fade-in (`afade d=0.015`) on the inner edges only.
- Suggestions are never applied without a click. SponsorBlock only for `extractor_key == "Youtube"`; request `https://sponsor.ajay.app/api/skipSegments/<first 4 hex of sha256(videoId)>` with categories `music_offtopic, sponsor, intro, outro, selfpromo, interaction, preview`, `actionType` `skip` only; any error or >5 s → no suggestions, no message.
- Preview plays in WKWebView, WebView2, Android WebView: always an AAC `.m4a` preview copy (stream copy when the source is AAC).
- Every new UI string in both `vi` and `en` dictionaries of `app/src/i18n.ts` (a test enforces equal keys).
- Look follows `app/src/style.css`: removed = red diagonal stripes (`--rec`), selection = white frame with handles, playhead = `--ok`, suggestion = dashed `--amber`; light and dark schemes; works at the 420 px minimum window width and on a phone in portrait.
- Commit messages: conventional commits in English, ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

- A one-hour mix: preview and peaks must still load (peaks read ffmpeg PCM at 4000 Hz mono, reduced in a streaming pass, not by holding all samples). Test in Task 2.
- yt-dlp reports no `duration` (some TikToks/live replays): the trim screen takes the duration from the preview `<audio>` once loaded; the tray row shows "–:––". Test in Task 6 (`kept` with duration from the audio) and Task 7 (row without duration).
- The user removes a tray row while its audio is still being fetched: the fetch task is aborted and the cache files are deleted, no orphan yt-dlp/ffmpeg. Test in Task 4 (cancel drops scratch) and manual check in Task 9.
- Format changed MP3 → M4A after preview: the save re-encodes from the cache with no network access. Test in Task 4 (`save` with `audio: Some` never calls yt-dlp).
- Preview fetch fails (network): the trim screen shows the error with Retry, and "Download whole track" still works (falls back to the normal pipeline). Test in Task 7 (reducer `previewFailed` keeps the row `ready`).

---

### Task 1: Cut ranges and trimmed encode in core

**Files:**
- Create: `crates/core/src/cut.rs`
- Modify: `crates/core/src/lib.rs` (add `mod cut; pub use cut::{Cut, Range};`)
- Modify: `crates/core/src/transcode.rs` (`Job` gains `keep`)
- Modify: `crates/core/src/resolve.rs` (`Info` gains `duration: Option<f64>`, `extractor_key: Option<String>`)
- Modify: `crates/core/src/engine.rs` (pass `keep: None` where `transcode::Job` is built)
- Test: `crates/core/src/cut.rs` (unit), `crates/core/tests/cut_ffmpeg.rs` (real ffmpeg, skipped when absent)

**Interfaces:**
- Produces:
  - `#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)] pub struct Range { pub start: f64, pub end: f64 }` (seconds; serde camelCase not needed, field names are single words)
  - `#[derive(Clone, Debug, Deserialize)] pub struct Cut { pub keep: Vec<Range>, pub label: String }` — `label` is the name suffix text, e.g. `"đã cắt"`.
  - `pub fn normalize(keep: &[Range]) -> Vec<Range>` — drop NaN/inf, clamp `start` to ≥ 0, drop ranges shorter than 0.05 s, sort, merge overlapping or touching (gap < 0.01 s).
  - `pub(crate) fn filter_graph(keep: &[Range]) -> String` — ffmpeg `-filter_complex` value ending in label `[out]`.
  - `transcode::Job { …, keep: Option<&'a [Range]> }`

- [ ] **Step 1: Write failing unit tests in `cut.rs`**

```rust
#[test]
fn normalize_sorts_merges_and_drops_slivers() {
    let r = |start, end| Range { start, end };
    assert_eq!(
        normalize(&[r(10.0, 20.0), r(-3.0, 4.0), r(18.0, 25.0), r(30.0, 30.02), r(f64::NAN, 1.0)]),
        vec![r(0.0, 4.0), r(10.0, 25.0)]
    );
    assert_eq!(normalize(&[r(5.0, 6.0), r(6.005, 9.0)]), vec![r(5.0, 9.0)]);
    assert!(normalize(&[]).is_empty());
}

#[test]
fn filter_graph_fades_only_inner_edges() {
    let r = |start, end| Range { start, end };
    assert_eq!(
        filter_graph(&[r(1.0, 3.0), r(6.0, 10.0)]),
        "[0:a]atrim=start=1.000:end=3.000,asetpts=PTS-STARTPTS,afade=t=out:st=1.985:d=0.015[a0];\
         [0:a]atrim=start=6.000:end=10.000,asetpts=PTS-STARTPTS,afade=t=in:d=0.015[a1];\
         [a0][a1]concat=n=2:v=0:a=1[out]"
    );
    assert_eq!(
        filter_graph(&[r(28.0, 245.0)]),
        "[0:a]atrim=start=28.000:end=245.000,asetpts=PTS-STARTPTS[a0];[a0]concat=n=1:v=0:a=1[out]"
    );
}
```

- [ ] **Step 2: Run** `cargo test -p url2mp3-core cut` — Expected: FAIL (module missing).

- [ ] **Step 3: Implement `cut.rs`** (`normalize`, `filter_graph`; times printed with `{:.3}`; fade-out `st = (end-start) - 0.015`, clamped ≥ 0).

- [ ] **Step 4: Extend `transcode::encode`**: when `job.keep` is `Some(k)`, replace `-map 0:a:0` by `-filter_complex <filter_graph(k)> -map [out]` and skip the stream-copy branch (always encode with the format's codec/bitrate). Cover mapping (`-map 1:v:0 -filter:v crop…`) stays as is — verified to work alongside `-filter_complex`.

- [ ] **Step 5: Add `duration` and `extractor_key` to `Info`**; extend the existing `parses_flat_playlist` test JSON with `"duration":272.4,"extractor_key":"Youtube"` on entry A and assert both fields.

- [ ] **Step 6: Write `crates/core/tests/cut_ffmpeg.rs`** (`#![cfg(unix)]`): returns early with a `eprintln!` if `ffmpeg`/`ffprobe` are not on PATH. Generates `sine=f=440:d=10` as Opus via lavfi, encodes with `keep = [1..3, 6..10]` to MP3 with a lavfi-generated JPEG cover, asserts ffprobe duration in `5.95..=6.10` and that an attached picture stream exists. Test name: `trimmed_encode_keeps_only_the_kept_ranges`.

- [ ] **Step 7: Run** `cargo test -p url2mp3-core` — Expected: all pass (existing + new).

- [ ] **Step 8: Commit** `feat(core): cut ranges and trimmed encode`

---

### Task 2: Preview copy and waveform peaks

**Files:**
- Create: `crates/core/src/preview.rs`
- Modify: `crates/core/src/lib.rs` (`pub mod preview;` or re-export `preview::{make_preview, peaks}`)
- Test: unit tests in `preview.rs`; `crates/core/tests/cut_ffmpeg.rs` (add a real-ffmpeg test)

**Interfaces:**
- Consumes: `Tools::ffmpeg_command()` (make it `pub` if needed by the app; core-internal use is enough here).
- Produces:
  - `pub async fn make_preview(tools: &Tools, input: &Path, src_codec: &str, out: &Path) -> Result<()>` — AAC in MP4 (`-f ipod`); `-c:a copy` when `src_codec` starts with `mp4a` or is `aac`, else `-c:a aac -b:a 128k`; `-vn`.
  - `pub async fn peaks(tools: &Tools, input: &Path, buckets: usize) -> Result<Vec<f32>>` — runs `ffmpeg -i input -vn -ac 1 -ar 4000 -f s16le -` with stdout piped and read in chunks.
  - `pub(crate) struct PeakReducer` with `fn new(total_samples_hint: Option<u64>, buckets: usize)`, `fn push(&mut self, samples: &[i16])`, `fn finish(self) -> Vec<f32>` — peak |sample|/32768 per bucket, values in `0.0..=1.0`, exactly `buckets` values (pad with 0.0). Without a hint it collects per-0.1 s maxima (400 samples) and resamples to `buckets` at `finish`, so memory stays at ~36 000 floats for one hour.
  - Constant `pub const PEAK_BUCKETS: usize = 1200;`

- [ ] **Step 1: Failing unit tests**

```rust
#[test]
fn reducer_keeps_the_loudest_sample_per_bucket() {
    let mut r = PeakReducer::new(Some(8), 4);
    r.push(&[0, 16384, -32768, 0]);
    r.push(&[100, -100, 0, 8192]);
    assert_eq!(r.finish(), vec![0.5, 1.0, 100.0 / 32768.0, 0.25]);
}

#[test]
fn reducer_without_hint_is_bounded_and_sized() {
    let mut r = PeakReducer::new(None, 1200);
    let hour = vec![1000i16; 4000];
    for _ in 0..3600 { r.push(&hour); }
    let p = r.finish();
    assert_eq!(p.len(), 1200);
    assert!(p.iter().all(|v| (*v - 1000.0 / 32768.0).abs() < 1e-6));
}
```

- [ ] **Step 2: Run** `cargo test -p url2mp3-core preview` — Expected: FAIL.
- [ ] **Step 3: Implement `PeakReducer`, `peaks`, `make_preview`** (ffmpeg errors reported like `transcode::encode`: last stderr line).
- [ ] **Step 4: Add real-ffmpeg test** `preview_is_aac_and_peaks_have_1200_buckets` to `cut_ffmpeg.rs` (Opus sine source → preview codec `aac` per ffprobe; `peaks(...).len() == 1200`, max > 0.5).
- [ ] **Step 5: Run** `cargo test -p url2mp3-core` — PASS.
- [ ] **Step 6: Commit** `feat(core): AAC preview copy and waveform peaks`

---

### Task 3: SponsorBlock suggestions

**Files:**
- Create: `crates/core/src/sponsor.rs`
- Modify: `crates/core/Cargo.toml` (add `sha2 = "0.11.1"`, same version as the app), `crates/core/src/lib.rs` (`pub use sponsor::{Segment, suggestions};`)
- Test: unit tests in `sponsor.rs`

**Interfaces:**
- Produces:
  - `#[derive(Clone, Debug, PartialEq, Serialize)] pub struct Segment { pub start: f64, pub end: f64, pub category: String }`
  - `pub async fn suggestions(client: &reqwest::Client, video_id: &str) -> Vec<Segment>` — never errors: returns `vec![]` on any failure; 5 s timeout on the request.
  - `fn url(video_id: &str) -> String` and `fn parse(body: &str, video_id: &str) -> Vec<Segment>` (private, tested).

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn url_sends_only_a_hash_prefix() {
    let u = url("Llw9Q6akRo4");
    assert!(u.starts_with("https://sponsor.ajay.app/api/skipSegments/"));
    assert!(!u.contains("Llw9Q6akRo4"));
    let prefix = u.trim_start_matches("https://sponsor.ajay.app/api/skipSegments/").split('?').next().unwrap();
    assert_eq!(prefix.len(), 4);
    assert!(u.contains("categories=")); // the 7 categories, JSON array, URL-encoded
}

#[test]
fn parse_keeps_this_video_skip_segments_sorted() {
    let body = r#"[
      {"videoID":"other","segments":[{"category":"sponsor","actionType":"skip","segment":[1,2]}]},
      {"videoID":"abc","segments":[
        {"category":"outro","actionType":"skip","segment":[245.0,272.0]},
        {"category":"music_offtopic","actionType":"skip","segment":[0,28.4]},
        {"category":"sponsor","actionType":"mute","segment":[100,110]}]}]"#;
    assert_eq!(parse(body, "abc"), vec![
        Segment { start: 0.0, end: 28.4, category: "music_offtopic".into() },
        Segment { start: 245.0, end: 272.0, category: "outro".into() },
    ]);
    assert!(parse("not json", "abc").is_empty());
}
```

- [ ] **Step 2: Run** `cargo test -p url2mp3-core sponsor` — FAIL.
- [ ] **Step 3: Implement** (`sha2::Sha256`, hex of first 2 bytes; parse with `serde_json::Value` or small `Deserialize` structs).
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(core): SponsorBlock suggestions by hash prefix`

---

### Task 4: Engine stages — resolve, fetch, save — with `process` on top

**Files:**
- Modify: `crates/core/src/engine.rs`, `crates/core/src/lib.rs` (export `Track`, `Resolved`)
- Test: `crates/core/tests/engine.rs`

**Interfaces:**
- Consumes: `Cut`, `Range`, `cut::normalize` (Task 1); `transcode::Job.keep`.
- Produces (all on `Engine`; `id`/`parent` are caller-chosen, as today):
  - `#[derive(Clone)] pub struct Track { pub title: String, pub artist: Option<String>, pub thumbnail: Option<String>, pub duration: Option<f64>, pub video_id: Option<String>, pub youtube: bool, info: Arc<Info> }` with `pub fn needs_probe(&self) -> bool` (flat playlist reference) and `pub fn codec(&self) -> &str`.
  - `pub enum Resolved { Track(Track), Playlist { title: String, tracks: Vec<Track> } }`
  - `pub async fn resolve(&self, id: Id, input: &str) -> Result<Resolved>` — emits `Probing { id, parent: None, label: input }`; holds a semaphore permit while probing; a search (`ytsearch1:`) resolves to its single hit; `title` of a playlist is already `sanitize`+`fit` like today. Does **not** emit `Failed` (caller decides).
  - `pub async fn probe_track(&self, track: &Track) -> Result<Track>` — no-op clone unless `needs_probe()`; nested playlist → error "playlist lồng nhau…" (today's text).
  - `pub async fn fetch(&self, id: Id, track: &Track, to: &Path) -> Result<()>` — today's `fetch_audio` (direct parallel download, `Fallback` event, yt-dlp fallback) into a caller-owned path, emitting `Progress { id, … }`.
  - `pub fn whole_destination(&self, track: &Track, playlist: Option<&str>) -> PathBuf` — the path an untrimmed save would use (no claim), for the "already in your folder" note.
  - `pub async fn save(&self, id: Id, parent: Option<Id>, track: &Track, playlist: Option<&str>, audio: Option<&Path>, cut: Option<&Cut>) -> Outcome` — takes a semaphore permit; emits `Track`, then `Progress` (only when fetching), `Converting`, `Done`/`Skipped`/`Failed` exactly like today's `try_track`. With `audio: Some(p)` it never calls yt-dlp or the network for the audio (cover is still fetched). With `cut: Some(c)`: skip the `id:` claim and the exists check; name `"{title} ({label})"`, then `"{title} ({label} 2)"` … up to 99 while the path is claimed or exists; pass `normalize(&c.keep)` as `keep`; empty after normalize → `Failed` with message `"không còn đoạn nào để lưu"`.
  - `process` keeps its signature and observable events; rebuilt as `resolve` → (`Playlist` event + per-track `probe_track` + `save`) or `save`.

- [ ] **Step 1: Failing tests in `tests/engine.rs`** (reuse `fakes`, `engine`, `kinds` helpers; fake yt-dlp JSON gains `"duration":272,"extractor_key":"Youtube"`):

```rust
#[tokio::test]
async fn resolve_then_save_from_cached_audio_never_runs_ytdlp_download() {
    // FAKE_LOG records every yt-dlp call; cached audio written by the test.
    // resolve(…) → Resolved::Track with duration Some(272.0), youtube true, video_id "abc"
    // save(id, None, &t, None, Some(&cached), None) → Outcome::Done; out/Fake Song.mp3 == "cached"
    // log has exactly one line (the -J probe), no "-o"
}

#[tokio::test]
async fn trimmed_save_sits_next_to_the_whole_one_and_is_numbered() {
    // save whole → "Fake Song.mp3"; save with Cut{keep:[0..10], label:"đã cắt"} → "Fake Song (đã cắt).mp3";
    // again → "Fake Song (đã cắt 2).mp3"; all three Outcome::Done (no Duplicate/Exists skip)
}

#[tokio::test]
async fn trimmed_save_with_nothing_kept_fails() {
    // Cut{keep: vec![], label} → Outcome::Failed, event message "không còn đoạn nào để lưu"
}

#[tokio::test]
async fn whole_destination_matches_where_a_whole_save_lands() { /* equal paths, playlist subfolder too */ }

#[tokio::test]
async fn cancelled_fetch_leaves_no_files() {
    // fake yt-dlp with FAKE_SLEEP=5 in fallback; abort the fetch task after 100 ms;
    // the `to` path does not exist (fetch writes to `<to>.part` and renames on success)
}
```

Fake ffmpeg already copies the first `-i` input to the last argument, which stays the output path with `-filter_complex`.

- [ ] **Step 2: Run** `cargo test -p url2mp3-core --test engine` — FAIL (methods missing).
- [ ] **Step 3: Implement the stages** in `engine.rs`, moving `try_track`'s body into `save` (scratch files for cover/encoded stay in the engine temp dir; audio from `audio` when given, else fetched into scratch as today). `fetch` writes `<to>.part` then renames.
- [ ] **Step 4: Rebuild `process` on the stages**; run the whole file — all old tests (`existing_file_is_skipped`, `same_video_twice_in_a_session_is_a_duplicate`, playlist subfolder, retry…) PASS unchanged.
- [ ] **Step 5: Run** `cargo test --workspace && cargo clippy --workspace --all-targets` — PASS, no warnings.
- [ ] **Step 6: Commit** `feat(core): resolve/fetch/save stages for the app's tray`

---

### Task 5: Tray in the Tauri backend

**Files:**
- Create: `app/src-tauri/src/tray.rs` (tray store + cache files)
- Modify: `app/src-tauri/src/jobs.rs` (commands' logic; running map keyed by tray id only), `app/src-tauri/src/lib.rs` (commands), `app/src-tauri/tauri.conf.json` (CSP `media-src 'self' blob:`)
- Test: unit tests in `tray.rs`

**Interfaces:**
- Consumes: `Engine::{resolve, probe_track, fetch, save, whole_destination}`, `Track`, `Resolved`, `Cut`, `preview::{make_preview, peaks, PEAK_BUCKETS}`, `sponsor::suggestions` (Tasks 1–4).
- Produces (Tauri commands; tray ids are one `AtomicU64` in `Jobs`, unique across engine generations, used as the engine `Id` for every call):
  - `add(inputs: Vec<String>) -> Result<usize, String>` — per input: spawn `resolve`; emits on channel `tray`:
    `{kind:"added", id, parent: number|null, input: string|null, title, artist, thumbnail, duration, youtube}` (playlist children have `parent` = the group id and `input: null`),
    `{kind:"playlist", id, input, title, count}` (before its children).
    Probing/failure go on `engine` as `probing`/`failed` with that id (Failed built by the app from the error).
  - `prepare(id) -> Result<Prepared, String>` with `#[serde(rename_all="camelCase")] struct Prepared { duration: Option<f64>, peaks: Vec<f32>, suggestions: Vec<Segment>, exists: bool }` — probe if needed (then emits `added` again with the resolved fields), fetch into `<app_cache_dir>/tray/<id>.audio` unless present (progress on `engine` as `progress`), `make_preview` → `<id>.m4a`, `peaks`, `suggestions` (only when `youtube`) and the exists check run concurrently. Registered in the running map so `discard`/`cancel` abort it.
  - `preview_audio(id) -> Result<tauri::ipc::Response, String>` — bytes of `<id>.m4a`.
  - `save(items: Vec<SaveItem>) -> Result<(), String>` with `#[derive(Deserialize)] struct SaveItem { id: u64, cut: Option<Cut> }` — spawns `engine.save(id, parent, track, playlist_title, cached_audio, cut)` per item using the **current** engine; events on `engine` as today. Deletes `<id>.audio`/`<id>.m4a` after `Done`/`Skipped`. Tray entry kept until `discard` (Retry = `save` again; with the cache gone it fetches anew).
  - `discard(ids: Vec<u64>)` — aborts tasks, deletes cache files, forgets entries (a group id discards its children).
  - `cancel(id: u64)` — as today, without `generation`; emits `{kind:"cancelled", id}` on `engine`.
  - Engine payload becomes `{ event }` (no `gen`); `enqueue` command removed.
  - Startup and `shutdown()`: `remove_dir_all(<app_cache_dir>/tray)`.
  - `tray.rs`: `pub struct Tray` with `insert(id, Entry)`, `get(id) -> Option<Entry>`, `remove(id) -> Vec<u64>` (returns the id and its children), `paths(id) -> (PathBuf, PathBuf)`; `Entry { track: Track, parent: Option<u64>, playlist: Option<String> }`.

- [ ] **Step 1: Failing unit tests in `tray.rs`**: `remove_group_removes_children` (insert group 1 with children 2,3 → `remove(1)` returns `[1,2,3]` in any order, `get(2)` is None) and `paths_live_under_the_tray_dir` (`paths(7)` → `…/tray/7.audio`, `…/tray/7.m4a`). `Entry` construction in tests needs a `Track`; add `#[cfg(test)] Track::for_tests(title)` in core (public only under `cfg(any(test, feature = "test-util"))`) or test `Tray` generically over the entry type — builder's choice.
- [ ] **Step 2: Run** `cargo test -p url2mp3-app tray` — FAIL.
- [ ] **Step 3: Implement `tray.rs`, the commands and wiring**; drop `generation` from the running map key and the `engine` payload.
- [ ] **Step 4: Run** `cargo test --workspace && cargo clippy --workspace --all-targets` — PASS.
- [ ] **Step 5: Commit** `feat(app): tray commands, preview audio and cache`

---

### Task 6: Trim model (pure TypeScript)

**Files:**
- Create: `app/src/cut.ts`, `app/src/cut.test.ts`

**Interfaces:**
- Produces:
  - `export interface Range { start: number; end: number }`
  - `export function normalize(removed: Range[], duration: number): Range[]` — clamp to `[0, duration]`, drop < 0.05 s, sort, merge overlapping/touching.
  - `export function kept(removed: Range[], duration: number): Range[]` — complement of `normalize(removed)` within `[0, duration]`, dropping kept slivers < 0.05 s.
  - `export function keptLength(removed: Range[], duration: number): number`
  - `export function skipTarget(t: number, removed: Range[]): number | null` — inside a removed range → its end; else null.
  - `export function nudge(r: Range, edge: "start" | "end", delta: number, duration: number): Range` — keeps `end - start ≥ 0.05`.
  - `export function formatTime(sec: number): string` — `m:ss`, hours as `h:mm:ss`, unknown/NaN → `"–:––"`.
  - `export class History` with `push(removed: Range[])`, `undo(): Range[] | null`, `current(): Range[]`.

- [ ] **Step 1: Failing tests** (values from the mockup: duration 272, removed `[0,28]`, `[98,128]`, `[245,272]`):

```ts
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
it("preview jumps over removed parts", () => {
  expect(skipTarget(10, [{ start: 0, end: 28 }])).toBe(28);
  expect(skipTarget(30, [{ start: 0, end: 28 }])).toBeNull();
});
it("nudge never inverts a range", () => {
  expect(nudge({ start: 10, end: 10.5 }, "start", 1, 272)).toEqual({ start: 10.45, end: 10.5 });
  expect(nudge({ start: 0.4, end: 5 }, "start", -1, 272)).toEqual({ start: 0, end: 5 });
});
it("undo walks back", () => {
  const h = new History(); h.push([{ start: 0, end: 1 }]); h.push([]);
  expect(h.undo()).toEqual([{ start: 0, end: 1 }]); expect(h.undo()).toEqual([]); expect(h.undo()).toBeNull();
});
```

- [ ] **Step 2: Run** `cd app && pnpm test cut` — FAIL.
- [ ] **Step 3: Implement `cut.ts`.**
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(ui): trim model`

---

### Task 7: Tray state, API types, mock backend

**Files:**
- Modify: `app/src/api.ts` (types + commands from Task 5), `app/src/state.ts`, `app/src/state.test.ts`, `app/dev/mock.ts`

**Interfaces:**
- Consumes: Task 5 commands/events; `Range` from `cut.ts`.
- Produces:
  - `api.ts`: `TrayEvent` (`added` | `playlist` as in Task 5), `Prepared`, `Segment`, `SaveItem { id: number; cut: { keep: Range[]; label: string } | null }`; `api.add(inputs)`, `api.prepare(id)`, `api.previewAudio(id): Promise<ArrayBuffer>`, `api.save(items)`, `api.discard(ids)`, `api.cancel(id)`, `api.onTray(fn)`; `EnginePayload` is `{ event: EngineEvent }`; `enqueue` removed.
  - `state.ts`: rows/groups keyed by `String(id)`. `Status` gains `"ready"`. `Row` gains `duration?: number`, `youtube?: boolean`, `removed: Range[]` (default `[]`), `preview?: { phase: "loading" | "ready" | "failed"; done: number; total: number; message?: string }`. New `Msg`s: `{kind:"tray", event: TrayEvent}`, `{kind:"cut", key, removed}`, `{kind:"preview", key, phase, message?}`, `{kind:"saving", keys}` (rows → `probing`-like `"downloading"` with 0/0). `progress` events on a `ready` row update `preview.done/total` and keep `status: "ready"`. `remove` on a ready row is allowed. New selector `ready(state): string[]` — keys of ready rows (playlist children included) in list order.
  - `mock.ts`: answers `add` by emitting a single YouTube track (duration 272) or a 3-track playlist when the input contains "playlist"; `prepare` returns 1200 synthetic peaks and the two mockup suggestions; `preview_audio` returns a generated 272 s mono WAV (8 kHz sine, quiet), built once; `save` replays track→progress→converting→done events.

- [ ] **Step 1: Failing reducer tests** (`state.test.ts`, rewrite the `run` helper without `gen`):
  - `"added" puts a ready row with duration and no cuts` → `{status:"ready", duration:272, removed:[]}`, `ready(s)` = `["0"]`.
  - `playlist then children → a group of ready rows`, `ready(s)` lists the 3 children.
  - `cut stores removed ranges`; `progress on a ready row fills preview, status stays ready`.
  - `previewFailed keeps the row ready with the message`.
  - `saving then done ends done with the path`.
  - `added without duration renders as unknown` (`duration` undefined → `formatTime` gives `"–:––"`).
  - Existing single-track/playlist/cancel tests adapted to string keys still pass.
- [ ] **Step 2: Run** `pnpm test state` — FAIL.
- [ ] **Step 3: Implement `api.ts` + `state.ts` changes.**
- [ ] **Step 4: Run** `pnpm test` — PASS (i18n test too).
- [ ] **Step 5: Update `mock.ts`** and check with `pnpm dev` → `http://localhost:1420/dev/mock.html` that the page loads without console errors (UI wiring comes in Task 8; ready rows appear once Task 8 lands).
- [ ] **Step 6: Commit** `feat(ui): tray state and API`

---

### Task 8: Tray list, trim dialog and wiring

**Files:**
- Create: `app/src/trim.ts` (dialog controller: waveform canvas, pointer selection, playback)
- Modify: `app/index.html` (`<dialog id="trim">` markup; main button label; tray footer `#save-all`), `app/src/main.ts`, `app/src/style.css`, `app/src/i18n.ts`

**Interfaces:**
- Consumes: Tasks 5–7.
- Produces: `export function openTrim(row: Row, deps: { language: Language; dispatch: (m: Msg) => void; onSave: (item: SaveItem) => void })` in `trim.ts`.

Behaviour to implement (spec "The flow" + "Goals"):
- Main button and ⌘/Ctrl+Enter call `api.add`. Exactly one input that becomes one track (an `added` with `parent === null` whose `input` equals the single submitted input) → `openTrim` on it. Android share → same path (replace the direct `enqueue` in `takeShared`).
- Ready rows show duration (or after cuts `4:32 → 3:07`), a tag: `whole track` / `N parts removed` / `suggested: remove m:ss` (after `prepare` returned suggestions not yet accepted), buttons: open (scissors icon), remove. Tapping the row opens the trim dialog. Footer `#save-all` "● Download N tracks" (`N = ready(state).length`), hidden when 0; sends one `SaveItem` per ready row (`cut` only when it has removed ranges; `label` = `t("trimmedSuffix")`).
- Trim dialog: on open calls `api.prepare(id)` (loading state with % from `preview`), then `api.previewAudio(id)` → `Blob` → `URL.createObjectURL` into an `<audio>` (revoked on close). Canvas waveform from `peaks`, ruler `0:00 … duration`, removed ranges striped red, suggestions dashed amber with chip "Found N non-music parts: … [Remove all] [No]", selection by pointer drag (Pointer Events, works with touch), handles 24 px hit area, edge drag on a removed range adjusts it, "Remove selected part", ±1 s nudge for the selected edge, Undo (`History`), Play/Pause skipping removed ranges via `skipTarget` in a `requestAnimationFrame` loop, kept length text, "Already in your folder — the trimmed copy is saved alongside" when `exists`. Buttons "Download whole track (m:ss)" and "● Download trimmed (m:ss)" (disabled when nothing removed or nothing kept). Duration = `prepared.duration ?? audio.duration`.
- Closing the dialog keeps the cuts (dispatch `cut`); saving closes it.
- i18n keys (vi / en), at least: `add` ("Thêm vào khay"/"Add to tray"), `saveAll` ("● Tải {n} bài"/"● Download {n} tracks"), `status.ready` ("Sẵn sàng"/"Ready"), `tag.whole`, `tag.removed` ("Đã bỏ {n} đoạn"/"{n} parts removed"), `tag.suggested`, `trim.loading`, `trim.failed`, `trim.retry`, `trim.play`, `trim.pause`, `trim.remove`, `trim.undo`, `trim.saveWhole`, `trim.saveCut`, `trim.kept` ("Giữ {kept} · bỏ {cut}"), `trim.exists`, `trim.suggest`, `trim.acceptAll`, `trim.dismiss`, `trimmedSuffix` ("đã cắt"/"trimmed"), category labels `sb.music_offtopic`, `sb.sponsor`, `sb.intro`, `sb.outro`, `sb.selfpromo`, `sb.interaction`, `sb.preview`, and `emptyHint` reworded for the tray.

- [ ] **Step 1: Add i18n keys** in both dictionaries; run `pnpm test i18n` — PASS (equal keys).
- [ ] **Step 2: Markup + styles** for tray rows, `#save-all`, `#trim` dialog (tape-deck look, light/dark, 420 px wide, phone portrait full-width sheet).
- [ ] **Step 3: Implement `trim.ts` and the `main.ts` wiring** (submit → `add`, auto-open, `takeShared`, save-all, row actions, `cancel(id)`, retry on a failed saved row → `api.save` with its stored cut).
- [ ] **Step 4: Check in the mock**: `pnpm dev`, open `/dev/mock.html`; paste a link → trim dialog opens, suggestions chip shows, Remove all → kept `3:37`, play skips the first 28 s, Download trimmed → row ends "Xong"; paste a playlist link → 3 ready rows + "● Tải 3 bài". Take screenshots at 420 px and 1000 px width, light and dark.
- [ ] **Step 5: Run** `pnpm test && pnpm build` — PASS (tsc clean).
- [ ] **Step 6: Commit** `feat(ui): tray list and trim dialog`

---

### Task 9: Docs and end-to-end check

**Files:**
- Modify: `README.md`, `README.vi.md` ("App" section: tray, trim screen, SponsorBlock suggestions and that only a 4-character hash prefix is sent; `pnpm dev` mock note still true), `THIRD_PARTY.md` (SponsorBlock data, CC BY-NC-SA 4.0, https://sponsor.ajay.app), spec status line → "implemented".

- [ ] **Step 1: Update the three docs**; `rg -n "enqueue|Tải về" README*.md docs app/src` shows no stale references to removed behaviour.
- [ ] **Step 2: Full checks**: `cargo test --workspace`, `cargo clippy --workspace --all-targets`, `cd app && pnpm test && pnpm build` — all PASS.
- [ ] **Step 3: Real app on macOS** (`pnpm tauri dev`): YouTube music video with an intro → suggestions shown, cut start/middle/end, saved file duration (ffprobe) equals the shown kept length ±0.1 s and has no click at joins; "Download whole" then "Download trimmed" → both files; change format MP3→M4A with a prepared row, save → no network activity for the audio (log shows no yt-dlp download); 20-track playlist saved without opening rows; remove a row during preview fetch → no leftover file in the cache dir; TikTok link without SponsorBlock.
- [ ] **Step 4: Android** (`pnpm tauri android build --apk --split-per-abi --target aarch64`, install): Share → url2mp3 from YouTube opens the trim dialog; touch selection and nudge work in portrait; trimmed file appears in `Download/url2mp3`. Windows: if no machine is available, say so in the final report instead of claiming it.
- [ ] **Step 5: Commit** `docs: tray and trim screen`
