//! Trimmed encode against the real ffmpeg; skipped when it is not installed.
#![cfg(unix)]

use std::{path::Path, process::Command};

use url2mp3_core::{Format, Job, Range, Tags, Tools, encode};

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
