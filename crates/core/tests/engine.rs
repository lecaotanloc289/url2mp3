//! Engine runs against fake yt-dlp/ffmpeg shell scripts: no network, no real media.
#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use url2mp3_core::{Cut, Engine, Event, Format, Options, Outcome, Range, Resolved, Skip, Tools};

/// `-J` prints a track (or a playlist for links containing "playlist"); otherwise writes
/// "audio" to the `-o` path (with `FAKE_SLEEP`: a partial "aud", then the full file after that
/// many seconds). Links containing "bad" fail like a private video.
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
  # Links containing "other" are another video with the same title.
  case "$prev" in *other*) id=xyz;; *) id=abc;; esac
  echo "{\"id\":\"$id\",\"title\":\"Fake Song\",\"uploader\":\"Tester\",\"webpage_url\":\"https://example.com/v/$id\",\"acodec\":\"mp3\",\"duration\":272,\"extractor_key\":\"Youtube\"}"
else
  if [ -n "$FAKE_SLEEP" ]; then printf 'aud' > "$out"; sleep "$FAKE_SLEEP"; fi
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
        ytdlp_args: Vec::new(),
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

#[test]
fn events_serialize_for_the_apps() {
    let done = Event::Done {
        id: 0,
        path: "/x/a.mp3".into(),
        bytes: 5,
        secs: 1.0,
    };
    assert_eq!(
        serde_json::to_value(&done).unwrap(),
        serde_json::json!({"kind": "done", "id": 0, "path": "/x/a.mp3", "bytes": 5, "secs": 1.0})
    );
    let skipped = Event::Skipped {
        id: 1,
        parent: Some(0),
        title: "t".into(),
        reason: Skip::Exists("/x".into()),
    };
    assert_eq!(
        serde_json::to_value(&skipped).unwrap(),
        serde_json::json!({"kind": "skipped", "id": 1, "parent": 0, "title": "t",
                           "reason": {"kind": "exists", "path": "/x"}})
    );
    assert_eq!(
        serde_json::to_value(Skip::Duplicate).unwrap(),
        serde_json::json!({"kind": "duplicate"})
    );
    assert_eq!(serde_json::to_value(Format::M4a).unwrap(), "m4a");
    assert_eq!(serde_json::from_str::<Format>("\"opus\"").unwrap(), Format::Opus);
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

/// The app resolves links and fetches audio into its tray long before the user saves:
/// nothing may appear in the output folder until then.
#[tokio::test]
async fn start_and_resolve_do_not_touch_the_output_folder() {
    let (bin, parent) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let out = parent.path().join("Music");
    let (engine, _) = engine(fakes(bin.path()), &out).await;
    let track = resolve_track(&engine, LINK).await;
    engine
        .fetch(1, &track, &bin.path().join("cache.audio"))
        .await
        .unwrap();
    assert!(!out.exists(), "start/resolve/fetch created {}", out.display());

    assert_eq!(
        engine.save(1, None, &track, None, None, None).await,
        Outcome::Done(5)
    );
    assert_eq!(fs::read(out.join("Fake Song.mp3")).unwrap(), b"audio");

    drop(engine);
    let left: Vec<_> = fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, ["Fake Song.mp3"], "the temp dir goes with the engine");
}

