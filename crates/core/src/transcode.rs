//! Steps 5–6: ffmpeg re-encodes (or just remuxes when the source codec already matches),
//! embeds the cover and writes the tags in one pass.

use std::{path::Path, process::Stdio};

use anyhow::{Context, Result, bail};

use crate::tools::Tools;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum Format {
    /// MP3: chơi được ở mọi nơi
    Mp3,
    /// M4A (AAC): giữ nguyên luồng AAC gốc, không nén lại
    M4a,
    /// Opus: giữ nguyên luồng Opus gốc của YouTube, chất lượng cao nhất, file nhỏ nhất
    Opus,
}

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Mp3 => "mp3",
            Format::M4a => "m4a",
            Format::Opus => "opus",
        }
    }

    /// yt-dlp format selector: prefer the source stream that can be copied without re-encoding.
    pub fn selector(self) -> &'static str {
        match self {
            Format::Mp3 => "bestaudio/best",
            Format::M4a => "bestaudio[ext=m4a]/bestaudio/best",
            Format::Opus => "bestaudio[acodec=opus]/bestaudio/best",
        }
    }

    /// The Ogg muxer cannot embed a picture stream.
    pub fn supports_cover(self) -> bool {
        self != Format::Opus
    }

    fn can_copy(self, src_codec: &str) -> bool {
        match self {
            Format::Mp3 => src_codec == "mp3",
            Format::M4a => src_codec.starts_with("mp4a") || src_codec == "aac",
            Format::Opus => src_codec == "opus",
        }
    }

    fn muxer(self) -> &'static str {
        match self {
            Format::Mp3 => "mp3",
            Format::M4a => "ipod",
            Format::Opus => "opus",
        }
    }
}

pub struct Tags<'a> {
    pub title: &'a str,
    pub artist: Option<&'a str>,
    pub album: Option<&'a str>,
    pub comment: Option<&'a str>,
}

pub struct Job<'a> {
    pub input: &'a Path,
    pub cover: Option<&'a Path>,
    pub output: &'a Path,
    pub format: Format,
    pub src_codec: &'a str,
    pub bitrate: Option<u32>,
    pub tags: Tags<'a>,
}

pub async fn encode(tools: &Tools, job: Job<'_>) -> Result<()> {
    let mut cmd = tools.ffmpeg_command();
    cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i"])
        .arg(job.input);
    if let Some(cover) = job.cover {
        cmd.arg("-i").arg(cover);
    }
    cmd.args(["-map", "0:a:0", "-map_metadata", "-1"]);

    if job.cover.is_some() {
        // Square, centre-cropped cover: what music players expect.
        cmd.args([
            "-map",
            "1:v:0",
            "-filter:v",
            "crop='min(iw,ih)':'min(iw,ih)'",
            "-c:v",
            "mjpeg",
            "-disposition:v:0",
            "attached_pic",
        ]);
        if job.format == Format::Mp3 {
            cmd.args([
                "-metadata:s:v",
                "title=Album cover",
                "-metadata:s:v",
                "comment=Cover (front)",
            ]);
        }
    }

    if job.format.can_copy(job.src_codec) {
        cmd.args(["-c:a", "copy"]);
    } else {
        match (job.format, job.bitrate) {
            // LAME V0: VBR ~245 kbps, transparent for a 128–160 kbps source.
            (Format::Mp3, None) => cmd.args(["-c:a", "libmp3lame", "-q:a", "0"]),
            (Format::Mp3, Some(k)) => cmd.args(["-c:a", "libmp3lame", "-b:a"]).arg(format!("{k}k")),
            (Format::M4a, k) => cmd
                .args(["-c:a", "aac", "-b:a"])
                .arg(format!("{}k", k.unwrap_or(256))),
            (Format::Opus, k) => cmd
                .args(["-c:a", "libopus", "-b:a"])
                .arg(format!("{}k", k.unwrap_or(160))),
        };
    }

    let tags = &job.tags;
    let pairs = [
        ("title", Some(tags.title)),
        ("artist", tags.artist),
        ("album", tags.album),
        ("comment", tags.comment),
    ];
    for (key, value) in pairs {
        if let Some(v) = value.filter(|v| !v.is_empty()) {
            cmd.arg("-metadata").arg(format!("{key}={v}"));
        }
    }
    if job.format == Format::Mp3 {
        cmd.args(["-id3v2_version", "3"]);
    }
    cmd.arg("-f").arg(job.format.muxer()).arg(job.output);

    let out = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .context("không chạy được ffmpeg")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("ffmpeg: {}", err.lines().last().unwrap_or("lỗi không rõ").trim());
    }
    Ok(())
}
