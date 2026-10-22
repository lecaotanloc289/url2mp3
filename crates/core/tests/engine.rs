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
    assert_eq!(
        kinds(&events),
        ["probing", "track", "fallback", "converting", "done"]
    );
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
    assert!(
        matches!(
            last,
            Event::Skipped {
                id: 1,
                reason: Skip::Duplicate,
                ..
            }
        ),
        "{last:?}"
    );
}

#[tokio::test]
async fn failure_carries_the_ytdlp_error_line() {
    let (bin, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (engine, events) = engine(fakes(bin.path()), out.path()).await;
    let link = "https://example.com/bad";

    assert_eq!(
        process(&engine, link).await,
        vec![Outcome::Failed { network: false }]
    );

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
    tools
        .env
        .push(("FAIL_ONCE".into(), bin.path().join("failed").into()));
    let (engine, _) = engine(tools, out.path()).await;

    assert_eq!(
        process(&engine, LINK).await,
        vec![Outcome::Failed { network: false }]
    );
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
        [
            "probing",
            "playlist",
            "probing",
            "track",
            "fallback",
            "converting",
            "done"
        ]
    );
    let events = events.lock().unwrap();
    assert!(matches!(&events[1], Event::Playlist { id: 0, title, count: 1 } if title == "Mix"));
    assert!(matches!(
        &events[3],
        Event::Track {
            id: 1,
            parent: Some(0),
            ..
        }
    ));
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
    assert!(
        log.lines().all(|l| l.contains("--js-runtimes deno:/opt/deno")),
        "{log}"
    );
}
