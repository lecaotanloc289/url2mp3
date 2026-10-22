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
        Self { tools, base }
    }

    /// One JSON document for a video, a playlist (entries flat when `flat`), or a search.
    pub async fn probe(&self, target: &str, flat: bool) -> Result<Info> {
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
            {"_type":"url","url":"https://www.youtube.com/watch?v=a","title":"A"}, null]}"#;
        let info: Info = serde_json::from_str(json).unwrap();
        assert!(info.is_playlist());
        let entries = info.entries.unwrap();
        assert!(entries[0].as_ref().unwrap().is_reference());
        assert!(entries[1].is_none());
    }

    #[test]
    fn direct_url_only_for_plain_http() {
        let mut info: Info = serde_json::from_str(r#"{"url":"https://x/a","protocol":"https"}"#).unwrap();
        assert_eq!(info.direct_url(), Some("https://x/a"));
        info.protocol = Some("m3u8_native".into());
        assert_eq!(info.direct_url(), None);
    }
}
