# Plan 1 — Core library + workspace split

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split url2mp3 into a Cargo workspace with a reusable `url2mp3-core` library (explicit tool paths, typed progress events) and the unchanged `url2mp3` CLI on top, so the Tauri apps (later plans) can drive the same engine.

**Architecture:** `crates/core` owns resolve/download/transcode plus a new `Engine` that runs the per-link pipeline and reports `Event`s through a callback instead of drawing indicatif bars. `Tools` holds the yt-dlp/ffmpeg paths, an optional `--js-runtimes` value and extra env. `crates/cli` keeps clap + indicatif and renders events. Behaviour and messages of the CLI stay the same.

**Tech Stack:** Rust 2024, tokio, reqwest (rustls, HTTP/1.1), clap (optional feature in core), indicatif (CLI only).

Spec: `docs/superpowers/specs/2026-10-23-cross-platform-apps-design.md` (section 1, "core" and "cli").

Later plans: 2 desktop app, 3 release CI, 4 Android spike, 5 Android app, 6 Homebrew + docs + v0.2.0.

---

## File map

| Path | Role |
|---|---|
| `Cargo.toml` | Workspace root: members, shared package fields, shared deps, release profile |
| `crates/cli/Cargo.toml` | Package `url2mp3` (binary name unchanged) |
| `crates/cli/src/main.rs` | clap args, input collection, run loop, summary |
| `crates/cli/src/render.rs` | Event → indicatif bars and lines |
| `crates/core/Cargo.toml` | Package `url2mp3-core`, feature `clap` |
| `crates/core/src/lib.rs` | Public API re-exports, `default_out_dir` |
| `crates/core/src/tools.rs` | `Tools`: program paths, js runtime, env, version checks |
| `crates/core/src/names.rs` | `sanitize`, `path_key`, `is_network_error` (moved from main.rs) |
| `crates/core/src/engine.rs` | `Engine`, `Options`, `Event`, `Skip`, `Outcome`, `Id`; pipeline moved from main.rs |
| `crates/core/src/resolve.rs` | moved; uses `Tools` |
| `crates/core/src/download.rs` | moved; `Progress` trait instead of `indicatif::ProgressBar` |
| `crates/core/src/transcode.rs` | moved; uses `Tools`; clap derive behind feature |
| `crates/core/tests/engine.rs` | Engine tests with fake yt-dlp/ffmpeg shell scripts (unix) |

---

### Task 1: Workspace with the CLI as its only member

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/cli/Cargo.toml`
- Move: `src/` → `crates/cli/src/`

- [ ] **Step 1: Move sources**

```bash
mkdir -p crates/cli && git mv src crates/cli/src
```

- [ ] **Step 2: Root `Cargo.toml` becomes the workspace**

```toml
[workspace]
resolver = "3"
members = ["crates/cli"]

[workspace.package]
version = "0.1.0"
edition = "2024"
license = "MIT"
repository = "https://github.com/lecaotanloc289/url2mp3"

[workspace.dependencies]
anyhow = "1.0.104"
clap = { version = "4.6.7", features = ["derive"] }
dirs = "7.0.0"
futures-util = "0.3.34"
indicatif = "0.18.6"
# HTTP/1.1 only on purpose: N real TCP connections beat N streams multiplexed on one h2 connection.
reqwest = { version = "0.13.5", default-features = false, features = ["rustls"] }
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.154"
tempfile = "3.27.0"
tokio = { version = "1.53.3", features = ["rt-multi-thread", "macros", "process", "fs", "io-util", "sync", "time", "signal"] }

[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
strip = true
```

- [ ] **Step 3: `crates/cli/Cargo.toml`**

```toml
[package]
name = "url2mp3"
description = "Tải nhạc YouTube/TikTok sang MP3/M4A/Opus: tải song song nhiều kết nối, không quảng cáo"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
anyhow.workspace = true
clap.workspace = true
dirs.workspace = true
futures-util.workspace = true
indicatif.workspace = true
reqwest.workspace = true
serde.workspace = true
serde_json.workspace = true
tempfile.workspace = true
tokio.workspace = true
```

- [ ] **Step 4: Verify nothing changed**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: `7 passed`, no clippy warnings. `target/debug/url2mp3 --version` prints `url2mp3 0.1.0`.

- [ ] **Step 5: Commit**

```bash
git add -A Cargo.toml Cargo.lock crates src
git commit -m "build: move the CLI into a Cargo workspace"
```

---

### Task 2: `url2mp3-core` with Tools, Engine and events; CLI renders events

One commit: moving the modules out of the CLI and switching the CLI to the engine cannot be
split without a broken intermediate tree.

**Files:**
- Create: `crates/core/Cargo.toml`, `crates/core/src/lib.rs`, `crates/core/src/tools.rs`,
  `crates/core/src/names.rs`, `crates/core/src/engine.rs`, `crates/core/tests/engine.rs`,
  `crates/cli/src/render.rs`
- Move + modify: `crates/cli/src/{resolve,download,transcode}.rs` → `crates/core/src/`
- Rewrite: `crates/cli/src/main.rs`, `crates/cli/Cargo.toml`
- Modify: `Cargo.toml` (members)

- [ ] **Step 1: Write the failing engine tests** — `crates/core/tests/engine.rs`

```rust
//! Engine runs against fake yt-dlp/ffmpeg shell scripts: no network, no real media.
#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use url2mp3_core::{Engine, Event, Format, Options, Outcome, Skip, Tools};

/// `-J` prints a track (or a playlist for links containing "playlist"); otherwise writes
/// "audio" to the `-o` path. Links containing "bad" fail like a private video.
const FAKE_YTDLP: &str = r#"#!/bin/sh
[ -n "$FAKE_LOG" ] && echo "$@" >> "$FAKE_LOG"
json=0; out=""; prev=""
for a in "$@"; do
  case "$a" in --version) echo 2099.01.01; exit 0;; -J) json=1;; esac
  [ "$prev" = "-o" ] && out="$a"
  prev="$a"
done
case "$prev" in
  *bad*) echo "ERROR: Video unavailable" >&2; exit 1;;
  *playlist*) echo '{"_type":"playlist","title":"Mix","entries":[{"_type":"url","url":"https://example.com/v/abc","title":"Fake Song"}]}'; exit 0;;
