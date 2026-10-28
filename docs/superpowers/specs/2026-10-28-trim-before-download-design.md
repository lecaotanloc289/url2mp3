# Listen, trim and join before saving — design

Date: 2026-10-28 · Status: agreed in brainstorming, builder-checked, pending approval

## Why

Many tracks people fetch from YouTube/TikTok carry parts they don't want: a spoken intro, a
sponsor read in the middle, an outro or a long silence. Today the app saves the whole track and
the user hears those parts every time. The user wants every app (macOS, Windows, Android) to let
people **listen first, cut out any parts — at the start, in the middle, at the end — join what
is left, and only then save the file**. Making the listening step part of the normal flow is
deliberate: people should discover that the app can cut.

Success: a user pastes a music video link, hears it, drops the intro and a mid-roll ad in a few
drags, presses one button and gets one clean file. A user pasting a 30-track playlist is not
slowed down: one press saves them all.

## Goals

- A **"ready to download" tray** between pasting and saving. Nothing is written to the output
  folder until the user presses Download.
- A **trim screen** for one track: waveform with a time ruler, play/pause, drag to select a
  range, "Remove this part", drag the edges of a removed part to adjust, ±1 s nudge buttons
  (precise on a phone), undo. Any number of removed parts anywhere. The kept parts are joined
  into one file. Shows the new length.
- **Preview skips removed parts**, so the user hears exactly what will be saved.
- **Suggestions for YouTube** from SponsorBlock (non-music sections, intro, outro, sponsor,
  self-promo…): shown as "Found 2 non-music parts: 0:00–0:28 (intro), 4:05–4:32 (outro) —
  [Remove both] [No]". Never applied without the user's click.
- Two save buttons on the trim screen: **"Download whole track (4:32)"** and **"● Download
  trimmed (3:07)"**.
- Same feature on macOS, Windows and Android, one shared UI.

## Non-goals

- The CLI (`crates/cli`) gets no listening/trimming step and its behaviour must not change.
- No editing beyond removing ranges: no fades the user controls, no volume, no reordering
  parts, no splitting one video into several files.
- The tray is not saved across app restarts.
- No setting to skip the tray and download straight away (can come later if asked for).

## Anti-goals (would be a failure even if it works)

- The listening step makes plain downloading slower or more tedious, especially for
  playlists and many links.
- A suggestion is applied silently and the user loses music they wanted.
- The saved file differs from what the preview played (wrong cut points, clicks at joins).
- Audio is downloaded twice: once to preview, again to save.

## The flow (decided with the user — option "C" of the mockups)

Mockups: `.superpowers/brainstorm/18191-1793185972/content/trim-flow.html` (local, not
committed).

1. The user pastes one or more links / search phrases and presses the main button (today
   "Tải về"; it becomes "Add to tray"-style wording — exact copy is the builder's, in vi + en).
   Each input is probed (yt-dlp `-J`, as today) and appears in the tray: cover, title, length
   and a status tag ("whole track", "2 parts removed", "suggested: remove 0:40").
2. **Exactly one track added → its trim screen opens by itself.** This covers the Android
   *Share → url2mp3* path too (my call: share behaves like pasting one link, instead of
   downloading at once as today).
3. **Several links or a playlist → they wait in the tray.** Tapping a row opens its trim
   screen. A playlist shows its tracks as rows (grouped as today).
4. A tray row can be removed before downloading.
5. **"● Download N tracks"** at the bottom of the tray saves everything in it; each track keeps
   the cuts set for it, untouched ones are saved whole. The trim screen's own buttons save
   just that track.
6. After the press, rows go through today's progress states (downloading, converting, done,
   skipped, failed, cancelled…) in the existing track list. Retry, cancel, reveal keep
   working.

## Constraints and decisions

- **No double download.** When a track enters the tray as a single link (and whenever its
  trim screen is opened), its audio is downloaded in the background into a temp cache, using
  the existing parallel downloader / yt-dlp fallback. Saving then only runs ffmpeg on the
  cached audio. For playlists and multi-link adds, audio is fetched only for rows the user
  opens (my call: saves bandwidth on long playlists); the rest download when Download is
  pressed, exactly as today.
- **Preview must play in all three webviews**: WKWebView (macOS), WebView2 (Windows), Android
  WebView. YouTube's Opus-in-WebM is not reliably playable in WKWebView; the builder picks
  how (e.g. remux/transcode a preview copy with ffmpeg, or play through the Tauri asset
  protocol with a format each webview supports) and verifies on each platform.
- **Waveform**: computed from the cached audio with ffmpeg (decode to low-rate mono PCM, reduce
  to a fixed number of peaks) on the Rust side, so it works the same on Android (ffmpeg there
  comes from youtubedl-android through `Tools`). The UI only draws peaks.
- **Duration** comes from yt-dlp's JSON (`duration`), added to `resolve::Info`; the trim
  screen can show the ruler before the waveform is ready.
