//! The per-link pipeline: probe with yt-dlp, expand playlists, skip what exists, download,
//! convert, move into place. It reports what happens as `Event`s and draws nothing itself.
//!
//! The stages are public so the app can stop between them (`resolve` into its tray, `fetch` the
//! audio to listen and cut, `save` from that audio); `process` chains them for the CLI.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use futures_util::future::join_all;
use serde::Serialize;
use tempfile::TempDir;
use tokio::sync::Semaphore;

use crate::{
    cut::{Cut, normalize},
    download::{Downloader, Progress},
    names::{fit, is_network_error, path_key, sanitize},
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

/// One track as yt-dlp described it: what the app shows in its tray, and what `fetch` and
/// `save` need. A flat playlist entry only knows its page until `probe_track` reads it.
/// Not `Debug`: the yt-dlp JSON inside carries cookies.
#[derive(Clone)]
pub struct Track {
    pub title: String,
    pub artist: Option<String>,
    pub thumbnail: Option<String>,
    /// Seconds, when the extractor knows it.
    pub duration: Option<f64>,
    pub video_id: Option<String>,
    /// A YouTube video (the only kind SponsorBlock knows).
    pub youtube: bool,
    info: Arc<Info>,
}

impl Track {
    fn new(info: Info) -> Self {
        // A flat entry goes by its page until probed, as its `Probing` label always did.
        let title = if info.is_reference() {
            info.title
                .clone()
                .or_else(|| info.url.clone())
                .unwrap_or_else(|| "?".into())
        } else {
            let title = info.title.clone().or_else(|| info.id.clone());
            title.unwrap_or_else(|| "untitled".into())
        };
        Self {
            title,
            artist: info.display_artist().map(str::to_owned),
            thumbnail: info.thumbnail.clone(),
            duration: info.duration,
            video_id: info.id.clone(),
            youtube: info.extractor_key.as_deref() == Some("Youtube"),
            info: Arc::new(info),
        }
    }

    /// A flat playlist entry: `probe_track` it before `fetch` (`save` probes it itself).
    pub fn needs_probe(&self) -> bool {
        self.info.is_reference()
    }

    /// yt-dlp's codec name for the picked audio stream ("opus", "mp4a.40.2"…), or "".
    pub fn codec(&self) -> &str {
        self.info.acodec.as_deref().unwrap_or("")
    }

    /// A probed track with only a title, for tests of code that stores tracks.
    #[cfg(any(test, feature = "test-util"))]
    pub fn for_tests(title: &str) -> Self {
        let info = serde_json::from_value(serde_json::json!({ "title": title })).expect("Info from a title");
        Self::new(info)
    }
}

#[derive(Clone)]
pub enum Resolved {
    Track(Track),
    /// `title` is already the name of the playlist's folder inside the output folder.
    Playlist {
        title: String,
        tracks: Vec<Track>,
    },
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

    /// The programs this engine runs, for the preview copy and peaks of a fetched track.
    pub fn tools(&self) -> &Tools {
        &self.tools
    }

    /// A link, playlist or search query → one or more tracks. Dropping the future (or aborting
    /// its task) kills every yt-dlp/ffmpeg it started, playlist tracks included. Cancelling
    /// sends no event, and the tracks give back their claims only once the future is dropped:
    /// after aborting a task, await its `JoinHandle` before starting the same link again, or it
    /// is taken for a duplicate.
    pub async fn process(self: Arc<Self>, id: Id, input: String) -> Vec<Outcome> {
        // A single track keeps the permit from its probe to its file, as it always did: with
        // `jobs` links at work the next one waits rather than probing ahead of their downloads.
        let permit = self.sem.acquire().await.expect("semaphore never closed");
        let (title, tracks) = match self.resolve_now(id, &input).await {
            Err(e) => return vec![self.fail(id, None, &input, e)],
            Ok(Resolved::Track(track)) => {
                return vec![self.save_now(id, None, &track, None, None, None).await];
            }
            Ok(Resolved::Playlist { title, tracks }) => (title, tracks),
        };
        drop(permit);

        self.emit(Event::Playlist {
            id,
            title: title.clone(),
            count: tracks.len(),
        });
        // Futures of this task rather than spawned tasks: cancelling the link drops them (and
        // their yt-dlp/ffmpeg, and their claims) at once. The semaphore bounds the work.
        let (engine, title) = (&*self, title.as_str());
        let saves = tracks.iter().map(|track| {
            let child = engine.next_id();
            async move {
                let _permit = engine.sem.acquire().await.expect("semaphore never closed");
                engine
                    .save_now(child, Some(id), track, Some(title), None, None)
                    .await
            }
        });
        join_all(saves).await
    }

    /// A link or search query → one track or a playlist of (usually flat) tracks. Emits
    /// `Probing` and holds a job permit while yt-dlp reads; failures are the caller's to report.
    pub async fn resolve(&self, id: Id, input: &str) -> Result<Resolved> {
        let _permit = self.sem.acquire().await.expect("semaphore never closed");
        self.resolve_now(id, input).await
    }

    /// `resolve` inside a permit the caller holds.
    async fn resolve_now(&self, id: Id, input: &str) -> Result<Resolved> {
        let search = !(input.starts_with("http://") || input.starts_with("https://"));
        self.emit(Event::Probing {
            id,
            parent: None,
            label: input.to_owned(),
        });
        // Searches are not flattened: yt-dlp resolves the hit in the same call.
        let target = if search {
            format!("ytsearch1:{input}")
        } else {
            input.to_owned()
        };
        let info = self.ytdlp.probe(&target, !search).await?;
        if !info.is_playlist() {
            return Ok(Resolved::Track(Track::new(info)));
        }
        let mut entries = info.entries.into_iter().flatten().flatten();
        if search {
            // ytsearch1 is a one-entry playlist: the hit stands for the search.
            let hit = entries.next().ok_or_else(|| anyhow!("không tìm thấy bài nào"))?;
            return Ok(Resolved::Track(Track::new(hit)));
        }
        // Leave the tracks inside at least ~100 characters of path.
        let title = fit(
            &self.opts.out,
            &sanitize(info.title.as_deref().unwrap_or("playlist")),
            &"x".repeat(100),
        );
        Ok(Resolved::Playlist {
            title,
            tracks: entries.map(Track::new).collect(),
        })
    }

    /// Reads a flat playlist entry in full; any other track comes back as it is. Emits nothing
    /// and takes no job permit.
    pub async fn probe_track(&self, track: &Track) -> Result<Track> {
        if !track.needs_probe() {
            return Ok(track.clone());
        }
        let url = track
            .info
            .url
            .as_deref()
            .ok_or_else(|| anyhow!("playlist thiếu link bài"))?;
        let info = self.ytdlp.probe(url, true).await?;
        if info.is_playlist() {
            bail!("playlist lồng nhau, hãy dán trực tiếp link playlist con");
        }
        Ok(Track::new(info))
    }

    /// Downloads the track's audio stream (the one picked for the format in effect when the
    /// engine started) to `to`, emitting `Progress` or `Fallback` under `id`. The file only
    /// appears at `to` once complete: it is written to `<to>.part`, which is removed if the
    /// download fails or the future is dropped (its yt-dlp is killed with it).
    pub async fn fetch(&self, id: Id, track: &Track, to: &Path) -> Result<()> {
        ensure!(!track.needs_probe(), "bài trong playlist chưa được đọc");
        // yt-dlp writes `<its -o>.part` itself while downloading: remove that one too.
        let part = with_suffix(to, ".part");
        let scratch = Scratch([with_suffix(&part, ".part"), part]);
        let part = &scratch.0[1];
        self.fetch_audio(id, &track.info, part).await?;
        tokio::fs::rename(part, to).await?;
        Ok(())
    }

    /// Where an untrimmed `save` of `track` would put it, without claiming anything. The
    /// session may still choose another name (`[id]` added) when two videos share a title, and
    /// a flat entry's title is only final once `probe_track` has read it.
    pub fn whole_destination(&self, track: &Track, playlist: Option<&str>) -> PathBuf {
        let dir = self.dir(playlist);
        named(
            &dir,
            &sanitize(&track.title),
            &format!(".{}", self.opts.format.ext()),
        )
    }

    /// Encodes `track` into the output folder (inside the `playlist` folder if given) with the
    /// engine's settings, emitting `Track` … `Done`/`Skipped`/`Failed` under `id`. Takes a job
    /// permit, and probes a flat entry first.
    ///
    /// `audio`: the stream already fetched by `fetch`; it is read, never moved or deleted, and
    /// nothing is downloaded for it. Without it the audio is fetched into the engine's temp dir.
    ///
    /// `cut`: keep only `cut.keep`, re-encoded, as `<title> (<label>).<ext>` (then
    /// `(<label> 2)`…). Such a copy never replaces a file and is never skipped, whether or not
    /// this video was already saved. Without it the whole track is saved, skipped when the
    /// video already ran in this session or its file exists (unless `force`).
    pub async fn save(
        &self,
        id: Id,
        parent: Option<Id>,
        track: &Track,
        playlist: Option<&str>,
        audio: Option<&Path>,
        cut: Option<&Cut>,
    ) -> Outcome {
        let _permit = self.sem.acquire().await.expect("semaphore never closed");
        self.save_now(id, parent, track, playlist, audio, cut).await
    }

    /// `save` inside a permit the caller holds.
    async fn save_now(
        &self,
        id: Id,
        parent: Option<Id>,
        track: &Track,
        playlist: Option<&str>,
        audio: Option<&Path>,
        cut: Option<&Cut>,
    ) -> Outcome {
        let probed;
        let track = if track.needs_probe() {
            if track.info.url.is_some() {
                self.emit(Event::Probing {
                    id,
                    parent,
                    label: track.title.clone(),
                });
            }
            match self.probe_track(track).await {
                Ok(t) => {
                    probed = t;
                    &probed
                }
                Err(e) => return self.fail(id, parent, &track.title, e),
            }
        } else {
            track
        };
        let dir = self.dir(playlist);
        match self.try_save(id, parent, track, &dir, audio, cut).await {
            Ok(o) => o,
            Err(e) => self.fail(id, parent, &track.title, e),
        }
    }

    async fn try_save(
        &self,
        id: Id,
        parent: Option<Id>,
        track: &Track,
        dir: &Path,
        audio: Option<&Path>,
        cut: Option<&Cut>,
    ) -> Result<Outcome> {
        let started = Instant::now();
        let (info, title) = (&*track.info, track.title.as_str());
        let name = sanitize(title);
        let ext = self.opts.format.ext();
        let keep = match cut {
            Some(cut) => {
                let keep = normalize(&cut.keep);
                ensure!(!keep.is_empty(), "không còn đoạn nào để lưu");
                Some(keep)
            }
            None => None,
        };
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
        let dest = if let Some(cut) = cut {
            self.trimmed_destination(&mut claims, dir, &name, &cut.label)
                .await
                .ok_or_else(|| anyhow!("đã có quá nhiều bản cắt của bài này"))?
        } else {
            // Same video reached twice (plain link + its &list= link, or two playlists).
            if let Some(vid) = info.id.as_deref()
                && !claims.take(format!("id:{vid}"))
            {
                return skip(Skip::Duplicate);
            }
            let mut dest = named(dir, &name, &format!(".{ext}"));
            if !claims.take(path_key(&dest)) {
                // Same title twice in one run (e.g. two "original sound" TikToks): add the id.
                let vid = sanitize(info.id.as_deref().unwrap_or("2"));
                dest = named(dir, &name, &format!(" [{vid}].{ext}"));
                if !claims.take(path_key(&dest)) {
                    return skip(Skip::Duplicate);
                }
            }
            if !self.opts.force && tokio::fs::try_exists(&dest).await.unwrap_or(false) {
                // Keep the name taken: another video with this title must not land on it either.
                claims.keep = true;
                return skip(Skip::Exists(dest));
            }
            dest
        };
        tokio::fs::create_dir_all(dir).await?;
        self.emit(Event::Track {
            id,
            parent,
            title: title.to_owned(),
            artist: track.artist.clone(),
            thumbnail: track.thumbnail.clone(),
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
        let [fetched, cover, encoded] = &scratch.0;

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
        let fetch_audio = async {
            match audio {
                Some(cached) => Ok(cached),
                None => self
                    .fetch_audio(id, info, fetched)
                    .await
                    .map(|()| fetched.as_path()),
            }
        };
        let (audio_res, has_cover) = tokio::join!(fetch_audio, fetch_cover);
        let input = audio_res?;

        self.emit(Event::Converting { id });
        let job = |with_cover: bool| transcode::Job {
            input,
            cover: with_cover.then_some(cover.as_path()),
            output: encoded,
            format: self.opts.format,
            src_codec: track.codec(),
            bitrate: self.opts.bitrate,
            tags: Tags {
                title,
                artist: track.artist.as_deref(),
                album: info.album.as_deref(),
                comment: info.webpage_url.as_deref(),
            },
            keep: keep.as_deref(),
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

    /// `<name> (<label>).<ext>`, else `(<label> 2)` … `(<label> 99)`: the first name neither
    /// on disk nor taken in this session, claimed. None when all are.
    async fn trimmed_destination(
        &self,
        claims: &mut Claims<'_>,
        dir: &Path,
        name: &str,
        label: &str,
    ) -> Option<PathBuf> {
        let (label, ext) = (sanitize(label), self.opts.format.ext());
        for n in 1..=99 {
            let tag = match n {
                1 => label.clone(),
                n => format!("{label} {n}"),
            };
            let dest = named(dir, name, &format!(" ({tag}).{ext}"));
            // Unknown counts as taken: a trimmed copy must never replace a file.
            if !tokio::fs::try_exists(&dest).await.unwrap_or(true) && claims.take(path_key(&dest)) {
                return Some(dest);
            }
        }
        None
    }

    /// The output folder, or the playlist's folder inside it.
    fn dir(&self, playlist: Option<&str>) -> PathBuf {
        match playlist {
            Some(name) => self.opts.out.join(name),
            None => self.opts.out.clone(),
        }
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

/// `dir/<name fitted to the path limit><suffix>`.
fn named(dir: &Path, name: &str, suffix: &str) -> PathBuf {
    dir.join(format!("{}{suffix}", fit(dir, name, suffix)))
}

/// `path` with `suffix` appended to its file name (`a.audio` → `a.audio.part`).
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    s.into()
}

/// Temp files (a track's audio, cover and encoded file; a fetch's partial download), removed
/// on drop whether the work finished, failed or was cancelled, so a long-lived engine doesn't
/// pile them up.
struct Scratch<const N: usize>([PathBuf; N]);

impl<const N: usize> Drop for Scratch<N> {
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
