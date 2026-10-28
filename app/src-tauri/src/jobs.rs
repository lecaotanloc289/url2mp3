//! Everything behind the UI's tray and track list: links are resolved into the tray (`add`),
//! a row the user opens has its audio fetched for listening (`prepare`), and rows are saved
//! with the settings in effect when Download is pressed (`save`).
//!
//! Tray ids come from one counter that outlives engines (a new one is built after settings
//! change or yt-dlp updates), and every engine call about a row uses its tray id as the engine
//! `Id`. Tray rows go to the webview as `tray` events, engine events as `engine` `{ event }`.

use std::{
    collections::{BTreeSet, HashMap},
    future::Future,
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, async_runtime::JoinHandle};
use url2mp3_core::{Cut, Engine, Event, Id, PEAK_BUCKETS, Resolved, Segment, Track, is_network_error};

use crate::{
    platform::{Platform, ToolsStatus},
    settings::{self, Settings},
    tray::{Entry, Tray},
};

/// What the trim screen needs once a row's audio is here.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prepared {
    /// The audio's measured length (yt-dlp's rounded one if nothing was decoded).
    duration: Option<f64>,
    peaks: Vec<f32>,
    /// SponsorBlock's non-music parts (YouTube only), never applied by themselves.
    suggestions: Vec<Segment>,
    /// The whole track is already in the output folder (current folder and format).
    exists: bool,
}

/// One row to save: whole, or only the kept ranges of `cut`.
#[derive(Deserialize)]
pub struct SaveItem {
    id: Id,
    cut: Option<Cut>,
}

/// A row for the tray, on the `tray` channel.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
enum TrayEvent<'a> {
    /// A track ready to download. Sent again with the full details once a playlist entry has
    /// been read (`prepare`). `input` is the user's text, for a track that isn't in a playlist.
    Added {
        id: Id,
        parent: Option<Id>,
        input: Option<&'a str>,
        title: &'a str,
        artist: Option<&'a str>,
        thumbnail: Option<&'a str>,
        duration: Option<f64>,
        youtube: bool,
    },
    /// `id` is a playlist; its `count` tracks follow as `added` with `parent: id`.
    Playlist {
        id: Id,
        input: &'a str,
        title: &'a str,
        count: usize,
    },
}

