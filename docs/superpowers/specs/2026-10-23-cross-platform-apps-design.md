# url2mp3 apps for Windows, macOS and Android — design

Date: 2026-10-23 · Status: approved in brainstorming, pending spec review

## Goal

Ship url2mp3 as graphical apps anyone can download from GitHub Releases: Windows, macOS
(Apple Silicon and Intel) and Android. Each app runs yt-dlp and ffmpeg on the user's own device;
there is no server. The existing `url2mp3` CLI keeps working unchanged.

## Decisions

| Topic | Decision | Why |
|---|---|---|
| Framework | Tauri 2 for all three platforms, one shared web UI | One codebase; reuses the Rust core |
| iOS | Not supported | Sideload-only outside the App Store, App Store rejects YouTube downloaders, iOS cannot spawn yt-dlp |
| Code signing | None. README explains first launch; Homebrew cask for macOS | Free; user's choice |
| UI language | Vietnamese + English, follows the OS, switchable in Settings | Public release |
| First app release | `v0.2.0` | CLI is at 0.1.0 |

## 1. Architecture

```
url2mp3/                       Cargo workspace
├── crates/core/   resolve (yt-dlp -J), download (parallel HTTP Range), transcode (ffmpeg)
├── crates/cli/    the `url2mp3` command (indicatif renders core events)
└── app/           Tauri 2
    ├── src-tauri/ commands + event bridge to the UI
    ├── ui/        TypeScript + Vite, no heavy framework
    └── android/   Kotlin plugin around youtubedl-android
```

### core

- Moves today's `resolve.rs`, `download.rs`, `transcode.rs` and the per-track pipeline out of
  `main.rs` into a library. Behaviour is unchanged: parallel ranged download with resume,
  `Content-Range` validation, fallback to yt-dlp's downloader on 403/HLS/DASH, skip existing
  files, retry on DNS blips, stream copy when codecs match.
- `Tools { ytdlp, ffmpeg, js_runtime }` holds explicit executable paths (plus any env vars the
  platform needs) instead of looking them up on `PATH`. The CLI builds it from `PATH`; the app
  builds it from bundled/updated copies.
- Progress is reported as typed events on a channel (`Resolving`, `Resolved{title, thumb,
  playlist}`, `Downloading{done, total}`, `Converting`, `Done{path}`, `Skipped{path}`,
  `Failed{kind, message}`) instead of driving `indicatif` directly. Cancellation through a
  token per job.

### cli

Thin wrapper: clap args → `core` → indicatif bars from events. Same flags, same messages, same
exit codes. All current `cargo test` cases must pass after the split.

### app (Tauri)

- Commands: `enqueue(inputs)`, `cancel(id)`, `retry(id)`, `reveal(path)`, `get_settings`,
  `set_settings`, `tools_status`, `update_ytdlp`, `copy_log`.
- Core events are forwarded to the UI as Tauri events keyed by job id.
- Settings persisted as JSON in the app data dir.

### External tools

- **Desktop:** ffmpeg and QuickJS (`qjs` from quickjs-ng) ship as Tauri sidecars (they rarely
  need changing). yt-dlp is **not** bundled: on first launch the app downloads yt-dlp's
  *onedir* build (`yt-dlp_macos.zip` / `yt-dlp_win.zip`) into the app data dir, and once a day
  checks for a newer release. Every download is verified against the release's
  `SHA2-256SUMS`, unpacked into a versioned folder, then switched to atomically. Settings shows
  the version and an "Update now" button. Reason: YouTube breaks yt-dlp every few weeks; users
  must not need a new app build for that.
- **Measured 2026-10-23 (M-series Mac), why these choices:**
  - The onefile `yt-dlp_macos` takes ~13 s to start *every* run (it unpacks to a new temp
    dir that macOS scans each time). The onedir build pays ~14 s once, then starts in 0.25 s.
    Onedir builds refuse `yt-dlp -U` ("unpackaged executables"), hence our own updater.
  - QuickJS resolves a YouTube link in 5.2 s vs 3.2 s with deno (first time per player
    version; yt-dlp caches the result), but is 1.3 MB instead of 124 MB. The same runtime
    then serves Android, where deno is not an option.
- **Android:** the Kotlin plugin initialises `youtubedl-android` (Python, yt-dlp, ffmpeg as
  `.so` files in `nativeLibraryDir`, which Android allows to execute) and returns the paths and
  env the core needs. yt-dlp updates through the library's `updateYoutubeDL()`.
