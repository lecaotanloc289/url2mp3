//! Step 4: parallel HTTP range download. The file is split into byte ranges fetched over
//! N connections at once, each written straight to its offset in a pre-sized file.

use std::{fs::File, io, path::Path, sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use futures_util::{StreamExt, TryStreamExt, stream};
use reqwest::{
    Client, Response, StatusCode,
    header::{CONTENT_RANGE, CONTENT_TYPE, HeaderMap, RANGE},
};

const MIN_CHUNK: u64 = 256 * 1024;
/// googlevideo throttles single requests above ~10 MB; stay under it.
const MAX_CHUNK: u64 = 8 * 1024 * 1024;
/// Size of the first request when the total size is not known up front.
const PROBE_CHUNK: u64 = 1024 * 1024;
/// Write batch for servers that ignore Range and stream the whole body.
const FLUSH: usize = 4 * 1024 * 1024;
/// Consecutive failures without progress before a range gives up.
const RETRIES: u32 = 3;
/// Hard cap on requests per range, even when each one makes a little progress.
const MAX_REQUESTS: u32 = 32;

/// Byte progress of one download.
pub trait Progress: Sync {
    fn set_length(&self, total: u64);
    fn inc(&self, n: u64);
}

pub struct Downloader {
    client: Client,
    pub connections: usize,
}

enum Failure {
    /// Network hiccup or 5xx/429: resume the range from where it stopped.
    Retry(anyhow::Error),
    /// 403/404, wrong status or wrong slice: give up so the caller can fall back to yt-dlp.
    Fatal(anyhow::Error),
}

/// The file being downloaded: what every range request needs.
struct Target<'a> {
    url: &'a str,
    headers: &'a HeaderMap,
    total: u64,
    file: &'a Arc<File>,
    pb: &'a dyn Progress,
}

impl Downloader {
    pub fn new(connections: usize) -> Result<Self> {
        let client = crate::http_client()
            .http1_only()
            .tcp_nodelay(true)
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(connections)
            .build()?;
        Ok(Self { client, connections })
    }

    pub async fn fetch(
        &self,
        url: &str,
        headers: &HeaderMap,
        size_hint: Option<u64>,
        path: &Path,
        pb: &dyn Progress,
    ) -> Result<()> {
        let file = Arc::new(tokio::fs::File::create(path).await?.into_std().await);
        let (total, mut start) = match size_hint {
            Some(n) => (n, 0),
            None => self.probe(url, headers, &file, pb).await?,
        };
        if start >= total {
            return Ok(());
        }
        pb.set_length(total);
        let f = file.clone();
        tokio::task::spawn_blocking(move || f.set_len(total)).await??;

        let target = Target {
            url,
            headers,
            total,
            file: &file,
            pb,
        };
        let chunk = (total - start)
            .div_ceil(self.connections as u64)
            .clamp(MIN_CHUNK, MAX_CHUNK);
        let mut ranges = Vec::new();
        while start < total {
            let end = (start + chunk).min(total);
            ranges.push((start, end));
            start = end;
        }
        stream::iter(ranges)
            .map(|(s, e)| self.fetch_range(&target, s, e))
            .buffer_unordered(self.connections)
            .try_collect()
            .await
    }

    /// Small whole-body GET (cover art). Rejects empty bodies and HTML error pages.
    pub async fn fetch_small(&self, url: &str, headers: &HeaderMap, path: &Path) -> Result<()> {
        let resp = self
            .client
            .get(url)
            .headers(headers.clone())
            .send()
            .await
            .map_err(short)?
            .error_for_status()
            .map_err(short)?;
        let content_type = resp.headers().get(CONTENT_TYPE).and_then(|v| v.to_str().ok());
        if content_type.is_some_and(|t| t.starts_with("text/")) {
            bail!("không phải ảnh");
        }
        let body = resp.bytes().await.map_err(short)?;
        if body.is_empty() {
            bail!("ảnh rỗng");
        }
        tokio::fs::write(path, body).await?;
        Ok(())
    }

