//! The whole trim pipeline against real YouTube, yt-dlp and ffmpeg (all on PATH, plus deno).
//! Needs the network, so it is ignored by default:
//! `cargo test -p url2mp3-core --test real_youtube -- --ignored --nocapture`

use std::{path::Path, process::Command};

use url2mp3_core::{
    Cut, Engine, Format, Options, Outcome, PEAK_BUCKETS, Range, Resolved, Tools, make_preview, peaks,
    suggestions,
};

const LINK: &str = "https://www.youtube.com/watch?v=Llw9Q6akRo4";

fn ffprobe(file: &Path, entry: &str) -> String {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a:0", "-show_entries", entry])
        .args(["-of", "default=nw=1:nk=1"])
        .arg(file)
        .output()
        .expect("ffprobe on PATH");
    assert!(
        out.status.success(),
        "ffprobe failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn duration(file: &Path) -> f64 {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=nw=1:nk=1",
        ])
        .arg(file)
        .output()
        .expect("ffprobe on PATH");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .expect("duration")
}

#[tokio::test]
#[ignore = "needs the network, yt-dlp, ffmpeg and deno"]
async fn trim_pipeline_on_real_youtube() {
    let (cache, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let opts = Options {
        format: Format::Mp3,
        bitrate: None,
        out: out.path().to_owned(),
        jobs: 2,
        connections: 8,
        playlist: false,
        limit: None,
        cover: true,
        force: false,
        cookies_from_browser: None,
    };
    let engine = Engine::start(Tools::from_path(), opts, |_| {}).await.unwrap();
    let id = engine.next_id();

    let Resolved::Track(track) = engine.resolve(id, LINK).await.expect("resolve") else {
        panic!("a watch link is one track");
    };
    let length = track.duration.expect("duration");
    println!(
        "resolved: {:?} ({length} s), codec {}",
        track.title,
        track.codec()
    );
    assert!(length > 200.0, "duration {length}");
    assert!(track.youtube);

    let audio = cache.path().join("audio");
    engine.fetch(id, &track, &audio).await.expect("fetch");
    assert!(audio.metadata().unwrap().len() > 100_000);

    let preview = cache.path().join("preview.m4a");
    make_preview(engine.tools(), &audio, track.codec(), &preview)
        .await
        .expect("preview");
    let codec = ffprobe(&preview, "stream=codec_name");
    println!("preview codec: {codec}");
    assert_eq!(codec, "aac");

    let bars = peaks(engine.tools(), &audio, PEAK_BUCKETS).await.expect("peaks");
    assert_eq!(bars.len(), 1200);
    assert!(bars.iter().any(|p| *p > 0.0));

    let client = url2mp3_core::http_client().build().unwrap();
    let found = suggestions(&client, track.video_id.as_deref().unwrap()).await;
    println!("sponsorblock suggestions: {found:?}");

    let cut = Cut {
        keep: vec![
            Range {
                start: 30.0,
                end: 60.0,
            },
            Range {
                start: 90.0,
                end: 120.0,
            },
        ],
        label: "đã cắt".into(),
    };
    let outcome = engine
        .save(id, None, &track, None, Some(&audio), Some(&cut))
        .await;
    assert!(matches!(outcome, Outcome::Done(_)), "{outcome:?}");

    let saved: Vec<_> = std::fs::read_dir(out.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        // The engine's hidden scratch folder lives here until it is dropped.
        .filter(|p| !p.file_name().unwrap().to_string_lossy().starts_with('.'))
        .collect();
    println!("saved: {saved:?}");
    assert_eq!(saved.len(), 1, "only the trimmed copy is written");
    let name = saved[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.ends_with(" (đã cắt).mp3"), "{name}");
    let secs = duration(&saved[0]);
    println!("trimmed duration: {secs} s");
    assert!((59.8..=60.3).contains(&secs), "duration {secs}");
}