esac
if [ "$json" = 1 ]; then
  echo '{"id":"abc","title":"Fake Song","uploader":"Tester","webpage_url":"https://example.com/v/abc","acodec":"mp3"}'
else
  printf 'audio' > "$out"
fi
"#;

/// Copies the first `-i` input to the last argument. With `FAIL_ONCE=<file>` the first call
/// fails and creates that file.
const FAKE_FFMPEG: &str = r#"#!/bin/sh
[ "$1" = "-version" ] && exit 0
if [ -n "$FAIL_ONCE" ] && [ ! -e "$FAIL_ONCE" ]; then touch "$FAIL_ONCE"; echo "boom" >&2; exit 1; fi
in=""; prev=""; last=""
for a in "$@"; do
  [ "$prev" = "-i" ] && [ -z "$in" ] && in="$a"
  prev="$a"; last="$a"
done
cp "$in" "$last"
"#;

const LINK: &str = "https://example.com/v/abc";

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn fakes(dir: &Path) -> Tools {
    Tools {
        ytdlp: script(dir, "yt-dlp", FAKE_YTDLP),
        ffmpeg: script(dir, "ffmpeg", FAKE_FFMPEG),
        js_runtime: None,
        env: Vec::new(),
    }
}

fn options(out: &Path) -> Options {
    Options {
        format: Format::Mp3,
        bitrate: None,
        out: out.to_owned(),
        jobs: 2,
        connections: 2,
        playlist: false,
        limit: None,
        cover: true,
        force: false,
        cookies_from_browser: None,
    }
}

/// Engine with a sink that records every event.
async fn engine(tools: Tools, out: &Path) -> (Arc<Engine>, Arc<Mutex<Vec<Event>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = {
        let events = events.clone();
        move |e: Event| events.lock().unwrap().push(e)
    };
    (Engine::start(tools, options(out), sink).await.unwrap(), events)
}

async fn process(engine: &Arc<Engine>, input: &str) -> Vec<Outcome> {
    engine.clone().process(engine.next_id(), input.to_owned()).await
}

fn kind(e: &Event) -> &'static str {
    match e {
        Event::Probing { .. } => "probing",
        Event::Playlist { .. } => "playlist",
        Event::Track { .. } => "track",
        Event::Progress { .. } => "progress",
        Event::Fallback { .. } => "fallback",
        Event::Converting { .. } => "converting",
        Event::Done { .. } => "done",
        Event::Skipped { .. } => "skipped",
        Event::Failed { .. } => "failed",
    }
}

fn kinds(events: &Mutex<Vec<Event>>) -> Vec<&'static str> {
    events.lock().unwrap().iter().map(kind).collect()
}

#[tokio::test]
async fn tools_report_versions() {
    let bin = tempfile::tempdir().unwrap();
    let tools = fakes(bin.path());
    assert_eq!(tools.ytdlp_version().await.unwrap(), "2099.01.01");
    tools.check_ffmpeg().await.unwrap();
    let missing = Tools {
        ytdlp: bin.path().join("nope"),
        ..tools
    };
    assert!(missing.ytdlp_version().await.is_err());
}

