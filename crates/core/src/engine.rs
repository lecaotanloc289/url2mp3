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
use serde::Serialize;
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

/// Serialized for the apps as `{"kind": "done", "id": 3, …}` with camelCase fields.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Event {
    /// yt-dlp is reading a link: the user's input, or a playlist entry.
    Probing {
        id: Id,
        parent: Option<Id>,
        label: String,
    },
    /// `id` is a playlist; its tracks follow with `parent: Some(id)`.
    Playlist {
        id: Id,
        title: String,
        count: usize,
    },
    /// The download starts.
    Track {
        id: Id,
        parent: Option<Id>,
        title: String,
        artist: Option<String>,
        thumbnail: Option<String>,
    },
    /// Bytes downloaded so far, at most every 100 ms per track.
    Progress {
        id: Id,
        done: u64,
        total: u64,
    },
    /// yt-dlp downloads the stream itself. `reason`: why our direct download failed, if tried.
    Fallback {
        id: Id,
        reason: Option<String>,
    },
    Converting {
        id: Id,
    },
    Done {
        id: Id,
        path: PathBuf,
        bytes: u64,
        secs: f64,
    },
    Skipped {
        id: Id,
        parent: Option<Id>,
        title: String,
        reason: Skip,
    },
    Failed {
        id: Id,
        parent: Option<Id>,
        label: String,
        message: String,
        /// DNS/connection trouble on the user's side, not a YouTube change.
        network: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", content = "path", rename_all = "camelCase")]
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
    /// Names temp files; separate from `Id`, which callers may reuse.
    scratch: AtomicU64,
    /// Video ids and destinations taken in this session, so two tracks with the same title
    /// don't overwrite each other and one video given twice is fetched once.
    claimed: Mutex<HashSet<String>>,
    sink: Box<dyn Fn(Event) + Send + Sync>,
    opts: Options,
}

impl Engine {
    /// Creates the output folder and a temp folder inside it (removed when the engine is
    /// dropped), after clearing temp folders left behind by a killed run. `sink` is called from
    /// the download tasks and must return quickly: queue the event, don't block on it.
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
        let tmp = tempfile::Builder::new()
            .prefix(".url2mp3-")
            .tempdir_in(&opts.out)?;
        Ok(Arc::new(Self {
            ytdlp: Ytdlp::new(
                tools.clone(),
                opts.format.selector(),
                opts.playlist,
                opts.limit,
                opts.cookies_from_browser.as_deref(),
            ),
            dl: Downloader::new(opts.connections.max(1))?,
            tools,
            tmp,
            sem: Semaphore::new(opts.jobs.max(1)),
            next: AtomicU64::new(0),
            scratch: AtomicU64::new(0),
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
    /// its task) kills every yt-dlp/ffmpeg it started. Cancelling sends no event, and the
    /// cancelled tracks give back their claims only once the task is gone: await the aborted
    /// `JoinHandle` before starting the same link again, or it is taken for a duplicate.
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
            // Keep the name taken: another video with this title must not land on it either.
            claims.keep = true;
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
        // Another url2mp3 may have swept our temp dir as stale while this engine sat idle.
        tokio::fs::create_dir_all(tmp).await?;
        let n = self.scratch.fetch_add(1, Relaxed);
        let scratch = Scratch([
            tmp.join(format!("{n}.audio")),
            tmp.join(format!("{n}.cover")),
            tmp.join(format!("{n}.{ext}")),
        ]);
        let [audio, cover, encoded] = &scratch.0;

        let thumb = info
            .thumbnail
            .as_deref()
            .filter(|_| self.opts.cover && self.opts.format.supports_cover());
        let fetch_cover = async {
            match thumb {
                Some(url) => self.dl.fetch_small(url, &info.headers(), cover).await.is_ok(),
                None => false,
            }
        };
        let (audio_res, has_cover) = tokio::join!(self.fetch_audio(id, info, audio), fetch_cover);
        audio_res?;

        self.emit(Event::Converting { id });
        let job = |with_cover: bool| transcode::Job {
            input: audio,
            cover: with_cover.then_some(cover.as_path()),
            output: encoded,
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
        tokio::fs::rename(encoded, &dest).await?;
        drop(scratch);

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
        let fresh = self
            .engine
            .claimed
            .lock()
            .expect("claimed lock")
            .insert(key.clone());
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

/// One track's temp files (audio, cover, encoded), removed on drop whether the track
/// finished, failed or was cancelled, so a long-lived engine doesn't pile them up.
struct Scratch([PathBuf; 3]);

impl Drop for Scratch {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_file(path);
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
        {
            let mut last = self.last.lock().expect("progress lock");
            if !force && last.is_some_and(|t| t.elapsed() < PROGRESS_EVERY) {
                return;
            }
            *last = Some(Instant::now());
        }
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

/// Remove `.url2mp3-*` temp dirs left by a run that was killed or crashed. Only ones untouched
/// for an hour (the dir and every file in it, which a running download keeps writing), so
/// another url2mp3 working right now keeps its own.
async fn sweep_stale_temp(out: &Path) {
    let Ok(mut dir) = tokio::fs::read_dir(out).await else {
        return;
    };
    while let Ok(Some(entry)) = dir.next_entry().await {
        let is_dir = entry.file_type().await.is_ok_and(|t| t.is_dir());
        if is_dir
            && entry.file_name().to_string_lossy().starts_with(".url2mp3-")
            && idle_for(&entry.path()).await > Duration::from_secs(3600)
        {
            let _ = tokio::fs::remove_dir_all(entry.path()).await;
        }
    }
}

/// Time since anything in `dir` (the dir itself or a file directly in it) was modified.
async fn idle_for(dir: &Path) -> Duration {
    async fn age(path: &Path) -> Duration {
        let modified = tokio::fs::metadata(path).await.and_then(|m| m.modified());
        modified.ok().and_then(|t| t.elapsed().ok()).unwrap_or_default()
    }
    let mut idle = age(dir).await;
    if let Ok(mut files) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(file)) = files.next_entry().await {
            idle = idle.min(age(&file.path()).await);
        }
    }
    idle
}