    /// Unknown size: fetch the first chunk alone and learn the total from Content-Range.
    /// Returns (total, bytes already written from offset 0).
    async fn probe(
        &self,
        url: &str,
        headers: &HeaderMap,
        file: &Arc<File>,
        pb: &dyn Progress,
    ) -> Result<(u64, u64)> {
        let mut resp = self
            .client
            .get(url)
            .headers(headers.clone())
            .header(RANGE, format!("bytes=0-{}", PROBE_CHUNK - 1))
            .send()
            .await
            .map_err(short)?;
        match resp.status() {
            StatusCode::PARTIAL_CONTENT => {
                let (first, _, total) = content_range(&resp).context("server không trả Content-Range")?;
                if first != 0 {
                    bail!("server trả sai đoạn (bắt đầu {first}, cần 0)");
                }
                pb.set_length(total);
                let mut buf = Vec::with_capacity(PROBE_CHUNK.min(total) as usize);
                while let Some(piece) = resp.chunk().await.map_err(short)? {
                    if (buf.len() + piece.len()) as u64 > PROBE_CHUNK.min(total) {
                        bail!("server trả thừa dữ liệu");
                    }
                    pb.inc(piece.len() as u64);
                    buf.extend_from_slice(&piece);
                }
                let got = buf.len() as u64;
                write_at(file, buf, 0).await?;
                Ok((total, got))
            }
            // Server ignored Range: plain single-stream download, written in batches.
            StatusCode::OK => {
                let expected = resp.content_length();
                if let Some(n) = expected {
                    pb.set_length(n);
                }
                let (mut buf, mut written) = (Vec::with_capacity(FLUSH), 0u64);
                while let Some(piece) = resp.chunk().await.map_err(short)? {
                    pb.inc(piece.len() as u64);
                    buf.extend_from_slice(&piece);
                    if buf.len() >= FLUSH {
                        let batch = std::mem::replace(&mut buf, Vec::with_capacity(FLUSH));
                        let n = batch.len() as u64;
                        write_at(file, batch, written).await?;
                        written += n;
                    }
                }
                let n = buf.len() as u64;
                write_at(file, buf, written).await?;
                written += n;
                if expected.is_some_and(|e| e != written) {
                    bail!("tải thiếu dữ liệu ({written}/{} byte)", expected.unwrap_or(0));
                }
                Ok((written, written))
            }
            s => bail!("HTTP {s}"),
        }
    }

    /// Download [start, end) into memory (≤ MAX_CHUNK), resuming on transient errors, then
    /// write it at its offset.
    async fn fetch_range(&self, t: &Target<'_>, start: u64, end: u64) -> Result<()> {
        let want = (end - start) as usize;
        let mut buf = Vec::with_capacity(want);
        let (mut failures, mut requests) = (0, 0);
        while buf.len() < want {
            requests += 1;
            if requests > MAX_REQUESTS {
                bail!("đoạn {start}-{end} rớt kết nối quá nhiều lần");
            }
            let before = buf.len();
            match self
                .read_range(t, start + before as u64, end, &mut buf, want)
                .await
            {
                Ok(()) => failures = 0,
                Err(Failure::Retry(e)) => {
                    failures = if buf.len() > before { 1 } else { failures + 1 };
                    if failures > RETRIES {
                        return Err(e);
                    }
                    tokio::time::sleep(Duration::from_millis(250 << failures)).await;
                }
                Err(Failure::Fatal(e)) => return Err(e),
            }
        }
        write_at(t.file, buf, start).await
    }