- **Cutting**: the user's removed ranges become a list of kept ranges `[start, end)`; ffmpeg
  keeps those and joins them in one encode pass (`atrim`/`concat` or `aselect`, builder's
  choice) alongside today's cover + tags. A trimmed track is always re-encoded (stream copy
  cannot cut precisely); untrimmed tracks keep today's copy-when-codecs-match behaviour.
  Joins must not click: a very short fade (~10–20 ms) at each join is the builder's call.
- **Removed ranges are clamped and merged** (overlaps, touching ranges, ranges past the end).
  Removing everything is not allowed (Download trimmed disabled).
- **SponsorBlock**: only for YouTube videos. Query the public API with the privacy-preserving
  hash-prefix endpoint (`/api/skipSegments/<first 4 hex of sha256(videoId)>`) so the video id
  is not sent; take categories that are not music (`music_offtopic`, `sponsor`, `intro`,
  `outro`, `selfpromo`, `interaction`, `preview`). Any error or timeout → no suggestions,
  silently. Suggestions appear as dashed amber ranges on the waveform until accepted.
- **Settings at save time**: format, bitrate, cover, output folder are read when Download is
  pressed, not when the track entered the tray (my call). Changing settings or a yt-dlp update
  while items sit in the tray must not lose their cached audio or cuts.
- **Whole-track saves keep today's rules**: skip if the file exists (unless `force`), the same
  video twice is fetched once, names fitted to the path limit, playlist tracks into the
  playlist folder.
- **A trimmed save never overwrites and is never skipped** (decided by the user: keep both).
  It gets its own name, `<title> (đã cắt).<ext>` / `<title> (trimmed).<ext>` following the UI
  language (suffix wording is my call); if that name is taken too, number it (`(đã cắt 2)`, …).
  So "Download whole" then "Download trimmed" on the same screen, re-pasting an already saved
  song to trim it, or trimming a song already in the folder all produce a second file next to
  the whole one. The session's "same video already done" claim must not block a trimmed save;
  core needs to let a trimmed save bypass the video-id claim (it still claims its own path).
- **The trim screen says when the whole track already exists** in the output folder ("Already
  in your folder — the trimmed copy is saved alongside"), checked when the screen opens with
  the current folder/format; it is informational only.
- **Cached stream vs format change**: the cached audio is the stream yt-dlp picked for the
  format in effect when it was fetched (today's selector). If the format setting changes before
  saving, the save re-encodes from the cached audio; it never downloads again (my call: a rare
  case, and "no double download" wins over keeping the original AAC/Opus untouched).
- **Temp files**: cached audio, preview copies and peaks live in a temp folder that is cleaned
  when the row is removed, after saving, on app exit, and by the existing stale-temp sweep if
  the app was killed.
- **Android**: downloads continue in the foreground service as today; the trim screen must
  work with touch (large handles, nudge buttons), in portrait at phone width.
- **Look**: follow the existing tape-deck style in `app/src/style.css` (espresso chassis,
  amber, red REC). Removed parts: red diagonal stripes; selection: white frame with handles;
  playhead: green. Both light and dark schemes.
- **Languages**: every new string in vi and en (`app/src/i18n.ts`).

## Architecture sketch (builder may reshape)

- **core**: split today's one-shot `Engine::process` into stages the app can drive — *probe*
  (as today, emitting tray-ready info incl. duration), *fetch audio* (into a cache the caller
  owns), *peaks*, *save* (encode from cached audio with optional kept ranges, or the full
  pipeline when nothing is cached). `process` keeps working unchanged for the CLI on top of
  these. New pure function for removed → kept ranges, unit-tested.
- **app/src-tauri**: new commands roughly `add(inputs)` (probe into tray), `prepare(key)`
  (fetch + preview + peaks), `suggestions(key)`, `save(keys with cuts)`, `discard(key)`;
  events for tray rows and preview readiness. Keyed like today (`gen:id`) or by a tray id that
  survives engine generations — builder's call, given the constraint above.
- **app/src**: tray state in the pure reducer style of `state.ts` (with vitest tests); trim
  screen as its own module; `dev/mock.ts` gains the new commands so the UI can be worked on in
  a browser.

## Tests

- core: kept-range computation (clamp, merge, edge cases); ffmpeg argument building for cuts;
  engine stages with the existing fake yt-dlp/ffmpeg scripts; CLI tests unchanged and passing.
- UI: reducer tests for the tray (add, open, cut, undo, remove, save all) and the trim model.
- Manual on macOS, Windows, Android: single YouTube link with SponsorBlock suggestions, TikTok
  link, 20-track playlist saved without opening any row, cut at start/middle/end and confirm
  the saved file matches the preview, share from YouTube on Android.

## Docs

README.md and README.vi.md "App" section: the tray, the trim screen, SponsorBlock suggestions
(and that it sends only a hash prefix). THIRD_PARTY.md if SponsorBlock data terms need a
mention (SponsorBlock data is CC BY-NC-SA 4.0 — attribution in the app/README).

## Left to the builder

Exact wording, layout details within the mockup's direction, how preview audio is made
playable per webview, the ffmpeg filter used for joining, fade length, peaks resolution, how
tray ids survive engine generations, command/event names.
