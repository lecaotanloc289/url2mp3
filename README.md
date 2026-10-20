# url2mp3

**English** · [Tiếng Việt](README.vi.md)

Download music from YouTube, TikTok and the ~1,800 other sites yt-dlp supports (SoundCloud,
Facebook…) as MP3, M4A or Opus, with cover art and tags. It is a terminal command: no ads, no
queue, several tracks at once and several connections per track.

The program's own messages are in Vietnamese.

## Install

Requires [yt-dlp](https://github.com/yt-dlp/yt-dlp) (with [deno](https://deno.com) to solve
YouTube's signature challenge) and [ffmpeg](https://ffmpeg.org) on your `PATH`.

macOS:

```bash
brew install rust yt-dlp ffmpeg   # the yt-dlp formula pulls in deno
cargo install --git https://github.com/lecaotanloc289/url2mp3
```

Linux / Windows: install `yt-dlp`, `ffmpeg` and `deno` with your package manager, then run the
same `cargo install` line.

From a clone: `cargo install --path .`

## Usage

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

- **Resolve** (`src/resolve.rs`): runs `yt-dlp -J` only to get JSON. YouTube changes its
  signature scheme every few weeks; yt-dlp keeps up, so fixing breakage is just
  `brew upgrade yt-dlp`.
- **Download** (`src/download.rs`): splits the file into byte ranges and fetches them over N
  HTTP/1.1 connections at once. Each range is at most 8 MB (googlevideo throttles requests above
  ~10 MB) and resumes after a dropped connection. Every response's `Content-Range` is checked
  (right offset, right total size) so bad data is never written. If the CDN refuses (403),
  returns the wrong slice, or the stream is HLS/DASH, it falls back to yt-dlp's own downloader.
- **Convert** (`src/transcode.rs`): one ffmpeg pass encodes, embeds a square-cropped cover and
  writes the tags. When the source codec already matches the target (Opus→`.opus`, AAC→`.m4a`,
  TikTok MP3→`.mp3`), the stream is copied without re-encoding.

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
cargo test
cargo clippy --release
cargo build --release   # target/release/url2mp3
```

Only download content you have the right to: personal use, your own uploads, Creative Commons.
Downloading from YouTube or TikTok is against those platforms' terms of service.

## License

[MIT](LICENSE)
