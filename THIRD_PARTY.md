# Third-party software

url2mp3 itself is MIT-licensed. The desktop app ships or downloads these programs, which run
as separate executables under their own licences.

| Program | Version | Licence | Where it comes from |
|---|---|---|---|
| [FFmpeg](https://ffmpeg.org) | 9.0.2 | GPL v3 (built with libmp3lame, libopus and other GPL parts) | macOS: static builds by Martin Riedl, <https://ffmpeg.martin-riedl.de>. Windows: "essentials" build by Gyan Doshi, <https://github.com/GyanD/codexffmpeg/releases/tag/9.0.2>. Source: <https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz> |
| [QuickJS-ng](https://github.com/quickjs-ng/quickjs) | 0.17.0 | MIT | <https://github.com/quickjs-ng/quickjs/releases/tag/v0.17.0> |
| [yt-dlp](https://github.com/yt-dlp/yt-dlp) | latest, downloaded at runtime | Unlicense (its bundled Python and libraries keep their own licences) | <https://github.com/yt-dlp/yt-dlp/releases> |
| [youtubedl-android](https://github.com/yausername/youtubedl-android) (Android app) | 0.18.1 | GPL v3; bundles Python, yt-dlp, FFmpeg and QuickJS under their own licences | Maven Central `io.github.junkfood02.youtubedl-android` |
| [Tauri](https://tauri.app) | 2.12 | MIT or Apache-2.0 | crates.io / npm |
| [Bricolage Grotesque](https://fontsource.org/fonts/bricolage-grotesque) | 5.3.0 (fontsource) | SIL Open Font License 1.1 | npm `@fontsource-variable/bricolage-grotesque` |
| [Be Vietnam Pro](https://fontsource.org/fonts/be-vietnam-pro) | 5.3.0 (fontsource) | SIL Open Font License 1.1 | npm `@fontsource/be-vietnam-pro` |

The Android APK links youtubedl-android (GPL v3), so the APK as a whole is distributed under
GPL v3; its complete source is this repository (url2mp3's own code stays MIT, which GPL v3
allows).

The exact download URLs and SHA-256 checksums of the bundled FFmpeg and QuickJS builds are in
[`app/sidecars.json`](app/sidecars.json).
