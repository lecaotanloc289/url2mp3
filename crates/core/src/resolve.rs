//! Steps 1–3 of the pipeline (read the link, solve signatures, pick the audio stream) are
//! delegated to yt-dlp: YouTube rotates its player cipher every few weeks and yt-dlp tracks
//! that upstream. We only ask it for JSON and do the heavy lifting ourselves.

use std::{collections::HashMap, path::Path, process::Stdio};

use anyhow::{Context, Result, bail};
use reqwest::header::{ACCEPT_ENCODING, COOKIE, HeaderMap, HeaderName, HeaderValue};
use serde::Deserialize;
use tokio::process::Command;

use crate::tools::Tools;

#[derive(Debug, Deserialize)]
pub struct Info {
    #[serde(rename = "_type")]
    pub kind: Option<String>,
    pub id: Option<String>,
    pub title: Option<String>,
    /// Direct media URL of the selected format (for `_type: "url"` entries: the page URL).
    pub url: Option<String>,
    pub protocol: Option<String>,
    pub acodec: Option<String>,
    /// Length in seconds, when the extractor knows it.
    pub duration: Option<f64>,
    /// yt-dlp's extractor, e.g. "Youtube".
    pub extractor_key: Option<String>,
    pub filesize: Option<f64>,
    pub http_headers: Option<HashMap<String, String>>,
    /// Set-Cookie style string: `name=value; Domain=..; Path=..; name2=value2; ...`
    pub cookies: Option<String>,
    pub thumbnail: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub uploader: Option<String>,
    pub channel: Option<String>,
    pub webpage_url: Option<String>,
    pub entries: Option<Vec<Option<Info>>>,
}

impl Info {
    pub fn is_playlist(&self) -> bool {
        self.kind.as_deref() == Some("playlist") || self.entries.is_some()
    }

    /// Flat playlist entry: only a page URL, still needs its own probe.
    pub fn is_reference(&self) -> bool {
        matches!(self.kind.as_deref(), Some("url" | "url_transparent"))
    }

    /// Media URL we can fetch with plain HTTP range requests (not HLS/DASH manifests).
    pub fn direct_url(&self) -> Option<&str> {
        let url = self.url.as_deref()?;
        match self.protocol.as_deref() {
            Some("https" | "http") => Some(url),
            None if url.starts_with("http") => Some(url),
            _ => None,
        }
    }

    pub fn exact_size(&self) -> Option<u64> {
        self.filesize.filter(|n| *n > 0.0).map(|n| n as u64)
    }

    pub fn display_artist(&self) -> Option<&str> {
        self.artist
            .as_deref()
            .or(self.uploader.as_deref())
            .or(self.channel.as_deref())
    }

    /// Headers yt-dlp itself would send (User-Agent, Referer), without cookies: safe for
    /// any host, e.g. the thumbnail CDN.
    pub fn headers(&self) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in self.http_headers.iter().flatten() {
            if let (Ok(k), Ok(v)) = (HeaderName::try_from(k.as_str()), HeaderValue::from_str(v)) {
                map.insert(k, v);
            }
        }
        // Byte ranges must refer to the raw file, never to a gzip-encoded body.
        map.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        map
    }

    /// Headers for the media URL itself. yt-dlp computes `cookies` from its jar for exactly
    /// that URL, so they are only ever sent to the host they were scoped to.
    pub fn media_headers(&self) -> HeaderMap {
        let mut map = self.headers();
        if let Some(cookie) = self.cookies.as_deref().and_then(cookie_header)
            && let Ok(v) = HeaderValue::from_str(&cookie)
        {
            map.insert(COOKIE, v);
        }
        map
    }
}

/// Turn yt-dlp's Set-Cookie style field into a `Cookie:` header value.
fn cookie_header(raw: &str) -> Option<String> {
    const ATTRS: [&str; 9] = [
        "domain", "path", "expires", "max-age", "secure", "httponly", "samesite", "version", "comment",
    ];
    let pairs: Vec<&str> = raw
        .split(';')
        .map(str::trim)
        .filter(|part| match part.split_once('=') {
            Some((name, _)) => !ATTRS.contains(&name.trim().to_ascii_lowercase().as_str()),
            None => false,
        })
        .collect();
    (!pairs.is_empty()).then(|| pairs.join("; "))
}

pub struct Ytdlp {
    tools: Tools,
    /// Flags shared by every call: format selector, playlist policy, cookies, JS runtime.
    base: Vec<String>,
}

impl Ytdlp {
    pub fn new(
        tools: Tools,
        selector: &str,
        playlist: bool,
        limit: Option<usize>,
        browser: Option<&str>,
    ) -> Self {
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
        // An app's own ffmpeg isn't on PATH; yt-dlp needs it for HLS/DASH downloads.
        if tools.ffmpeg.is_absolute() {
            base.extend([
                "--ffmpeg-location".into(),
                tools.ffmpeg.to_string_lossy().into_owned(),
            ]);
        }
        Self { tools, base }
    }

    /// One JSON document for a video, a playlist (entries flat when `flat`), or a search.
    pub async fn probe(&self, target: &str, flat: bool) -> Result<Info> {
        let target = tiktok_photo_as_video(target).unwrap_or_else(|| target.to_owned());
        match self.probe_once(&target, flat).await {
            // A short link (vt.tiktok.com) shows it is a photo post only once yt-dlp follows it.
            // If the /video/ form fails too, report the error about the link the user gave.
            Err(e) => match unsupported_tiktok_photo(&e.to_string()) {
                Some(video) => self.probe_once(&video, flat).await.map_err(|_| e),
                None => Err(e),
            },
            ok => ok,
        }
    }