impl<'a> TrayEvent<'a> {
    fn added(id: Id, parent: Option<Id>, input: Option<&'a str>, track: &'a Track) -> Self {
        Self::Added {
            id,
            parent,
            input,
            title: &track.title,
            artist: track.artist.as_deref(),
            thumbnail: track.thumbnail.as_deref(),
            duration: track.duration,
            youtube: track.youtube,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Resolving a link into the tray.
    Add,
    /// Fetching a row's audio, preview copy and peaks.
    Prepare,
    /// Saving a row into the output folder, whole or cut.
    Save { whole: bool },
}

struct Task {
    id: Id,
    stage: Stage,
    handle: JoinHandle<()>,
}

/// The tray and the tasks working on it, under one lock so a row removed from the tray can't
/// be picked up by a task starting at the same moment.
struct State {
    tray: Tray,
    /// By task number: one row can have a save and a prepare (or two prepares) at once.
    running: HashMap<u64, Task>,
}

impl State {
    fn take(&mut self, pick: impl Fn(&Task) -> bool) -> Vec<Task> {
        let numbers: Vec<u64> = self
            .running
            .iter()
            .filter(|(_, task)| pick(task))
            .map(|(no, _)| *no)
            .collect();
        numbers.iter().filter_map(|no| self.running.remove(no)).collect()
    }
}

pub struct Jobs {
    app: AppHandle,
    client: reqwest::Client,
    platform: Platform,
    settings_path: PathBuf,
    settings: Mutex<Settings>,
    engine: tokio::sync::Mutex<Option<Arc<Engine>>>,
    /// Tray ids, unique across engines.
    next_id: AtomicU64,
    /// Task numbers, so a task that ends removes its own entry and no other.
    next_task: AtomicU64,
    state: Mutex<State>,
    /// Prepares fetching and decoding at once (rows opened one after another keep going in
    /// the background); separate from the engine's job permits, which bound saves.
    prepares: tokio::sync::Semaphore,
    /// One yt-dlp download at a time.
    updating: tokio::sync::Mutex<()>,
}

/// See `Jobs::prepares`.
const PREPARES: usize = 2;

impl Jobs {
    /// `cache_dir`: the app's cache folder. This run's tray caches audio in its own
    /// `tray/run-*/` inside it (another instance, possible on Windows, has its own), removed on
    /// shutdown; folders of killed runs are swept once idle for an hour.
    pub fn new(app: AppHandle, platform: Platform, config_dir: PathBuf, cache_dir: PathBuf) -> Self {
        let settings_path = config_dir.join("settings.json");
        let trays = cache_dir.join("tray");
        let run = run_dir(&trays);
        // Ours is brand new, so never idle long enough to go.
        tauri::async_runtime::spawn(async move {
            url2mp3_core::remove_stale_dirs(&trays, RUN_PREFIX).await;
        });
        Self {
            app,
            client: url2mp3_core::http_client()
                .user_agent(concat!("url2mp3/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("reqwest client"),
            platform,
            settings: Mutex::new(settings::load(&settings_path)),
            settings_path,
            engine: tokio::sync::Mutex::new(None),
            next_id: AtomicU64::new(1),
            next_task: AtomicU64::new(0),
            state: Mutex::new(State {
                tray: Tray::new(run),
                running: HashMap::new(),
            }),
            prepares: tokio::sync::Semaphore::new(PREPARES),
            updating: tokio::sync::Mutex::new(()),
        }
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub fn platform(&self) -> &Platform {
        &self.platform
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().expect("settings lock").clone()
    }

    /// Save, and build the next engine from the new settings. Rows in the tray keep their
    /// cached audio: the next save encodes it with these settings. A row without cached audio
    /// is read again for the new format when it is fetched or saved.
    pub async fn set_settings(&self, new: Settings) -> Result<()> {
        settings::save(&self.settings_path, &new)?;
        *self.settings.lock().expect("settings lock") = new;
        *self.engine.lock().await = None;
        Ok(())
    }

    pub fn tools_status(&self) -> ToolsStatus {
        self.platform.status()
    }

    /// Install yt-dlp if missing, or update it when `force` or a day has passed. Progress and
    /// errors also go to the UI as `tools` events.
    pub async fn update_ytdlp(&self, force: bool) -> Result<ToolsStatus> {
        let _one_at_a_time = self.updating.lock().await;
        if Platform::UPDATE_IN_PLACE && !self.state().running.is_empty() {
            // Rewriting yt-dlp under a running download would break it: the daily check waits
            // for a quiet moment, a click on Update is told to wait.
            if force {
                anyhow::bail!("ytdlp-busy");
            }
            return Ok(self.platform.status());
        }
        let downloading = |version: &str| {
            let _ = self.app.emit(
                "tools",
                ToolsStatus::Downloading {
                    version: version.to_owned(),
                },
            );
        };
        let result = self.platform.update(&self.client, force, downloading).await;
        let status = match &result {
            Ok(updated) => ToolsStatus::Ready {
                version: updated.version.clone(),
            },
            Err(e) => {
                log::error!("yt-dlp update: {e:#}");
                ToolsStatus::Error {
                    message: format!("{e:#}"),
                    version: self.platform.version(),
                }
            }
        };
        if result.as_ref().is_ok_and(|u| u.changed) {
            // New work uses the new yt-dlp; running tasks keep theirs.
            *self.engine.lock().await = None;
        }
        let _ = self.app.emit("tools", &status);
        result.map(|_| status)
    }

    /// Resolve each link or search phrase into the tray (`tray` events). Probing and failures
    /// follow on `engine` under the row's id.
    pub async fn add(self: &Arc<Self>, inputs: Vec<String>) -> Result<usize> {
        let _no_update = self.no_update().await;
        let engine = self.engine().await?;
        let inputs: Vec<String> = inputs
            .iter()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        let count = inputs.len();
        let mut state = self.state();
        for input in inputs {
            let id = self.next_id.fetch_add(1, Relaxed);
            let (jobs, engine) = (self.clone(), engine.clone());
            self.spawn(&mut state, id, Stage::Add, move |task| async move {
                let resolved = engine.resolve(id, &input).await;
                jobs.added(task, id, &input, resolved);
            });
        }
        Ok(count)
    }

    /// Puts what `add` resolved into the tray, unless the row was discarded meanwhile.
    fn added(&self, task: u64, id: Id, input: &str, resolved: Result<Resolved>) {
        let mut state = self.state();
        if !state.running.contains_key(&task) {
            return;
        }
        match resolved {
            Err(e) => {
                drop(state);
                let message = format!("{e:#}");
                let network = is_network_error(&message);
                let failed = Event::Failed {
                    id,
                    parent: None,
                    label: input.to_owned(),
                    message,
                    network,
                };
                forward(&self.app, failed);
            }
            Ok(Resolved::Track(track)) => {
                self.emit_tray(TrayEvent::added(id, None, Some(input), &track));
                state.tray.insert(id, Entry::new(track, None, None));
            }
            Ok(Resolved::Playlist { title, tracks }) => {
                self.emit_tray(TrayEvent::Playlist {
                    id,
                    input,
                    title: &title,
                    count: tracks.len(),
                });
                for track in tracks {
                    let child = self.next_id.fetch_add(1, Relaxed);
                    self.emit_tray(TrayEvent::added(child, Some(id), None, &track));
                    state
                        .tray
                        .insert(child, Entry::new(track, Some(id), Some(title.clone())));
                }
            }
        }
    }

    /// Fetch a row's audio (unless cached) for listening and cutting: its preview copy, peaks,
    /// suggestions and whether the whole track is already saved. Progress goes on `engine`;
    /// a failure only comes back here (the row stays in the tray and can still be saved).
    /// At most `PREPARES` rows are prepared at once; the others wait their turn.
    pub async fn prepare(self: &Arc<Self>, id: Id) -> Result<Prepared> {
        let done = {
            let _no_update = self.no_update().await;
            let engine = self.engine().await?;
            let mut state = self.state();
            let entry = state.tray.get(id).context("not-in-tray")?;
            let (tx, rx) = tokio::sync::oneshot::channel();
            let jobs = self.clone();
            self.spawn(&mut state, id, Stage::Prepare, move |_| async move {
                let _ = tx.send(jobs.prepare_now(&engine, id, entry).await);
            });
            rx
        };
        // Dropped unsent: the row was discarded (or the app is closing).
        done.await.unwrap_or_else(|_| Err(anyhow!("cancelled")))
    }

    async fn prepare_now(&self, engine: &Engine, id: Id, entry: Entry) -> Result<Prepared> {
        // Taken before the cache lock, so a prepare waiting for its turn never holds up a save.
        let _turn = self.prepares.acquire().await.context("cancelled")?;
        let (dir, (audio, preview)) = {
            let state = self.state();
            (state.tray.dir(id), state.tray.paths(id))
        };
        let track = {
            // Only while the audio is fetched: a second prepare or a save of this row waits
            // here rather than fetching again, but a save never waits for the preview copy,
            // peaks or SponsorBlock below (it only reads the audio, as they do).
            let _cache = entry.cache.lock().await;
            // Read again when the format changed since `add`, so the cache holds this format's
            // stream (audio already cached stays as it is).
            let stale = engine.picked_for_another_format(&entry.track) && !exists(&audio).await;
            let track = if entry.track.needs_probe() || stale {
                let probed = engine.reprobe(&entry.track).await?;
                let mut state = self.state();
                if state.tray.set_track(id, probed.clone()) {
                    self.emit_tray(TrayEvent::added(id, entry.parent, None, &probed));
                }
                probed
            } else {
                entry.track
            };
            if !exists(&audio).await {
                tokio::fs::create_dir_all(&dir).await?;
                engine.fetch(id, &track, &audio).await?;
            }
            track
        };

        let tools = engine.tools();
        let make_preview = async {
            let _one_writer = entry.preview.lock().await;
            if exists(&preview).await {
                return Ok(());
            }
            // Only a complete copy is ever at `preview`, so a later prepare can reuse it.
            let part = dir.join("preview.m4a.part");
            url2mp3_core::make_preview(tools, &audio, track.codec(), &part).await?;
            tokio::fs::rename(&part, &preview).await?;
            anyhow::Ok(())
        };
        let peaks = url2mp3_core::peaks(tools, &audio, PEAK_BUCKETS);
        let suggestions = async {
            match track.video_id.as_deref() {
                Some(video) if track.youtube => url2mp3_core::suggestions(&self.client, video).await,
                _ => Vec::new(),
            }
        };
        let whole = engine.whole_destination(&track, entry.playlist.as_deref());
        let (made, wave, suggestions, exists) =
            tokio::join!(make_preview, peaks, suggestions, exists(&whole));
        made?;
        let wave = wave?;
        Ok(Prepared {
            // What was decoded, not yt-dlp's rounded figure: cuts must be able to reach the end.
            duration: Some(wave.seconds).filter(|s| *s > 0.0).or(track.duration),
            peaks: wave.peaks,
            suggestions,
            exists,
        })
    }

    /// The preview copy `prepare` made for `id`.
    pub async fn preview_audio(&self, id: Id) -> Result<Vec<u8>> {
        let (_, preview) = self.state().tray.paths(id);
        tokio::fs::read(&preview).await.context("no-preview")
    }

    /// Save each row with the current settings, from its cached audio when `prepare` fetched
    /// it (the normal download otherwise). Events follow on `engine`; the row stays in the tray
    /// until discarded, so a failed one can be saved again. Saves of one row run one after the
    /// other (whole, then trimmed…); a whole save while one is running is ignored (it would
    /// only be skipped as a duplicate). The cache stays until the row is discarded: saving a
    /// trimmed copy after the whole track never downloads it again.
    pub async fn save(self: &Arc<Self>, items: Vec<SaveItem>) -> Result<()> {
        let _no_update = self.no_update().await;
        let engine = self.engine().await?;
        let mut state = self.state();
        for SaveItem { id, cut } in items {
            let Some(entry) = state.tray.get(id) else {
                log::warn!("save: {id} is not in the tray");
                continue;
            };
            let whole = cut.is_none();
            let saving_whole = |t: &Task| t.id == id && t.stage == Stage::Save { whole: true };
            if whole && state.running.values().any(saving_whole) {
                continue;
            }
            let (audio, _) = state.tray.paths(id);
            let (jobs, engine) = (self.clone(), engine.clone());
            self.spawn(&mut state, id, Stage::Save { whole }, move |_| async move {
                // Waits for a fetch `prepare` still runs (or an earlier save), then uses its audio.
                let _cache = entry.cache.lock().await;
                // Probed by `prepare` meanwhile, maybe.
                let track = jobs.state().tray.get(id).map_or(entry.track, |e| e.track);
                let cached = exists(&audio).await.then_some(audio.as_path());
                let playlist = entry.playlist.as_deref();
                engine
                    .save(id, entry.parent, &track, playlist, cached, cut.as_ref())
                    .await;
            });
        }
        Ok(())
    }

    /// Stop what works on a row (or on every track of a playlist row): resolving, fetching for
    /// the trim screen (its `prepare` fails with "cancelled"), saving. Tells the UI once it has
    /// stopped; the rows stay in the tray with their cache.
    pub async fn cancel(&self, id: Id) {
        let tasks = {
            let mut state = self.state();
            let mut ids = state.tray.children(id);
            ids.push(id);
            state.take(|t| ids.contains(&t.id))
        };
        // Already finished: nothing to cancel, and the UI already shows how it ended.
        if tasks.is_empty() {
            return;
        }
        let mut stopped: BTreeSet<Id> = tasks.iter().map(|t| t.id).collect();
        stopped.insert(id);
        // A track gives back its claims only once its task is gone (see Engine::process).
        stop(tasks).await;
        for id in stopped {
            let _ = self
                .app
                .emit("engine", json!({ "event": { "kind": "cancelled", "id": id } }));
        }
    }

    /// Remove rows (a playlist row with its tracks) from the tray: stop whatever works on them
    /// and delete their cached audio.
    pub async fn discard(&self, ids: Vec<Id>) {
        let (tasks, dirs) = {
            let mut state = self.state();
            let removed: Vec<Id> = ids.iter().flat_map(|id| state.tray.remove(*id)).collect();
            let dirs: Vec<PathBuf> = removed.iter().map(|id| state.tray.dir(*id)).collect();
            (state.take(|t| removed.contains(&t.id)), dirs)
        };
        // Their yt-dlp/ffmpeg are gone once the tasks are: nothing writes in the folders now.
        stop(tasks).await;
        for dir in dirs {
            let _ = tokio::fs::remove_dir_all(dir).await;
        }
    }

    /// Stop everything before the app exits, so no yt-dlp or ffmpeg outlives it, the engines
    /// remove their temp folders and this run's tray cache goes.
    pub async fn shutdown(&self) {
        let tasks = self.state().take(|_| true);
        stop(tasks).await;
        *self.engine.lock().await = None;
        let root = self.state().tray.root().to_owned();
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    /// Run `work(task number)` as a task registered in `state` (still locked by the caller, so
    /// a task that ends at once removes its entry after it was added).
    fn spawn<F, Fut>(self: &Arc<Self>, state: &mut State, id: Id, stage: Stage, work: F)
    where
        F: FnOnce(u64) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let no = self.next_task.fetch_add(1, Relaxed);
        let work = work(no);
        let jobs = self.clone();
        let handle = tauri::async_runtime::spawn(async move {
            work.await;
            jobs.state().running.remove(&no);
        });
        state.running.insert(no, Task { id, stage, handle });
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("state lock")
    }

    /// Android: an in-place update must finish before yt-dlp runs again; held while tasks are
    /// registered, it also keeps an update from starting under them.
    async fn no_update(&self) -> Option<tokio::sync::MutexGuard<'_, ()>> {
        if Platform::UPDATE_IN_PLACE {
            Some(self.updating.lock().await)
        } else {
            None
        }
    }

    fn emit_tray(&self, event: TrayEvent) {
        let _ = self.app.emit("tray", event);
    }

    /// The engine for the current settings and yt-dlp, built on first use.
    async fn engine(&self) -> Result<Arc<Engine>> {
        let mut slot = self.engine.lock().await;
        if let Some(engine) = &*slot {
            return Ok(engine.clone());
        }
        let tools = self.platform.tools().await?;
        let options = self.settings().options()?;
        let app = self.app.clone();
        let engine = Engine::start(tools, options, move |event| forward(&app, event)).await?;
        *slot = Some(engine.clone());
        Ok(engine)
    }
}

const RUN_PREFIX: &str = "run-";

/// A fresh `<trays>/run-*` folder for this run's tray. Should the cache folder be unusable,
/// a per-process name: `prepare` then fails creating it, and saves download as usual.
fn run_dir(trays: &std::path::Path) -> PathBuf {
    let made = std::fs::create_dir_all(trays)
        .and_then(|()| tempfile::Builder::new().prefix(RUN_PREFIX).tempdir_in(trays));
    match made {
        // Kept past the `TempDir`: `shutdown` removes it.
        Ok(dir) => dir.keep(),
        Err(e) => {
            log::warn!("tray cache in {}: {e}", trays.display());
            trays.join(format!("{RUN_PREFIX}{}", std::process::id()))
        }
    }
}

/// An engine event to the webview as `engine` `{ event }`.
fn forward(app: &AppHandle, event: Event) {
    match &event {
        Event::Failed { label, message, .. } => log::warn!("{label}: {message}"),
        Event::Done { path, .. } => {
            if let Some(jobs) = app.try_state::<Arc<Jobs>>() {
                jobs.platform.media_added(path);
            }
        }
        _ => {}
    }
    let _ = app.emit("engine", json!({ "event": event }));
}

/// Abort the tasks and wait until they are gone (their futures dropped, so their yt-dlp and
/// ffmpeg killed and their temp files removed).
async fn stop(tasks: Vec<Task>) {
    for task in &tasks {
        task.handle.abort();
    }
    for task in tasks {
        let _ = task.handle.await;
    }
}

async fn exists(path: &std::path::Path) -> bool {
    tokio::fs::try_exists(path).await.unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use serde_json::{from_value, json, to_value};

    use super::*;

    /// Two app instances never share a tray cache folder.
    #[test]
    fn each_run_gets_its_own_tray_folder() {
        let cache = tempfile::tempdir().unwrap();
        let trays = cache.path().join("tray");
        let (a, b) = (run_dir(&trays), run_dir(&trays));
        assert_ne!(a, b);
        for run in [&a, &b] {
            assert!(run.is_dir());
            assert_eq!(run.parent(), Some(trays.as_path()));
            let name = run.file_name().unwrap().to_string_lossy();
            assert!(name.starts_with(RUN_PREFIX), "{name}");
        }
    }

    /// The shapes the UI (`app/src`) is written against.
    #[test]
    fn tray_events_and_command_payloads_keep_their_shape() {
        let track = Track::for_tests("Song");
        assert_eq!(
            to_value(TrayEvent::added(4, Some(3), None, &track)).unwrap(),
            json!({ "kind": "added", "id": 4, "parent": 3, "input": null, "title": "Song",
                    "artist": null, "thumbnail": null, "duration": null, "youtube": false })
        );
        let playlist = TrayEvent::Playlist {
            id: 3,
            input: "https://x/list",
            title: "Mix",
            count: 2,
        };
        assert_eq!(
            to_value(playlist).unwrap(),
            json!({ "kind": "playlist", "id": 3, "input": "https://x/list", "title": "Mix", "count": 2 })
        );
        let prepared = Prepared {
            duration: Some(1.5),
            peaks: vec![0.5],
            suggestions: Vec::new(),
            exists: true,
        };
        assert_eq!(
            to_value(prepared).unwrap(),
            json!({ "duration": 1.5, "peaks": [0.5], "suggestions": [], "exists": true })
        );
        let items: Vec<SaveItem> = from_value(json!([
            { "id": 1, "cut": null },
            { "id": 2, "cut": { "keep": [{ "start": 0.0, "end": 9.5 }, { "start": 12.0, "end": null }],
                                "label": "trimmed" } },
        ]))
        .unwrap();
        assert!(items[0].cut.is_none());
        assert_eq!(items[1].id, 2);
        let keep = &items[1].cut.as_ref().unwrap().keep;
        assert_eq!(keep.len(), 2);
        // `null`: to the end of the audio.
        assert_eq!(keep[1].end, f64::INFINITY);
    }
}
