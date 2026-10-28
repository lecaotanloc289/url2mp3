# url2mp3

**English** · [Tiếng Việt](README.vi.md)

Download music from YouTube, TikTok and the ~1,800 other sites yt-dlp supports (SoundCloud,
Facebook…) as MP3, M4A or Opus, with cover art and tags. No ads, no queue, several tracks at
once and several connections per track. It comes as an app for macOS, Windows and Android, and
as a terminal command.

## Download

From the [latest release](https://github.com/lecaotanloc289/url2mp3/releases/latest):

| System | File |
|---|---|
| macOS, Apple Silicon (M1 and later) | `url2mp3_<version>_aarch64.dmg` |
| macOS, Intel | `url2mp3_<version>_x64.dmg` |
| Windows 10/11 (64-bit) | `url2mp3_<version>_x64-setup.exe` |
| Android 11+ (most phones) | `url2mp3_<version>_android-arm64.apk` |
| Android 11+, older 32-bit phones | `url2mp3_<version>_android-armv7.apk` |

There is no iPhone version: iOS doesn't let apps run yt-dlp.

The apps aren't code-signed, so the first launch needs one extra step:

- **macOS**: drag url2mp3 into Applications and open it. If macOS says it "is damaged" or "can't
  be opened", go to System Settings → Privacy & Security → *Open Anyway*, or run
  `xattr -cr /Applications/url2mp3.app` once in Terminal.
- **Windows**: if SmartScreen says "Windows protected your PC", click *More info* → *Run anyway*.
- **Android**: open the APK and allow installing apps from your browser or file manager when
  asked.

## App

Paste links or type a song name and press Add to tray (⌘/Ctrl + Enter). One link opens a trim
screen: listen (the preview skips removed parts), remove any parts, then save the whole track or
the trimmed one. Several links or a playlist wait in the tray; "● Download N tracks" saves them
all, each with its cuts (it waits until every link added has been read). Nothing is written to
the output folder before you press Download. Format (MP3 / M4A / Opus), output folder, bitrate, tracks at once, playlist limit, cover art and
browser cookies are in Settings. The interface is in Vietnamese or English, following the system
unless you pick one.

- The trim screen shows the waveform; select a part and remove it, nudge the edges by ±1 s, undo.
  For YouTube, parts [SponsorBlock](https://sponsor.ajay.app) marks as non-music (intros,
  sponsors, outros…) are suggested, with "Remove all" or "No". To ask, the app sends only the
  first 4 characters of the SHA-256 of the video id, never the id or the link; if the service
  can't be reached there are simply no suggestions.
- Trimmed copies are saved next to the full track as `<title> (trimmed).<ext>`, then
  `(trimmed 2)`…; nothing is ever overwritten. While a track sits in the tray, the app keeps its
  downloaded audio in a temporary folder (so changing the format or trimming later doesn't
  download it again) until you remove the row or quit.
- yt-dlp is not bundled: on first launch the app downloads the latest release from
  [yt-dlp's GitHub](https://github.com/yt-dlp/yt-dlp/releases) (~50 MB), checks it against the
  release's SHA2-256SUMS, and updates it once a day. It lives in the app's data folder
  (macOS `~/Library/Application Support/dev.lecaotanloc.url2mp3/yt-dlp`, Windows
  `%APPDATA%\dev.lecaotanloc.url2mp3\yt-dlp`). Settings shows its version and an Update button.
- ffmpeg and QuickJS (yt-dlp's JavaScript runtime for YouTube) ship inside the app.
- macOS: the first download into `~/Downloads` asks for permission to use that folder. Closing
  the window keeps downloads running; click the Dock icon to bring it back, ⌘Q to quit.
- Android: everything runs on the phone (no server). Python, yt-dlp, ffmpeg and QuickJS come
  from [youtubedl-android](https://github.com/yausername/youtubedl-android), and Settings →
  Update fetches the latest yt-dlp. Tracks go to `Download/url2mp3`, where music players find
  them. In YouTube, TikTok or a browser, *Share → url2mp3* puts the link in the tray and opens its trim screen, and
  a notification keeps downloads going when you switch to another app.

Build it yourself (needs Rust, Node.js and pnpm):

```bash
cd app
pnpm install
pnpm sidecars                       # pinned ffmpeg + QuickJS for this machine, checksum-verified
pnpm tauri dev                      # run it
pnpm tauri build --bundles app,dmg  # macOS; on Windows: --bundles nsis,msi
```

Android: `pnpm tauri android build --apk --split-per-abi --target aarch64 --target armv7`
(needs the Android SDK + NDK, rustup's Android targets and, for a signed release, a key described
by `~/.url2mp3/keystore.properties` with `storeFile`, `password`, `keyAlias`). The Android project
is in `app/src-tauri/gen/android`; its Kotlin side is `ToolsPlugin.kt` and `DownloadService.kt`.

`app/scripts/release-local.sh` builds every release file (both dmgs, the Windows installer
cross-compiled with cargo-xwin, both APKs, CLI archives) into `release/<version>/`.

`pnpm test` runs the UI tests; `pnpm dev` then <http://localhost:1420/dev/mock.html> shows the
UI in a browser with a fake backend.

## Command line

The command's own messages are in Vietnamese.

### Install

Requires [yt-dlp](https://github.com/yt-dlp/yt-dlp) (with [deno](https://deno.com) to solve
YouTube's signature challenge) and [ffmpeg](https://ffmpeg.org) on your `PATH`.

macOS:

```bash
brew install rust yt-dlp ffmpeg   # the yt-dlp formula pulls in deno
cargo install --git https://github.com/lecaotanloc289/url2mp3 url2mp3
```

Linux / Windows: install `yt-dlp`, `ffmpeg` and `deno` with your package manager, then run the
same `cargo install` line.

From a clone: `cargo install --path crates/cli`

### Usage

```bash
url2mp3 https://www.youtube.com/watch?v=Llw9Q6akRo4      # one track → ~/Downloads
url2mp3 "never gonna give you up"                        # search YouTube, take the first hit
url2mp3 https://www.tiktok.com/@user/video/123 link2 …   # several links at once
url2mp3 'https://www.youtube.com/playlist?list=…' -n 20  # first 20 tracks of a playlist
url2mp3 -a links.txt                                     # one link per line
url2mp3                                                  # paste links, empty line to start
```

| Option | Meaning |
|---|---|
| `-f mp3\|m4a\|opus` | Output format. Default `mp3` |
| `-b 320` | Bitrate (kbps) when re-encoding. Default: MP3 VBR V0 (~245 kbps), M4A 256, Opus 160 |
| `-o DIR` | Output folder. Default `~/Downloads`. Playlists go into a subfolder named after the playlist |
| `-j N` | Tracks processed at once. Default: number of CPU cores (max 8) |
| `-c N` | Parallel connections per track. Default 8 |
| `-p` | For a `watch?v=…&list=…` link, download the whole playlist instead of the one track |
| `-n N` | Take only the first N tracks of each playlist |
| `--no-cover` | Don't embed cover art |
| `--force` | Download again even if the file exists. By default an existing file with the same name is skipped, and a video given twice is downloaded once |
| `--cookies-from-browser chrome` | Use your browser's login: age-restricted videos, Premium's 256 kbps streams |

## How it works

```
link ─▶ yt-dlp -J ─▶ best audio stream URL ─▶ 8 parallel connections ─▶ ffmpeg ─▶ .mp3 + tags + cover
        (solves signatures,                   (HTTP Range, each chunk        (stream copy when
         picks the stream)                     written at its offset)         codecs match)
```

The whole pipeline lives in the `crates/core` library (`url2mp3-core`), which reports progress as
events. The `url2mp3` command (`crates/cli`) draws those events in the terminal; the desktop app
(`app/src-tauri`, Tauri 2, UI in `app/src`) forwards them to its window.

- **Resolve** (`crates/core/src/resolve.rs`): runs `yt-dlp -J` only to get JSON. YouTube changes its
  signature scheme every few weeks; yt-dlp keeps up, so fixing breakage is just
  `brew upgrade yt-dlp`.
- **Download** (`crates/core/src/download.rs`): splits the file into byte ranges and fetches them over N
  HTTP/1.1 connections at once. Each range is at most 8 MB (googlevideo throttles requests above
  ~10 MB) and resumes after a dropped connection. Every response's `Content-Range` is checked
  (right offset, right total size) so bad data is never written. If the CDN refuses (403),
  returns the wrong slice, or the stream is HLS/DASH, it falls back to yt-dlp's own downloader.
- **Convert** (`crates/core/src/transcode.rs`): one ffmpeg pass encodes, embeds a square-cropped cover and
  writes the tags. When the source codec already matches the target (Opus→`.opus`, AAC→`.m4a`,
  TikTok MP3→`.mp3`), the stream is copied without re-encoding.
- **Trim** (used by the app; the CLI always saves whole tracks): `cut.rs` normalizes the kept
  ranges and builds the ffmpeg graph that joins them (15 ms fades on inner edges only; a last
  range that reaches the end is open-ended, so no tail is lost to yt-dlp's rounded duration),
  `preview.rs` makes the AAC preview copy, the waveform peaks and the audio's measured length,
  `sponsor.rs` asks SponsorBlock for suggestions. `Engine::fetch` downloads the audio once; `Engine::save` encodes from it.

## Quality and speed

- Free YouTube offers only Opus at ~130–160 kbps or AAC at 128 kbps. **Encoding that to 320 kbps
  MP3 doesn't make it sound better**, it only makes the file bigger. V0 is already transparent.
- For the exact original quality and the fastest result, use `-f opus` (YouTube) or `-f m4a`
  (plays everywhere, including Apple Music). Nothing is re-encoded, so a track takes ~0.5 s after
  it is resolved.
- For MP3, most of the time goes to the LAME encoder (~2.5 s for a 4-minute track,
  single-threaded), which is why `-j` defaults to the number of CPU cores.

## Troubleshooting

| Error | Fix |
|---|---|
| `zsh: parse error near '&'` | Quote the link: `url2mp3 '…watch?v=…&list=…'`, or run `url2mp3` and paste it |
| `Sign in to confirm you're not a bot` | Lower `-j`, or add `--cookies-from-browser chrome` |
| `dns error` / `Failed to resolve '…googlevideo.com'` | Your network or DNS dropped out for a moment (flaky Wi-Fi, VPN/WARP). Run the same command again: finished tracks are skipped |
| Other YouTube errors | `brew upgrade yt-dlp` (YouTube changes its API often) |
| `tải trực tiếp lỗi (HTTP 403)` ("direct download failed") | Only a warning: the track was downloaded again through yt-dlp |

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets
cargo build --release   # target/release/url2mp3
```

Only download content you have the right to: personal use, your own uploads, Creative Commons.
Downloading from YouTube or TikTok is against those platforms' terms of service.

## License

[MIT](LICENSE). The app bundles ffmpeg (GPL) and QuickJS (MIT) and downloads yt-dlp
(Unlicense): see [THIRD_PARTY.md](THIRD_PARTY.md).