    async fn probe_once(&self, target: &str, flat: bool) -> Result<Info> {
        let mut cmd = self.command();
        cmd.arg("-J");
        if flat {
            cmd.arg("--flat-playlist");
        }
        cmd.arg("--").arg(target);
        let out = cmd.output().await.context("không chạy được yt-dlp")?;
        if !out.status.success() {
            bail!(error_line(&out.stderr));
        }
        serde_json::from_slice(&out.stdout).context("yt-dlp trả JSON không đọc được")
    }

    /// Fallback when direct download is refused: let yt-dlp fetch the stream itself.
    pub async fn download(&self, page_url: &str, out: &Path, fragments: usize) -> Result<()> {
        let mut cmd = self.command();
        // Back off between its 10 retries (1, 2, 4, 8, 8… s, for whole-file and HLS/DASH
        // fragment requests): without it a DNS blip of a few seconds exhausts every retry at once.
        cmd.args([
            "-q",
            "--no-progress",
            "--force-overwrites",
            "--retry-sleep",
            "http:exp=1:8",
            "--retry-sleep",
            "fragment:exp=1:8",
            "-N",
        ])
        .arg(fragments.to_string())
        .arg("-o")
        .arg(out)
        .arg("--")
        .arg(page_url);
        let out = cmd.output().await.context("không chạy được yt-dlp")?;
        if !out.status.success() {
            bail!(error_line(&out.stderr));
        }
        Ok(())
    }

    fn command(&self) -> Command {
        let mut cmd = self.tools.ytdlp_command();
        cmd.args(&self.base).stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd
    }
}

fn error_line(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let line = text
        .lines()
        .rev()
        .find(|l| l.starts_with("ERROR:"))
        .or_else(|| text.lines().rev().find(|l| !l.trim().is_empty()))
        .unwrap_or("yt-dlp lỗi không rõ");
    line.trim_start_matches("ERROR:").trim().to_owned()
}

/// yt-dlp has no extractor for TikTok photo posts (`/@user/photo/<id>`), but the same id under
/// `/video/` resolves to the post's music as an audio-only format.
fn tiktok_photo_as_video(link: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(link).ok()?;
    let host = url.host_str()?;
    if host != "tiktok.com" && !host.ends_with(".tiktok.com") {
        return None;
    }
    let segments: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    let [user, "photo", id] = segments[..] else {
        return None;
    };
    if !user.starts_with('@') {
        return None;
    }
    let path = format!("/{user}/video/{id}");
    url.set_path(&path);
    Some(url.into())
}

/// The `/video/` form of the photo post yt-dlp refused, from its `Unsupported URL: <url>` error.
fn unsupported_tiktok_photo(error: &str) -> Option<String> {
    tiktok_photo_as_video(error.strip_prefix("Unsupported URL:")?.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_header_drops_attributes() {
        let raw = "tt_chain_token=abc; Domain=.tiktok.com; Path=/; Secure; Expires=1700000000; \
                   ttwid=x=y; Domain=.tiktok.com; Path=/";
        assert_eq!(
            cookie_header(raw).as_deref(),
            Some("tt_chain_token=abc; ttwid=x=y")
        );
        assert_eq!(cookie_header("Secure; Path=/"), None);
    }

    #[test]
    fn parses_flat_playlist() {
        let json = r#"{"_type":"playlist","title":"p","entries":[
            {"_type":"url","url":"https://www.youtube.com/watch?v=a","title":"A","duration":272.4,"extractor_key":"Youtube"}, null]}"#;
        let info: Info = serde_json::from_str(json).unwrap();
        assert!(info.is_playlist());
        let entries = info.entries.unwrap();
        assert!(entries[0].as_ref().unwrap().is_reference());
        assert_eq!(entries[0].as_ref().unwrap().duration, Some(272.4));
        assert_eq!(
            entries[0].as_ref().unwrap().extractor_key.as_deref(),
            Some("Youtube")
        );
        assert!(entries[1].is_none());
    }

    #[test]
    fn tiktok_photo_posts_become_video_links() {
        assert_eq!(
            tiktok_photo_as_video("https://www.tiktok.com/@lyricx369/photo/7697286291569790216?_r=1")
                .as_deref(),
            Some("https://www.tiktok.com/@lyricx369/video/7697286291569790216?_r=1")
        );
        assert_eq!(
            tiktok_photo_as_video("https://tiktok.com/@a/photo/1").as_deref(),
            Some("https://tiktok.com/@a/video/1")
        );
        assert_eq!(
            tiktok_photo_as_video("https://m.tiktok.com/@a/photo/1/").as_deref(),
            Some("https://m.tiktok.com/@a/video/1")
        );
        for other in [
            "https://www.tiktok.com/@a/video/1",
            "https://www.tiktok.com/tag/photo/1",
            "https://vt.tiktok.com/ZSgphjbt8/",
            "https://example.com/@a/photo/1",
            "https://nottiktok.com/@a/photo/1",
            "lofi chill",
        ] {
            assert_eq!(tiktok_photo_as_video(other), None, "{other}");
        }
        assert_eq!(
            unsupported_tiktok_photo("Unsupported URL: https://www.tiktok.com/@a/photo/1?x=y").as_deref(),
            Some("https://www.tiktok.com/@a/video/1?x=y")
        );
        assert_eq!(
            unsupported_tiktok_photo("Unsupported URL: https://example.com/a"),
            None
        );
        assert_eq!(unsupported_tiktok_photo("Video unavailable"), None);
    }

    #[test]
    fn direct_url_only_for_plain_http() {
        let mut info: Info = serde_json::from_str(r#"{"url":"https://x/a","protocol":"https"}"#).unwrap();
        assert_eq!(info.direct_url(), Some("https://x/a"));
        info.protocol = Some("m3u8_native".into());
        assert_eq!(info.direct_url(), None);
    }
}
