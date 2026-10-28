//! Trimmed encode against the real ffmpeg; skipped when it is not installed.
#![cfg(unix)]

use std::{path::Path, process::Command};

use url2mp3_core::{Format, Job, PEAK_BUCKETS, Range, Tags, Tools, encode, make_preview, peaks};

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn ffmpeg(args: &[&str], out: &Path) {
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .arg(out)
        .status()
        .unwrap();
    assert!(status.success(), "ffmpeg {args:?} failed");
}

fn probe(path: &Path, args: &[&str]) -> String {
    let out = Command::new("ffprobe")
        .args(["-v", "error"])
        .args(args)
        .arg(path)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[tokio::test]
async fn trimmed_encode_keeps_only_the_kept_ranges() {
    if !have("ffmpeg") || !have("ffprobe") {
        eprintln!("ffmpeg/ffprobe not on PATH, skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (src, cover, out) = (
        dir.path().join("in.opus"),
        dir.path().join("c.jpg"),
        dir.path().join("out.mp3"),
    );
    ffmpeg(&["-f", "lavfi", "-i", "sine=f=440:d=10", "-c:a", "libopus"], &src);
    ffmpeg(
        &["-f", "lavfi", "-i", "testsrc=s=64x48:d=1", "-frames:v", "1"],
        &cover,
    );

    let keep = [
        Range { start: 1.0, end: 3.0 },
        Range {
            start: 6.0,
            end: 10.0,
        },
    ];
    let job = Job {
        input: &src,
        cover: Some(&cover),
        output: &out,
        format: Format::Mp3,
        src_codec: "opus",
        bitrate: None,
        tags: Tags {
            title: "t",
            artist: None,
            album: None,
            comment: None,
        },
        keep: Some(&keep),
    };
    encode(&Tools::from_path(), job).await.unwrap();

    let duration: f64 = probe(&out, &["-show_entries", "format=duration", "-of", "csv=p=0"])
        .parse()
        .unwrap();
    assert!((5.95..=6.10).contains(&duration), "duration {duration}");
    let video = probe(
        &out,
        &[
            "-select_streams",
            "v",
            "-show_entries",
            "stream=codec_name",
            "-of",
            "csv=p=0",
        ],
    );
    assert!(!video.is_empty(), "no attached picture stream");
}

/// yt-dlp says 10 s for a 10.4 s track: the measured length and an open end keep the tail.
#[tokio::test]
async fn open_ended_cut_keeps_the_tail_past_the_rounded_duration() {
    if !have("ffmpeg") || !have("ffprobe") {
        eprintln!("ffmpeg/ffprobe not on PATH, skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (src, out) = (dir.path().join("in.opus"), dir.path().join("out.mp3"));
    ffmpeg(
        &["-f", "lavfi", "-i", "sine=f=440:d=10.4", "-c:a", "libopus"],
        &src,
    );
    let tools = Tools::from_path();

    let measured = peaks(&tools, &src, PEAK_BUCKETS).await.unwrap().seconds;
    assert!((measured - 10.4).abs() < 0.03, "measured {measured} s");

    let keep = [Range {
        start: 2.0,
        end: f64::INFINITY,
    }];
    let job = Job {
        input: &src,
        cover: None,
        output: &out,
        format: Format::Mp3,
        src_codec: "opus",
        bitrate: None,
        tags: Tags {
            title: "t",
            artist: None,
            album: None,
            comment: None,
        },
        keep: Some(&keep),
    };
    encode(&tools, job).await.unwrap();

    let duration: f64 = probe(&out, &["-show_entries", "format=duration", "-of", "csv=p=0"])
        .parse()
        .unwrap();
    assert!((duration - 8.4).abs() <= 0.06, "duration {duration}, not 8.0");
}

#[tokio::test]
async fn preview_is_aac_and_peaks_have_1200_buckets() {
    if !have("ffmpeg") || !have("ffprobe") {
        eprintln!("ffmpeg/ffprobe not on PATH, skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (src, out) = (dir.path().join("in.opus"), dir.path().join("preview.m4a"));
    ffmpeg(
        &["-f", "lavfi", "-i", "sine=f=440:d=5,volume=6", "-c:a", "libopus"],
        &src,
    );

    let tools = Tools::from_path();
    make_preview(&tools, &src, "opus", &out).await.unwrap();
    let codec = probe(
        &out,
        &[
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name",
            "-of",
            "csv=p=0",
        ],
    );
    assert_eq!(codec, "aac");

    let wave = peaks(&tools, &src, PEAK_BUCKETS).await.unwrap();
    assert_eq!(wave.peaks.len(), 1200);
    let max = wave.peaks.iter().copied().fold(0.0, f32::max);
    assert!(max > 0.5 && max <= 1.0, "max {max}");
    assert!((wave.seconds - 5.0).abs() < 0.03, "measured {} s", wave.seconds);

    assert!(peaks(&tools, &dir.path().join("missing.opus"), 10).await.is_err());
}