#[tokio::test]
async fn only_stale_folders_with_the_prefix_are_removed() {
    let parent = tempfile::tempdir().unwrap();
    let two_hours_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
    for name in ["run-old", "run-busy", "run-new", "other-old"] {
        fs::create_dir(parent.path().join(name)).unwrap();
    }
    // "run-busy" is old itself but something inside it was just written.
    fs::write(parent.path().join("run-busy").join("audio"), b"a").unwrap();
    for name in ["run-old", "run-busy", "other-old"] {
        let dir = fs::File::open(parent.path().join(name)).unwrap();
        dir.set_modified(two_hours_ago).unwrap();
    }

    url2mp3_core::remove_stale_dirs(parent.path(), "run-").await;

    let mut left: Vec<_> = fs::read_dir(parent.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(left, ["other-old", "run-busy", "run-new"]);
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
async fn another_video_with_the_title_of_an_existing_file_gets_its_id_appended() {
    let (bin, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    fs::write(out.path().join("Fake Song.mp3"), b"old").unwrap();
    let (engine, _) = engine(fakes(bin.path()), out.path()).await;

    assert_eq!(process(&engine, LINK).await, vec![Outcome::Skipped]);
    assert_eq!(
        process(&engine, "https://example.com/v/other").await,
        vec![Outcome::Done(5)]
    );
    assert!(out.path().join("Fake Song [xyz].mp3").exists());
}

#[tokio::test]
async fn failed_track_leaves_no_temp_files() {
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

    let tmp = fs::read_dir(out.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.file_name().unwrap().to_string_lossy().starts_with(".url2mp3-"))
        .expect("engine temp dir");
    assert_eq!(fs::read_dir(&tmp).unwrap().count(), 0);
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

/// Android runs yt-dlp as `libpython.so <yt-dlp script> …`, and an app's own ffmpeg isn't on
/// PATH, so yt-dlp is told where it is.
#[tokio::test]
async fn ytdlp_args_come_first_and_ffmpeg_location_is_passed() {
    let (bin, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let log = bin.path().join("log");
    let mut tools = fakes(bin.path());
    tools.ytdlp_args = vec!["/data/yt-dlp".into()];
    tools.env.push(("FAKE_LOG".into(), log.clone().into()));
    let ffmpeg = tools.ffmpeg.display().to_string();
    let (engine, _) = engine(tools, out.path()).await;

    process(&engine, LINK).await;

    let log = fs::read_to_string(log).unwrap();
    assert!(log.lines().all(|l| l.starts_with("/data/yt-dlp ")), "{log}");
    assert!(
        log.lines()
            .all(|l| l.contains(&format!("--ffmpeg-location {ffmpeg}"))),
        "{log}"
    );
    assert!(Tools::from_path().ytdlp_args.is_empty());
}

async fn resolve_track(engine: &Arc<Engine>, input: &str) -> url2mp3_core::Track {
    match engine.resolve(engine.next_id(), input).await.unwrap() {
        Resolved::Track(t) => t,
        Resolved::Playlist { .. } => panic!("{input} resolved to a playlist"),
    }
}

fn trim(label: &str) -> Cut {
    Cut {
        keep: vec![Range {
            start: 0.0,
            end: 10.0,
        }],
        label: label.into(),
    }
}

#[tokio::test]
async fn resolve_then_save_from_cached_audio_never_runs_ytdlp_download() {
    let (bin, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let log = bin.path().join("log");
    let mut tools = fakes(bin.path());
    tools.env.push(("FAKE_LOG".into(), log.clone().into()));
    let (engine, events) = engine(tools, out.path()).await;

    let track = resolve_track(&engine, LINK).await;
    assert_eq!(track.title, "Fake Song");
    assert_eq!(track.duration, Some(272.0));
    assert!(track.youtube);
    assert_eq!(track.video_id.as_deref(), Some("abc"));
    assert!(!track.needs_probe());
    assert_eq!(track.codec(), "mp3");
    assert_eq!(kinds(&events), ["probing"]);

    let cached = bin.path().join("cached.audio");
    fs::write(&cached, b"cached").unwrap();
    let outcome = engine.save(7, None, &track, None, Some(&cached), None).await;

    assert_eq!(outcome, Outcome::Done(6));
    assert_eq!(fs::read(out.path().join("Fake Song.mp3")).unwrap(), b"cached");
    assert_eq!(
        fs::read(&cached).unwrap(),
        b"cached",
        "caller's audio is left alone"
    );
    assert_eq!(kinds(&events), ["probing", "track", "converting", "done"]);
    let log = fs::read_to_string(log).unwrap();
    assert_eq!(log.lines().count(), 1, "only the probe: {log}");
    assert!(log.contains("-J") && !log.contains("-o "), "{log}");
}

/// The app starts a new engine when settings change; tracks in its tray keep their audio.
#[tokio::test]
async fn cached_audio_is_saved_by_an_engine_with_another_format_without_fetching() {
    let (bin, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (first, _) = engine(fakes(bin.path()), out.path()).await;
    let track = resolve_track(&first, LINK).await;
    let cached = bin.path().join("cached.audio");
    fs::write(&cached, b"cached").unwrap();

    let log = bin.path().join("log");
    let mut tools = fakes(bin.path());
    tools.env.push(("FAKE_LOG".into(), log.clone().into()));
    let opts = Options {
        format: Format::M4a,
        ..options(out.path())
    };
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = {
        let events = events.clone();
        move |e: Event| events.lock().unwrap().push(e)
    };
    let second = Engine::start(tools, opts, sink).await.unwrap();

    let outcome = second.save(0, None, &track, None, Some(&cached), None).await;

    assert_eq!(outcome, Outcome::Done(6));
    assert_eq!(fs::read(out.path().join("Fake Song.m4a")).unwrap(), b"cached");
    assert_eq!(kinds(&events), ["track", "converting", "done"]);
    assert!(!log.exists(), "yt-dlp ran: {:?}", fs::read_to_string(&log));
}

#[tokio::test]
async fn trimmed_save_sits_next_to_the_whole_one_and_is_numbered() {
    let (bin, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (engine, events) = engine(fakes(bin.path()), out.path()).await;
    let track = resolve_track(&engine, LINK).await;
    let cut = trim("đã cắt");

    assert_eq!(
        engine.save(1, None, &track, None, None, None).await,
        Outcome::Done(5)
    );
    assert_eq!(
        engine.save(2, None, &track, None, None, Some(&cut)).await,
        Outcome::Done(5)
    );
    assert_eq!(
        engine.save(3, None, &track, None, None, Some(&cut)).await,
        Outcome::Done(5)
    );

    for name in [
        "Fake Song.mp3",
        "Fake Song (đã cắt).mp3",
        "Fake Song (đã cắt 2).mp3",
    ] {
        assert!(out.path().join(name).exists(), "{name}");
    }
    let events = events.lock().unwrap();
    assert!(!events.iter().any(|e| matches!(e, Event::Skipped { .. })));
    let done: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::Done { id, path, .. } => Some((*id, path.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        done,
        [
            (1, out.path().join("Fake Song.mp3")),
            (2, out.path().join("Fake Song (đã cắt).mp3")),
            (3, out.path().join("Fake Song (đã cắt 2).mp3")),
        ]
    );
}

#[tokio::test]
async fn trimmed_save_of_a_file_already_in_the_folder_is_saved_alongside() {
    let (bin, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    fs::write(out.path().join("Fake Song.mp3"), b"old").unwrap();
    fs::write(out.path().join("Fake Song (trimmed).mp3"), b"old").unwrap();
    let (engine, _) = engine(fakes(bin.path()), out.path()).await;

    // The whole one is skipped and keeps the video's claim; the trimmed one is not blocked.
    assert_eq!(process(&engine, LINK).await, vec![Outcome::Skipped]);
    let track = resolve_track(&engine, LINK).await;
    let outcome = engine
        .save(9, None, &track, None, None, Some(&trim("trimmed")))
        .await;

    assert_eq!(outcome, Outcome::Done(5));
    assert_eq!(
        fs::read(out.path().join("Fake Song (trimmed).mp3")).unwrap(),
        b"old"
    );
    assert!(out.path().join("Fake Song (trimmed 2).mp3").exists());
}

#[tokio::test]
async fn trimmed_save_with_nothing_kept_fails() {
    let (bin, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (engine, events) = engine(fakes(bin.path()), out.path()).await;
    let track = resolve_track(&engine, LINK).await;
    let cut = Cut {
        keep: vec![],
        label: "đã cắt".into(),
    };

    let outcome = engine.save(1, None, &track, None, None, Some(&cut)).await;

    assert_eq!(outcome, Outcome::Failed { network: false });
    let last = events.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        last,
        Event::Failed {
            id: 1,
            parent: None,
            label: "Fake Song".into(),
            message: "không còn đoạn nào để lưu".into(),
            network: false,
        }
    );
    assert!(!out.path().join("Fake Song (đã cắt).mp3").exists());
}

#[tokio::test]
async fn whole_destination_matches_where_a_whole_save_lands() {
    let (bin, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (engine, events) = engine(fakes(bin.path()), out.path()).await;
    let last_done = || match events.lock().unwrap().last().cloned() {
        Some(Event::Done { path, .. }) => path,
        other => panic!("{other:?}"),
    };

    let track = resolve_track(&engine, LINK).await;
    let expected = engine.whole_destination(&track, None);
    assert_eq!(expected, out.path().join("Fake Song.mp3"));
    assert_eq!(
        engine.save(1, None, &track, None, None, None).await,
        Outcome::Done(5)
    );
    assert_eq!(last_done(), expected);

    let Resolved::Playlist { title, tracks } = engine
        .resolve(2, "https://example.com/playlist?list=1")
        .await
        .unwrap()
    else {
        panic!("not a playlist");
    };
    assert_eq!(title, "Mix");
    assert_eq!(tracks.len(), 1);
    assert!(tracks[0].needs_probe());
    let entry = engine.probe_track(&tracks[0]).await.unwrap();
    assert!(!entry.needs_probe());
    let expected = engine.whole_destination(&entry, Some(&title));
    assert_eq!(expected, out.path().join("Mix/Fake Song.mp3"));
    // A new engine: the first one already holds this video's claim.
    let (other, other_events) = self::engine(fakes(bin.path()), out.path()).await;
    assert_eq!(
        other.save(3, Some(2), &entry, Some(&title), None, None).await,
        Outcome::Done(5)
    );
    assert!(matches!(
        other_events.lock().unwrap().last(),
        Some(Event::Done { path, .. }) if *path == expected
    ));
}

#[tokio::test]
async fn fetch_writes_the_audio_to_the_callers_path() {
    let (bin, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (engine, events) = engine(fakes(bin.path()), out.path()).await;
    let track = resolve_track(&engine, LINK).await;
    let to = bin.path().join("cache.audio");

    engine.fetch(4, &track, &to).await.unwrap();

    assert_eq!(fs::read(&to).unwrap(), b"audio");
    assert!(!bin.path().join("cache.audio.part").exists());
    assert!(matches!(
        events.lock().unwrap().last(),
        Some(Event::Fallback { id: 4, reason: None })
    ));
}

#[tokio::test]
async fn cancelled_fetch_leaves_no_files() {
    let (bin, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut tools = fakes(bin.path());
    // Short enough that a yt-dlp left running would write its file before the test ends.
    tools.env.push(("FAKE_SLEEP".into(), "1".into()));
    let (engine, _) = engine(tools, out.path()).await;
    let track = resolve_track(&engine, LINK).await;
    let cache = tempfile::tempdir().unwrap();
    let to = cache.path().join("cache.audio");

    let task = tokio::spawn({
        let (engine, to) = (engine.clone(), to.clone());
        async move { engine.fetch(0, &track, &to).await }
    });
    // Abort mid-download, once yt-dlp has written part of the file.
    let part = cache.path().join("cache.audio.part");
    for _ in 0..300 {
        if part.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(part.exists(), "fetch writes to <to>.part");
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    // Past the fake's sleep: a surviving yt-dlp would have written `<to>.part` by now.
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

    assert!(!to.exists());
    let left: Vec<_> = fs::read_dir(cache.path()).unwrap().collect();
    assert!(left.is_empty(), "{left:?}");
}