- **Open risk, Android JS runtime:** yt-dlp needs a JavaScript runtime to solve YouTube's
  signature challenge. Desktop uses QuickJS. On Android this must be proven first (spike):
  QuickJS packaged as an executable `.so`, recognised by yt-dlp. If the spike fails,
  Android falls back to a native Kotlin/Compose app on youtubedl-android (as Seal does);
  desktop is unaffected.
- **Licences:** ffmpeg (with LAME) is GPL and is distributed as a separate executable, with
  `THIRD_PARTY.md` listing each tool's licence and source link. The app stays MIT. yt-dlp is
  Unlicense, QuickJS MIT.

## 2. UI and v1 features

```
┌─────────────────────────────────────────────┐
│ url2mp3                          [MP3 ▾] ⚙  │
├─────────────────────────────────────────────┤
│ ┌─────────────────────────────────────────┐ │
│ │ Paste links or type a song name…        │ │
│ └─────────────────────────────────────────┘ │
│                                 [Download]  │
├─────────────────────────────────────────────┤
│ 🖼 Never Gonna Give You Up    ████████░ 82% │
│    Rick Astley · converting to MP3     [✕]  │
│ 🖼 Track B                     ✓ Done  [📂] │
│ 🖼 Track C                     ⚠ Error [↻]  │
│ ▸ Playlist "Chill" — 12/20                  │
└─────────────────────────────────────────────┘
```

In v1:

- Input: one or many links (one per line) or a search phrase (first YouTube hit, as in the CLI).
- Queue rows: thumbnail, title, artist, progress, status (resolving / downloading / converting /
  done / already exists / error), with cancel, retry and show-in-folder.
- Playlists appear as one collapsible group with done/total.
- Format switch (MP3 / M4A / Opus) in the top bar.
- Settings: bitrate; output folder (desktop default `~/Downloads`; Android default
  `Music/url2mp3` via MediaStore so music players see the files); tracks at once; connections
  per track; playlist limit; embed cover; language; yt-dlp version + update; browser cookies
  (desktop only).
- Light/dark follows the system.
- New-version banner: checks GitHub Releases' latest tag and links to it (no self-install).
- Android only: receives links from the system Share sheet; runs the queue in a foreground
  service with a progress notification so it keeps going when the user switches apps.

Not in v1: persistent history, built-in player, app self-update, iOS.

## 3. Release, errors, tests

### Release workflow (GitHub Actions, on tag `v*`)

| Job | Runner | Output |
|---|---|---|
| Windows | `windows-latest` | `url2mp3_<v>_x64-setup.exe`, `.msi` |
| macOS | `macos-latest`, cross-builds x86_64 | `url2mp3_<v>_aarch64.dmg`, `url2mp3_<v>_x64.dmg` |
| Android | `ubuntu-latest` | `url2mp3_<v>.apk` (arm64-v8a + armeabi-v7a) |
| CLI | Windows, macOS, Linux | `url2mp3-cli-<os>-<arch>` archives |

- The workflow creates a **draft** release; the owner publishes it after a smoke test.
- The APK is signed with a self-generated keystore stored in GitHub Secrets. The owner must back
  it up: losing it means existing installs cannot be updated.
- Sidecar binaries (ffmpeg, qjs) are fetched in CI from pinned versions with checksums.
- Homebrew tap `lecaotanloc289/homebrew-tap` with a cask whose postflight clears the quarantine
  attribute. Creating that repo is confirmed with the owner first.
- A separate CI workflow on push/PR runs `cargo test`, `cargo clippy` and the UI tests.

### Errors

- yt-dlp errors are mapped to localised, actionable messages: bot check → update yt-dlp or use
  cookies; unavailable/private video; network → one automatic retry, then the ↻ button.
- A failure stays on its own row; the rest of the queue continues.
- Logs go to a file in the app data dir; Settings has "Copy log" for GitHub issues.

### Tests

- core: all existing tests, plus tests for `Tools` and the event stream using fake yt-dlp/ffmpeg
  scripts.
- UI: vitest for the queue state reducer.
- Manual checklist per platform before publishing: YouTube link, TikTok link, playlist, search
  phrase, already-downloaded track, cancel mid-download, yt-dlp update.

### Docs

README and README.vi gain a download section (per-platform files, first-launch steps for
Gatekeeper and SmartScreen, Homebrew line) and a short architecture note for the workspace.
New `THIRD_PARTY.md`.

## Build order

1. Split into a workspace (core + cli), no behaviour change.
2. Desktop app (Tauri + UI + tool management).
3. Release CI for Windows/macOS/CLI; first draft release.
4. Android spike: yt-dlp + JS runtime resolving a YouTube link on a real device.
5. Android app (plugin, share intent, foreground service, MediaStore) and APK in CI.
6. Homebrew tap, docs, `v0.2.0`.