    async fn read_range(
        &self,
        t: &Target<'_>,
        from: u64,
        end: u64,
        buf: &mut Vec<u8>,
        want: usize,
    ) -> Result<(), Failure> {
        let mut resp = self
            .client
            .get(t.url)
            .headers(t.headers.clone())
            .header(RANGE, format!("bytes={from}-{}", end - 1))
            .send()
            .await
            .map_err(|e| Failure::Retry(short(e)))?;
        match resp.status() {
            StatusCode::PARTIAL_CONTENT => {}
            s if s == StatusCode::TOO_MANY_REQUESTS || s.is_server_error() => {
                return Err(Failure::Retry(anyhow!("HTTP {s}")));
            }
            StatusCode::OK => return Err(Failure::Fatal(anyhow!("server không hỗ trợ tải theo đoạn"))),
            s => return Err(Failure::Fatal(anyhow!("HTTP {s}"))),
        }
        // A cache serving another slice, or a file whose size differs from what yt-dlp
        // reported, would silently corrupt the output: refuse it.
        match content_range(&resp) {
            Some((first, last, total)) if first == from && last < end && total == t.total => {}
            got => {
                return Err(Failure::Fatal(anyhow!(
                    "server trả sai đoạn {got:?}, cần {from}-{}/{}",
                    end - 1,
                    t.total
                )));
            }
        }
        let before = buf.len();
        loop {
            match resp.chunk().await {
                Ok(Some(piece)) => {
                    if buf.len() + piece.len() > want {
                        return Err(Failure::Fatal(anyhow!("server trả thừa dữ liệu")));
                    }
                    t.pb.inc(piece.len() as u64);
                    buf.extend_from_slice(&piece);
                }
                Ok(None) if buf.len() < want && buf.len() == before => {
                    return Err(Failure::Retry(anyhow!("server trả đoạn rỗng")));
                }
                // Short body: the caller loops and resumes from the new offset.
                Ok(None) => return Ok(()),
                Err(e) => return Err(Failure::Retry(short(e))),
            }
        }
    }
}

/// reqwest errors embed the full signed media URL (~1.5 KB): drop it from messages.
fn short(e: reqwest::Error) -> anyhow::Error {
    e.without_url().into()
}

fn content_range(resp: &Response) -> Option<(u64, u64, u64)> {
    parse_content_range(resp.headers().get(CONTENT_RANGE)?.to_str().ok()?)
}

/// `bytes 0-1048575/4012345` → (0, 1048575, 4012345). Unknown total (`/*`) → None.
fn parse_content_range(value: &str) -> Option<(u64, u64, u64)> {
    let (range, total) = value.trim().strip_prefix("bytes ")?.split_once('/')?;
    let (first, last) = range.split_once('-')?;
    Some((
        first.trim().parse().ok()?,
        last.trim().parse().ok()?,
        total.trim().parse().ok()?,
    ))
}

async fn write_at(file: &Arc<File>, buf: Vec<u8>, offset: u64) -> Result<()> {
    let file = file.clone();
    tokio::task::spawn_blocking(move || write_all_at(&file, &buf, offset)).await??;
    Ok(())
}

#[cfg(unix)]
fn write_all_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(buf, offset)
}

#[cfg(windows)]
fn write_all_at(file: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = file.seek_write(buf, offset)?;
        buf = &buf[n..];
        offset += n as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Downloader, Progress, parse_content_range};
    use reqwest::header::HeaderMap;

    struct NoProgress;
    impl Progress for NoProgress {
        fn set_length(&self, _: u64) {}
        fn inc(&self, _: u64) {}
    }

    #[tokio::test]
    async fn dns_failure_message_has_no_media_url() {
        let dir = tempfile::tempdir().unwrap();
        let url = "https://url2mp3-test.invalid/videoplayback?sig=SECRET";
        let err = Downloader::new(2)
            .unwrap()
            .fetch(
                url,
                &HeaderMap::new(),
                Some(1000),
                &dir.path().join("a"),
                &NoProgress,
            )
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(!msg.contains("SECRET"), "{msg}");
        assert!(msg.contains("dns error"), "{msg}");
    }

    #[test]
    fn content_range_parsing() {
        assert_eq!(
            parse_content_range("bytes 0-1048575/4012345"),
            Some((0, 1048575, 4012345))
        );
        assert_eq!(parse_content_range("bytes 10-20/*"), None);
        assert_eq!(parse_content_range("items 0-1/2"), None);
    }
}