#[tokio::test]
async fn downloads_converts_and_reports_each_step() {
    let (bin, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (engine, events) = engine(fakes(bin.path()), out.path()).await;

    assert_eq!(process(&engine, LINK).await, vec![Outcome::Done(5)]);

    let dest = out.path().join("Fake Song.mp3");
    assert_eq!(fs::read(&dest).unwrap(), b"audio");
    assert_eq!(kinds(&events), ["probing", "track", "fallback", "converting", "done"]);
    let events = events.lock().unwrap();
    assert!(matches!(&events[1],
        Event::Track { id: 0, parent: None, title, artist: Some(a), .. } if title == "Fake Song" && a == "Tester"));
    assert!(matches!(&events[2], Event::Fallback { id: 0, reason: None }));
    assert!(matches!(&events[4], Event::Done { id: 0, path, bytes: 5, .. } if *path == dest));
}

#[tokio::test]
async fn existing_file_is_skipped() {
    let (bin, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    fs::write(out.path().join("Fake Song.mp3"), b"old").unwrap();
    let (engine, events) = engine(fakes(bin.path()), out.path()).await;

    assert_eq!(process(&engine, LINK).await, vec![Outcome::Skipped]);

    let last = events.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        last,
        Event::Skipped {
            id: 0,
            parent: None,
            title: "Fake Song".into(),
            reason: Skip::Exists(out.path().join("Fake Song.mp3")),
        }
    );
}

#[tokio::test]
async fn same_video_twice_in_a_session_is_a_duplicate() {
    let (bin, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (engine, events) = engine(fakes(bin.path()), out.path()).await;

    process(&engine, LINK).await;
    assert_eq!(process(&engine, LINK).await, vec![Outcome::Skipped]);

    let last = events.lock().unwrap().last().cloned().unwrap();
    assert!(matches!(last, Event::Skipped { id: 1, reason: Skip::Duplicate, .. }), "{last:?}");
}

#[tokio::test]
async fn failure_carries_the_ytdlp_error_line() {
    let (bin, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (engine, events) = engine(fakes(bin.path()), out.path()).await;
    let link = "https://example.com/bad";

    assert_eq!(process(&engine, link).await, vec![Outcome::Failed { network: false }]);

    let last = events.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        last,
        Event::Failed {
            id: 0,
            parent: None,
            label: link.into(),
            message: "Video unavailable".into(),
            network: false,
        }
    );
}

#[tokio::test]
async fn failed_track_can_be_retried_in_the_same_session() {
    let (bin, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut tools = fakes(bin.path());
    tools.env.push(("FAIL_ONCE".into(), bin.path().join("failed").into()));
    let (engine, _) = engine(tools, out.path()).await;

    assert_eq!(process(&engine, LINK).await, vec![Outcome::Failed { network: false }]);
    assert_eq!(process(&engine, LINK).await, vec![Outcome::Done(5)]);
}

#[tokio::test]
async fn playlist_tracks_go_to_a_subfolder_with_their_parent() {
    let (bin, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (engine, events) = engine(fakes(bin.path()), out.path()).await;

    let outcomes = process(&engine, "https://example.com/playlist?list=1").await;

    assert_eq!(outcomes, vec![Outcome::Done(5)]);
    assert!(out.path().join("Mix/Fake Song.mp3").exists());
    assert_eq!(
        kinds(&events),
        ["probing", "playlist", "probing", "track", "fallback", "converting", "done"]
    );
    let events = events.lock().unwrap();
    assert!(matches!(&events[1], Event::Playlist { id: 0, title, count: 1 } if title == "Mix"));
    assert!(matches!(&events[3], Event::Track { id: 1, parent: Some(0), .. }));
}

#[tokio::test]
async fn passes_js_runtime_and_env_to_ytdlp() {
    let (bin, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let log = bin.path().join("log");
    let mut tools = fakes(bin.path());
    tools.js_runtime = Some("deno:/opt/deno".into());
    tools.env.push(("FAKE_LOG".into(), log.clone().into()));
    let (engine, _) = engine(tools, out.path()).await;

    process(&engine, LINK).await;

    let log = fs::read_to_string(log).unwrap();
    assert_eq!(log.lines().count(), 2, "probe + download: {log}");
    assert!(log.lines().all(|l| l.contains("--js-runtimes deno:/opt/deno")), "{log}");
}
```

- [ ] **Step 2: Core crate manifest and workspace member**

Root `Cargo.toml`: `members = ["crates/core", "crates/cli"]`.

`crates/core/Cargo.toml`:

```toml
[package]
name = "url2mp3-core"
description = "Download engine behind url2mp3: yt-dlp to resolve, parallel HTTP range download, ffmpeg to convert"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true

[features]
# clap::ValueEnum for Format, used by the CLI.
clap = ["dep:clap"]

[dependencies]
anyhow.workspace = true
clap = { workspace = true, optional = true }
dirs.workspace = true
futures-util.workspace = true
reqwest.workspace = true
serde.workspace = true
serde_json.workspace = true
tempfile.workspace = true
tokio.workspace = true
```

`crates/cli/Cargo.toml` `[dependencies]` becomes:

```toml
[dependencies]
url2mp3-core = { path = "../core", features = ["clap"] }
anyhow.workspace = true
clap.workspace = true
indicatif.workspace = true
tokio.workspace = true
```

- [ ] **Step 3: Move the modules**

```bash
mkdir -p crates/core/src crates/core/tests
git mv crates/cli/src/resolve.rs crates/cli/src/download.rs crates/cli/src/transcode.rs crates/core/src/
```

- [ ] **Step 4: `crates/core/src/lib.rs`**

```rust
//! url2mp3's engine: yt-dlp resolves a link, our own downloader fetches the audio over parallel
//! connections, ffmpeg converts and tags it. Used by the `url2mp3` CLI and the apps.

mod download;
mod engine;
mod names;
mod resolve;
mod tools;
mod transcode;

use std::path::PathBuf;

pub use engine::{Engine, Event, Id, Options, Outcome, Skip};
pub use tools::Tools;
pub use transcode::Format;

/// `~/Downloads`, the default output folder.
pub fn default_out_dir() -> Option<PathBuf> {
    dirs::download_dir().or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
}
```

- [ ] **Step 5: `crates/core/src/tools.rs`**

```rust
//! Where the external programs live. The CLI finds them on PATH; the apps ship their own copies.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{Result, bail};
use tokio::process::Command;

#[derive(Clone, Debug)]
pub struct Tools {
    pub ytdlp: PathBuf,
    pub ffmpeg: PathBuf,
    /// Passed to yt-dlp as `--js-runtimes` (e.g. `deno:/path/to/deno`) for YouTube's signature
    /// challenge. None: yt-dlp's default, deno on PATH.
    pub js_runtime: Option<String>,
    /// Extra environment for both programs (e.g. PYTHONHOME on Android).
    pub env: Vec<(OsString, OsString)>,
}

impl Tools {
    pub fn from_path() -> Self {
        Self {
            ytdlp: "yt-dlp".into(),
            ffmpeg: "ffmpeg".into(),
            js_runtime: None,
            env: Vec::new(),
        }
    }

    pub async fn ytdlp_version(&self) -> Result<String> {
        let out = self.ytdlp_command().arg("--version").output().await?;
        if !out.status.success() {
            bail!("yt-dlp --version failed");
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    pub async fn check_ffmpeg(&self) -> Result<()> {
        let ok = self
            .ffmpeg_command()
            .arg("-version")
            .stdout(Stdio::null())
            .status()
            .await?
            .success();
        if !ok {
            bail!("ffmpeg -version failed");
        }
        Ok(())
    }

    pub(crate) fn ytdlp_command(&self) -> Command {
        self.command(&self.ytdlp)
    }

    pub(crate) fn ffmpeg_command(&self) -> Command {
        self.command(&self.ffmpeg)
    }

    fn command(&self, program: &Path) -> Command {
        let mut cmd = Command::new(program);
        cmd.envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .kill_on_drop(true);
        cmd
    }
}
```

- [ ] **Step 6: `crates/core/src/names.rs`** — moved verbatim from `crates/cli/src/main.rs`
  (`is_network_error`, `path_key`, `sanitize` and their two tests), made `pub(crate)`:

```rust
//! File names and error classification shared by the pipeline.

use std::path::Path;

/// DNS/connection trouble on the user's side, not a YouTube change.
pub(crate) fn is_network_error(msg: &str) -> bool {
    const SIGNS: [&str; 7] = [
        "dns error",
        "failed to resolve",
        "nodename nor servname",
        "network is unreachable",
        "connection refused",
        "connection reset",
        "timed out",
    ];
    let msg = msg.to_lowercase();
    SIGNS.iter().any(|s| msg.contains(s))
}

/// Case-insensitive: APFS and NTFS treat "Track.mp3" and "TRACK.mp3" as one file.
pub(crate) fn path_key(path: &Path) -> String {
    format!("path:{}", path.to_string_lossy().to_lowercase())
}

/// Safe file name on macOS/Windows/Linux, ≤ 180 bytes.
pub(crate) fn sanitize(name: &str) -> String {
    let mapped: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | '*' | '?' | '"' | '<' | '>' => '_',
            ':' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let squeezed = mapped.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = squeezed.trim_matches(|c: char| c == '.' || c.is_whitespace());
    let mut out = String::new();
    for c in trimmed.chars() {
        if out.len() + c.len_utf8() > 180 {
            break;
        }
        out.push(c);
    }
    let out = out.trim_end().to_owned();
    if out.is_empty() { "untitled".into() } else { out }
}

#[cfg(test)]
mod tests {
    use super::{is_network_error, sanitize};

    #[test]
    fn classifies_network_errors() {
        // Real messages from a run where the local DNS stopped resolving googlevideo hosts.
        assert!(is_network_error(
            "[download] Got error: HTTPSConnection(host='rr4---sn-oji3bc-5j.googlevideo.com', port=443): \
             Failed to resolve 'rr4---sn-oji3bc-5j.googlevideo.com' ([Errno 8] nodename nor servname \
             provided, or not known). Giving up after 10 retries"
        ));
        assert!(is_network_error(
            "error sending request: client error (Connect): dns error: failed to lookup address information"
        ));
        assert!(!is_network_error("Video unavailable. This video is private"));
        assert!(!is_network_error("Sign in to confirm you're not a bot"));
    }

    #[test]
    fn sanitize_names() {
        assert_eq!(
            sanitize("LẠC TRÔI | OFFICIAL MUSIC VIDEO | SƠN TÙNG M-TP"),
            "LẠC TRÔI - OFFICIAL MUSIC VIDEO - SƠN TÙNG M-TP"
        );
        assert_eq!(sanitize("a/b:c?  \n d"), "a_b-c_ d");
        assert_eq!(sanitize("...hidden"), "hidden");
        assert_eq!(sanitize("   "), "untitled");
        assert!(sanitize(&"ạ".repeat(200)).len() <= 180);
    }
}
```

- [ ] **Step 7: `resolve.rs` uses `Tools`**

Imports: drop `bail`'s sibling users only where unused; add `use crate::tools::Tools;`.
Replace the `Ytdlp` struct, `new`, `version` and `command` with:

```rust
pub struct Ytdlp {
    tools: Tools,
    /// Flags shared by every call: format selector, playlist policy, cookies, JS runtime.
    base: Vec<String>,
}

impl Ytdlp {
    pub fn new(tools: Tools, selector: &str, playlist: bool, limit: Option<usize>, browser: Option<&str>) -> Self {
        let mut base = vec!["--no-warnings".into(), "-f".into(), selector.into()];
        if !playlist {
            base.push("--no-playlist".into());
        }
        if let Some(n) = limit {
            base.extend(["--playlist-end".into(), n.to_string()]);
        }
        if let Some(b) = browser {
            base.extend(["--cookies-from-browser".into(), b.into()]);
        }
        if let Some(js) = &tools.js_runtime {
            base.extend(["--js-runtimes".into(), js.clone()]);
        }
        Self { tools, base }
    }
```

(`probe` and `download` unchanged; `version` is deleted — it moved to `Tools::ytdlp_version`.)

```rust
    fn command(&self) -> Command {
        let mut cmd = self.tools.ytdlp_command();
        cmd.args(&self.base).stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd
    }
```

- [ ] **Step 8: `download.rs` reports through a `Progress` trait**

Remove `use indicatif::ProgressBar;`. Add after the constants:

```rust
/// Byte progress of one download.
pub trait Progress: Sync {
    fn set_length(&self, total: u64);
    fn inc(&self, n: u64);
}
```

Replace every `ProgressBar` type with `dyn Progress`: `Target::pb: &'a dyn Progress`, and the
`pb` parameters of `fetch` and `probe` become `pb: &dyn Progress`. Method calls (`set_length`,
`inc`) stay the same. In the test module replace the indicatif import and `&ProgressBar::hidden()`:

```rust
    use super::{Downloader, Progress, parse_content_range};
    use reqwest::header::HeaderMap;

    struct NoProgress;
    impl Progress for NoProgress {
        fn set_length(&self, _: u64) {}
        fn inc(&self, _: u64) {}
    }
```

and pass `&NoProgress` to `fetch`.

- [ ] **Step 9: `transcode.rs` uses `Tools`; clap derive optional**

```rust
use std::{path::Path, process::Stdio};

use anyhow::{Context, Result, bail};

use crate::tools::Tools;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum Format {
```

Delete `pub async fn version()` (now `Tools::check_ffmpeg`). `encode` takes the tools:

```rust
pub async fn encode(tools: &Tools, job: Job<'_>) -> Result<()> {
    let mut cmd = tools.ffmpeg_command();
```

(rest of `encode` unchanged).

- [ ] **Step 10: `crates/core/src/engine.rs`** — the pipeline from `main.rs`, reporting events

```rust
//! The per-link pipeline: probe with yt-dlp, expand playlists, skip what exists, download,
//! convert, move into place. It reports what happens as `Event`s and draws nothing itself.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use tempfile::TempDir;
use tokio::{sync::Semaphore, task::JoinSet};

use crate::{
    download::{Downloader, Progress},
    names::{is_network_error, path_key, sanitize},
    resolve::{Info, Ytdlp},
    tools::Tools,
    transcode::{self, Format, Tags},
};

/// Identifies one link, playlist or track across its events.
pub type Id = u64;

const PROGRESS_EVERY: Duration = Duration::from_millis(100);

pub struct Options {
    pub format: Format,
    /// kbps when re-encoding. None: MP3 VBR V0, M4A 256, Opus 160.
    pub bitrate: Option<u32>,
    pub out: PathBuf,
    /// Tracks processed at once.
    pub jobs: usize,
    /// Parallel connections per track.
    pub connections: usize,
    /// For a `watch?v=…&list=…` link, download the whole playlist.
    pub playlist: bool,
    /// Only the first N tracks of each playlist.
    pub limit: Option<usize>,
    pub cover: bool,
    /// Download again even if the file exists.
    pub force: bool,
    pub cookies_from_browser: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// yt-dlp is reading a link: the user's input, or a playlist entry.
    Probing { id: Id, parent: Option<Id>, label: String },
    /// `id` is a playlist; its tracks follow with `parent: Some(id)`.
    Playlist { id: Id, title: String, count: usize },
    /// The download starts.
    Track {
        id: Id,
        parent: Option<Id>,
        title: String,
        artist: Option<String>,
        thumbnail: Option<String>,
    },
    /// Bytes downloaded so far, at most every 100 ms per track.
    Progress { id: Id, done: u64, total: u64 },
    /// yt-dlp downloads the stream itself. `reason`: why our direct download failed, if tried.
    Fallback { id: Id, reason: Option<String> },
    Converting { id: Id },
    Done { id: Id, path: PathBuf, bytes: u64, secs: f64 },
    Skipped { id: Id, parent: Option<Id>, title: String, reason: Skip },
    Failed {
        id: Id,
        parent: Option<Id>,
        label: String,
        message: String,
        /// DNS/connection trouble on the user's side, not a YouTube change.
        network: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Skip {
    /// The same video, or another one with the same file name, already ran in this session.
    Duplicate,
    /// The file is already there (`force` downloads it again).
    Exists(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done(u64),
    Skipped,
    Failed { network: bool },
}

pub struct Engine {
    ytdlp: Ytdlp,
    dl: Downloader,
    tools: Tools,
    tmp: TempDir,
    sem: Semaphore,
    next: AtomicU64,
    /// Video ids and destinations taken in this session, so two tracks with the same title
    /// don't overwrite each other and one video given twice is fetched once.
    claimed: Mutex<HashSet<String>>,
    sink: Box<dyn Fn(Event) + Send + Sync>,
    opts: Options,
}

impl Engine {
    /// Creates the output folder and a temp folder inside it (removed when the engine is
    /// dropped), after clearing temp folders left behind by a killed run.
    pub async fn start(
        tools: Tools,
        opts: Options,
        sink: impl Fn(Event) + Send + Sync + 'static,
    ) -> Result<Arc<Self>> {
        tokio::fs::create_dir_all(&opts.out)
            .await
            .with_context(|| format!("tạo {}", opts.out.display()))?;
        sweep_stale_temp(&opts.out).await;
        // Inside the output dir so the final rename never crosses filesystems.
        let tmp = tempfile::Builder::new().prefix(".url2mp3-").tempdir_in(&opts.out)?;
        Ok(Arc::new(Self {
            ytdlp: Ytdlp::new(
                tools.clone(),
                opts.format.selector(),
                opts.playlist,
                opts.limit,
                opts.cookies_from_browser.as_deref(),
            ),
            dl: Downloader::new(opts.connections)?,
            tools,
            tmp,
            sem: Semaphore::new(opts.jobs),
            next: AtomicU64::new(0),
            claimed: Mutex::default(),
            sink: Box::new(sink),
            opts,
        }))
    }

    /// A fresh id to pass to `process`.
    pub fn next_id(&self) -> Id {
        self.next.fetch_add(1, Relaxed)
    }

    /// A link, playlist or search query → one or more tracks. Dropping the future (or aborting
    /// its task) kills every yt-dlp/ffmpeg it started.
    pub async fn process(self: Arc<Self>, id: Id, input: String) -> Vec<Outcome> {
        let search = !(input.starts_with("http://") || input.starts_with("https://"));
        let permit = self.sem.acquire().await.expect("semaphore never closed");
        self.emit(Event::Probing {
            id,
            parent: None,
            label: input.clone(),
        });
        // Searches are not flattened: yt-dlp resolves the hit in the same call.
        let target = if search {
            format!("ytsearch1:{input}")
        } else {
            input.clone()
        };
        let info = match self.ytdlp.probe(&target, !search).await {
            Ok(info) => info,
            Err(e) => return vec![self.fail(id, None, &input, e)],
        };
        let out = self.opts.out.clone();
        if !info.is_playlist() {
            return vec![self.track(id, None, &info, &out).await];
        }
        if search {
            // ytsearch1 is a one-entry playlist: the hit keeps the search's id.
            return match info.entries.into_iter().flatten().flatten().next() {
                Some(hit) => vec![self.entry(id, None, hit, &out).await],
                None => vec![self.fail(id, None, &input, anyhow!("không tìm thấy bài nào"))],
            };
        }
        drop(permit);

        let entries: Vec<Info> = info.entries.into_iter().flatten().flatten().collect();
        let name = sanitize(info.title.as_deref().unwrap_or("playlist"));
        self.emit(Event::Playlist {
            id,
            title: name.clone(),
            count: entries.len(),
        });
        let dir = out.join(name);
        let mut set = JoinSet::new();
        for entry in entries {
            let (engine, dir, child) = (self.clone(), dir.clone(), self.next_id());
            set.spawn(async move {
                let _permit = engine.sem.acquire().await.expect("semaphore never closed");
                engine.entry(child, Some(id), entry, &dir).await
            });
        }
        set.join_all().await
    }

    /// A playlist entry or search hit. Flat entries only hold a page URL and are probed first.
    async fn entry(&self, id: Id, parent: Option<Id>, entry: Info, dir: &Path) -> Outcome {
        if !entry.is_reference() {
            return self.track(id, parent, &entry, dir).await;
        }
        let label = entry
            .title
            .as_deref()
            .or(entry.url.as_deref())
            .unwrap_or("?")
            .to_owned();
        let Some(url) = entry.url.as_deref() else {
            return self.fail(id, parent, &label, anyhow!("playlist thiếu link bài"));
        };
        self.emit(Event::Probing {
            id,
            parent,
            label: label.clone(),
        });
        match self.ytdlp.probe(url, true).await {
            Ok(info) if info.is_playlist() => self.fail(
                id,
                parent,
                &label,
                anyhow!("playlist lồng nhau, hãy dán trực tiếp link playlist con"),
            ),
            Ok(info) => self.track(id, parent, &info, dir).await,
            Err(e) => self.fail(id, parent, &label, e),
        }
    }

    async fn track(&self, id: Id, parent: Option<Id>, info: &Info, dir: &Path) -> Outcome {
        let title = info
            .title
            .clone()
            .or_else(|| info.id.clone())
            .unwrap_or_else(|| "untitled".into());
        match self.try_track(id, parent, info, &title, dir).await {
            Ok(o) => o,
            Err(e) => self.fail(id, parent, &title, e),
        }
    }

    async fn try_track(
        &self,
        id: Id,
        parent: Option<Id>,
        info: &Info,
        title: &str,
        dir: &Path,
    ) -> Result<Outcome> {
        let started = Instant::now();
        let name = sanitize(title);
        let ext = self.opts.format.ext();
        let skip = |reason: Skip| -> Result<Outcome> {
            self.emit(Event::Skipped {
                id,
                parent,
                title: title.to_owned(),
                reason,
            });
            Ok(Outcome::Skipped)
        };
        let mut claims = Claims {
            engine: self,
            keys: Vec::new(),
            keep: false,
        };
        // Same video reached twice (plain link + its &list= link, or two playlists).
        if let Some(vid) = info.id.as_deref()
            && !claims.take(format!("id:{vid}"))
        {
            return skip(Skip::Duplicate);
        }
        let mut dest = dir.join(format!("{name}.{ext}"));
        if !claims.take(path_key(&dest)) {
            // Same title twice in one run (e.g. two "original sound" TikToks): add the id.
            let vid = sanitize(info.id.as_deref().unwrap_or("2"));
            dest = dir.join(format!("{name} [{vid}].{ext}"));
            if !claims.take(path_key(&dest)) {
                return skip(Skip::Duplicate);
            }
        }
        if !self.opts.force && tokio::fs::try_exists(&dest).await.unwrap_or(false) {
            return skip(Skip::Exists(dest));
        }
        tokio::fs::create_dir_all(dir).await?;
        self.emit(Event::Track {
            id,
            parent,
            title: title.to_owned(),
            artist: info.display_artist().map(str::to_owned),
            thumbnail: info.thumbnail.clone(),
        });

        let tmp = self.tmp.path();
        let audio = tmp.join(format!("{id}.audio"));
        let cover = tmp.join(format!("{id}.cover"));
        let encoded = tmp.join(format!("{id}.{ext}"));

        let thumb = info
            .thumbnail
            .as_deref()
            .filter(|_| self.opts.cover && self.opts.format.supports_cover());
        let fetch_cover = async {
            match thumb {
                Some(url) => self.dl.fetch_small(url, &info.headers(), &cover).await.is_ok(),
                None => false,
            }
        };
        let (audio_res, has_cover) = tokio::join!(self.fetch_audio(id, info, &audio), fetch_cover);
        audio_res?;

        self.emit(Event::Converting { id });
        let job = |with_cover: bool| transcode::Job {
            input: &audio,
            cover: with_cover.then_some(cover.as_path()),
            output: &encoded,
            format: self.opts.format,
            src_codec: info.acodec.as_deref().unwrap_or(""),
            bitrate: self.opts.bitrate,
            tags: Tags {
                title,
                artist: info.display_artist(),
                album: info.album.as_deref(),
                comment: info.webpage_url.as_deref(),
            },
        };
        let mut encoded_ok = transcode::encode(&self.tools, job(has_cover)).await;
        if encoded_ok.is_err() && has_cover {
            // A thumbnail ffmpeg cannot decode must not cost the track.
            encoded_ok = transcode::encode(&self.tools, job(false)).await;
        }
        encoded_ok?;
        tokio::fs::rename(&encoded, &dest).await?;
        let _ = tokio::join!(tokio::fs::remove_file(&audio), tokio::fs::remove_file(&cover));

        let bytes = tokio::fs::metadata(&dest).await.map(|m| m.len()).unwrap_or(0);
        claims.keep = true;
        self.emit(Event::Done {
            id,
            path: dest,
            bytes,
            secs: started.elapsed().as_secs_f64(),
        });
        Ok(Outcome::Done(bytes))
    }

    /// Fast path: our own parallel range download. If the CDN refuses it, let yt-dlp try.
    async fn fetch_audio(&self, id: Id, info: &Info, path: &Path) -> Result<()> {
        let mut reason = None;
        if let Some(url) = info.direct_url() {
            let progress = Throttled::new(self, id);
            match self
                .dl
                .fetch(url, &info.media_headers(), info.exact_size(), path, &progress)
                .await
            {
                Ok(()) => {
                    progress.send(true);
                    return Ok(());
                }
                Err(e) => reason = Some(format!("{e:#}")),
            }
        }
        self.emit(Event::Fallback { id, reason });
        let page = info
            .webpage_url
            .as_deref()
            .context("không có link gốc để tải lại")?;
        self.ytdlp.download(page, path, self.dl.connections).await
    }

    fn fail(&self, id: Id, parent: Option<Id>, label: &str, err: anyhow::Error) -> Outcome {
        let message = format!("{err:#}");
        let network = is_network_error(&message);
        self.emit(Event::Failed {
            id,
            parent,
            label: label.to_owned(),
            message,
            network,
        });
        Outcome::Failed { network }
    }

    fn emit(&self, event: Event) {
        (self.sink)(event);
    }
}

/// Keys (video id, destination) taken by one track. Given back on drop unless the track
/// finished, so a failed or cancelled track can be retried in the same session.
struct Claims<'a> {
    engine: &'a Engine,
    keys: Vec<String>,
    keep: bool,
}

impl Claims<'_> {
    /// First caller to claim a key in this session wins.
    fn take(&mut self, key: String) -> bool {
        let fresh = self.engine.claimed.lock().expect("claimed lock").insert(key.clone());
        if fresh {
            self.keys.push(key);
        }
        fresh
    }
}

impl Drop for Claims<'_> {
    fn drop(&mut self) {
        if !self.keep {
            let mut claimed = self.engine.claimed.lock().expect("claimed lock");
            for key in &self.keys {
                claimed.remove(key);
            }
        }
    }
}

/// Turns the downloader's per-chunk counts into `Event::Progress`, at most every 100 ms.
struct Throttled<'a> {
    engine: &'a Engine,
    id: Id,
    total: AtomicU64,
    done: AtomicU64,
    last: Mutex<Option<Instant>>,
}

impl<'a> Throttled<'a> {
    fn new(engine: &'a Engine, id: Id) -> Self {
        Self {
            engine,
            id,
            total: AtomicU64::new(0),
            done: AtomicU64::new(0),
            last: Mutex::new(None),
        }
    }

    fn send(&self, force: bool) {
        let mut last = self.last.lock().expect("progress lock");
        if !force && last.is_some_and(|t| t.elapsed() < PROGRESS_EVERY) {
            return;
        }
        *last = Some(Instant::now());
        self.engine.emit(Event::Progress {
            id: self.id,
            done: self.done.load(Relaxed),
            total: self.total.load(Relaxed),
        });
    }
}

impl Progress for Throttled<'_> {
    fn set_length(&self, total: u64) {
        self.total.store(total, Relaxed);
        self.send(true);
    }

    fn inc(&self, n: u64) {
        self.done.fetch_add(n, Relaxed);
        self.send(false);
    }
}

/// Remove `.url2mp3-*` temp dirs left by a run that was killed or crashed. Only ones older than
/// an hour, so another url2mp3 running right now keeps its own.
async fn sweep_stale_temp(out: &Path) {
    let Ok(mut dir) = tokio::fs::read_dir(out).await else {
        return;
    };
    while let Ok(Some(entry)) = dir.next_entry().await {
        if !entry.file_name().to_string_lossy().starts_with(".url2mp3-") {
            continue;
        }
        let stale = entry
            .metadata()
            .await
            .ok()
            .filter(|m| m.is_dir())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(3600));
        if stale {
            let _ = tokio::fs::remove_dir_all(entry.path()).await;
        }
    }
}
```

- [ ] **Step 11: Run the core tests**

Run: `cargo test -p url2mp3-core`
Expected: all unit tests (resolve 3, download 2, names 2) and the 8 engine tests pass.

- [ ] **Step 12: `crates/cli/src/render.rs`**

```rust
//! Terminal output: an indicatif bar per running link or track, a line per finished one.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

use anyhow::Result;
use indicatif::{HumanBytes, MultiProgress, ProgressBar, ProgressFinish, ProgressStyle};
use url2mp3_core::{Event, Format, Id, Skip};

pub struct Render {
    multi: MultiProgress,
    bar_style: ProgressStyle,
    spin_style: ProgressStyle,
    format: Format,
    out: PathBuf,
    /// Running link or track → its bar and title. Removing a bar clears it from the screen.
    bars: Mutex<HashMap<Id, (ProgressBar, String)>>,
}

impl Render {
    pub fn new(format: Format, out: PathBuf) -> Result<Self> {
        Ok(Self {
            multi: MultiProgress::new(),
            bar_style: ProgressStyle::with_template(
                "{spinner:.cyan} [{bar:22.cyan/blue}] {bytes:>9}/{total_bytes:<9} {binary_bytes_per_sec:>11} {wide_msg}",
            )?
            .progress_chars("=> "),
            spin_style: ProgressStyle::with_template("{spinner:.cyan} {wide_msg}")?,
            format,
            out,
            bars: Mutex::default(),
        })
    }

    pub fn handle(&self, event: Event) {
        let mut bars = self.bars.lock().expect("bars lock");
        match event {
            Event::Probing { id, label, .. } => {
                let pb = self.add(ProgressBar::new_spinner(), &self.spin_style);
                pb.set_message(format!("đang đọc link: {label}"));
                bars.insert(id, (pb, label));
            }
            Event::Playlist { id, title, count } => {
                bars.remove(&id);
                self.println(format!("▸ Playlist {title}: {count} bài"));
            }
            Event::Track { id, title, .. } => {
                let pb = self.add(ProgressBar::new(0), &self.bar_style);
                pb.set_message(title.clone());
                // Replaces (and so clears) the link's probing spinner.
                bars.insert(id, (pb, title));
            }
            Event::Progress { id, done, total } => {
                if let Some((pb, _)) = bars.get(&id) {
                    pb.set_length(total);
                    pb.set_position(done);
                }
            }
            Event::Fallback { id, reason } => {
                if let Some((pb, title)) = bars.get(&id) {
                    if let Some(reason) = reason {
                        self.println(format!("  {title}: tải trực tiếp lỗi ({reason}), chuyển sang yt-dlp"));
                    }
                    pb.set_style(self.spin_style.clone());
                    pb.set_message(format!("yt-dlp đang tải: {title}"));
                }
            }
            Event::Converting { id } => {
                if let Some((pb, title)) = bars.get(&id) {
                    pb.set_style(self.spin_style.clone());
                    pb.set_message(format!("đang chuyển sang {}: {title}", self.format.ext()));
                }
            }
            Event::Done { id, path, bytes, secs } => {
                bars.remove(&id);
                self.println(format!(
                    "✓ {}  ({}, {secs:.1}s)",
                    self.relative(&path).display(),
                    HumanBytes(bytes)
                ));
            }
            Event::Skipped { id, title, reason, .. } => {
                bars.remove(&id);
                match reason {
                    Skip::Duplicate => self.println(format!("• trùng bài, bỏ qua: {title}")),
                    Skip::Exists(path) => self.println(format!(
                        "• đã có: {} (--force để tải lại)",
                        self.relative(&path).display()
                    )),
                }
            }
            Event::Failed { id, label, message, .. } => {
                bars.remove(&id);
                self.println(format!("✗ {label}: {message}"));
            }
        }
    }

    pub fn println(&self, line: impl AsRef<str>) {
        if self.multi.is_hidden() {
            println!("{}", line.as_ref());
        } else {
            let _ = self.multi.println(line);
        }
    }

    pub fn clear(&self) {
        self.bars.lock().expect("bars lock").clear();
        let _ = self.multi.clear();
    }

    fn add(&self, pb: ProgressBar, style: &ProgressStyle) -> ProgressBar {
        let pb = self.multi.add(pb.with_finish(ProgressFinish::AndClear));
        pb.set_style(style.clone());
        pb.enable_steady_tick(Duration::from_millis(120));
        pb
    }

    fn relative<'a>(&self, path: &'a Path) -> &'a Path {
        path.strip_prefix(&self.out).unwrap_or(path)
    }
}
```

- [ ] **Step 13: `crates/cli/src/main.rs`** — keep `Cli`, `main`, `default_jobs`,
  `collect_inputs` exactly as they are; replace everything from `mod download;` to the imports
  and from `struct Ctx` to the end of the file with:

```rust
mod render;

use std::{
    collections::HashSet,
    io::{BufRead, IsTerminal},
    path::PathBuf,
    process::ExitCode,
    sync::Arc,
    time::Instant,
};

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use indicatif::HumanBytes;
use tokio::task::JoinSet;
use url2mp3_core::{Engine, Format, Options, Outcome, Tools};

use crate::render::Render;
```

```rust
async fn run(cli: Cli, inputs: Vec<String>) -> Result<ExitCode> {
    let tools = Tools::from_path();
    let (ytdlp_version, ffmpeg) = tokio::join!(tools.ytdlp_version(), tools.check_ffmpeg());
    let ytdlp_version = ytdlp_version.map_err(|_| anyhow!("chưa có yt-dlp. Cài: brew install yt-dlp"))?;
    ffmpeg.map_err(|_| anyhow!("chưa có ffmpeg. Cài: brew install ffmpeg"))?;

    let out = match cli.out {
        Some(p) => p,
        None => url2mp3_core::default_out_dir().context("không tìm thấy thư mục Downloads, dùng -o")?,
    };
    let render = Arc::new(Render::new(cli.format, out.clone())?);
    let sink = {
        let render = render.clone();
        move |event| render.handle(event)
    };
    let opts = Options {
        format: cli.format,
        bitrate: cli.bitrate,
        out: out.clone(),
        jobs: cli.jobs.into(),
        connections: cli.connections.into(),
        playlist: cli.playlist,
        limit: cli.limit,
        cover: !cli.no_cover,
        force: cli.force,
        cookies_from_browser: cli.cookies_from_browser,
    };
    let engine = Engine::start(tools, opts, sink).await?;

    render.println(format!(
        "url2mp3 · yt-dlp {ytdlp_version} · {} · {} bài cùng lúc × {} kết nối → {}",
        cli.format.ext(),
        cli.jobs,
        cli.connections,
        out.display()
    ));

    let started = Instant::now();
    let mut set = JoinSet::new();
    for input in inputs {
        set.spawn(engine.clone().process(engine.next_id(), input));
    }
    let mut outcomes = Vec::new();
    let all_done = async {
        while let Some(res) = set.join_next().await {
            outcomes.extend(res.unwrap_or_default());
        }
    };
    let cancelled = tokio::select! {
        () = all_done => false,
        _ = tokio::signal::ctrl_c() => true,
    };
    if cancelled {
        // Stop every task (and its ffmpeg/yt-dlp child) before the engine removes its temp dir.
        set.shutdown().await;
        render.clear();
        eprintln!("\nĐã hủy.");
        return Ok(ExitCode::from(130));
    }

    let (mut done, mut bytes, mut skipped, mut failed, mut network) = (0, 0, 0, 0, 0);
    for o in &outcomes {
        match o {
            Outcome::Done(n) => (done, bytes) = (done + 1, bytes + n),
            Outcome::Skipped => skipped += 1,
            Outcome::Failed { network: net } => (failed, network) = (failed + 1, network + *net as u32),
        }
    }
    let mut summary = format!(
        "Xong {done} bài · {} · {:.1}s → {}",
        HumanBytes(bytes),
        started.elapsed().as_secs_f64(),
        out.display()
    );
    if skipped > 0 {
        summary += &format!(" · bỏ qua {skipped} bài đã có hoặc trùng");
    }
    if failed > 0 {
        summary += &format!(" · lỗi {failed} bài");
    }
    render.println(summary);
    if network > 0 {
        render.println(format!(
            "Gợi ý: {network} bài lỗi mạng/DNS (mạng chập chờn, VPN/WARP), không phải lỗi YouTube."
        ));
    }
    if failed > network {
        render.println("Gợi ý: lỗi YouTube thường do yt-dlp cũ, cập nhật: brew upgrade yt-dlp");
    }
    if failed > 0 {
        render.println("Chạy lại đúng lệnh này để tải các bài lỗi: bài đã tải xong sẽ tự bỏ qua.");
    }
    Ok(if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
```

- [ ] **Step 14: Whole workspace green**

Run: `cargo fmt --all && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: 15 tests pass (7 unit + 8 engine), no warnings.

- [ ] **Step 15: Real smoke test (network)**

```bash
cargo build --release
OUT=$(mktemp -d)
target/release/url2mp3 -o "$OUT" https://www.youtube.com/watch?v=Llw9Q6akRo4
target/release/url2mp3 -o "$OUT" https://www.youtube.com/watch?v=Llw9Q6akRo4   # → "• đã có"
target/release/url2mp3 -o "$OUT" -f opus "never gonna give you up"
ls -la "$OUT"; ffprobe -hide_banner "$OUT"/*.mp3 2>&1 | rg -i "title|artist|Video"
```

Expected: progress bar during download, `✓ …mp3`, the second run prints `• đã có: … (--force để tải lại)`,
search downloads an `.opus`, the mp3 has title/artist tags and an attached cover, no `.url2mp3-*`
dir left in `$OUT`.

- [ ] **Step 16: Commit**

```bash
git add -A Cargo.toml Cargo.lock crates
git commit -m "refactor: extract url2mp3-core engine with progress events"
```

---

### Task 3: Docs follow the new layout

**Files:**
- Modify: `README.md`, `README.vi.md`

- [ ] **Step 1: Update both READMEs**

- Install from git: `cargo install --git https://github.com/lecaotanloc289/url2mp3 url2mp3`
  (the package name is needed once the repo has more than one binary).
- From a clone: `cargo install --path crates/cli`.
- "How it works": paths become `crates/core/src/resolve.rs`, `crates/core/src/download.rs`,
  `crates/core/src/transcode.rs`; add one line that `crates/core` (`url2mp3-core`) is the engine
  shared by the CLI (`crates/cli`) and the upcoming apps, reporting progress as events.
- Development: `cargo test --workspace`, `cargo clippy --workspace --all-targets`.

- [ ] **Step 2: Check for stale paths**

Run: `rg -n "src/(resolve|download|transcode|main)\.rs|--path \.\s*$" README.md README.vi.md`
Expected: no matches.

- [ ] **Step 3: Commit**

```bash
git add README.md README.vi.md
git commit -m "docs: README for the workspace layout"
```
