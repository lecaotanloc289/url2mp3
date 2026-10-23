//! The download queue behind the UI: one engine per settings generation, a task per link the
//! user adds, and every engine event forwarded to the webview as `engine` `{ gen, event }`.
//!
//! A new engine is built after settings change or yt-dlp updates; its ids start again at 0,
//! so the UI keys rows by `gen:id`. Running links keep the engine they started with.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering::Relaxed},
    },
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, async_runtime::JoinHandle};
use url2mp3_core::{Engine, Event, Id, Tools};

use crate::{
    settings::{self, Settings},
    ytdlp::{self, Store},
};

/// ffmpeg and qjs, which Tauri puts next to the app's own executable.
pub struct Sidecars {
    pub ffmpeg: PathBuf,
    pub qjs: PathBuf,
}

impl Sidecars {
    pub fn beside_exe() -> Result<Self> {
        let exe = std::env::current_exe()?;
        let dir = exe.parent().context("app executable has no folder")?;
        let path = |name: &str| dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        Ok(Self {
            ffmpeg: path("ffmpeg"),
            qjs: path("qjs"),
        })
    }
}

/// State of the app's yt-dlp, sent to the UI as `tools` events.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "phase", rename_all = "camelCase")]
pub enum ToolsStatus {
    Missing,
    Downloading {
        version: String,
    },
    Ready {
        version: String,
    },
    Error {
        message: String,
        version: Option<String>,
    },
}

pub struct Jobs {
    app: AppHandle,
    client: reqwest::Client,
    sidecars: Sidecars,
    store: Store,
    settings_path: PathBuf,
    settings: Mutex<Settings>,
    engine: tokio::sync::Mutex<Option<(u32, Arc<Engine>)>>,
    generation: AtomicU32,
    running: Mutex<HashMap<(u32, Id), JoinHandle<()>>>,
    /// One yt-dlp download at a time.
    updating: tokio::sync::Mutex<()>,
}

impl Jobs {
    pub fn new(app: AppHandle, sidecars: Sidecars, data_dir: PathBuf, config_dir: PathBuf) -> Self {
        let settings_path = config_dir.join("settings.json");
        let store = Store::new(data_dir.join("yt-dlp"));
        store.prune();
        Self {
            app,
            client: reqwest::Client::builder()
                .user_agent(concat!("url2mp3/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("reqwest client"),
            sidecars,
            store,
            settings: Mutex::new(settings::load(&settings_path)),
            settings_path,
            engine: tokio::sync::Mutex::new(None),
            generation: AtomicU32::new(0),
            running: Mutex::default(),
            updating: tokio::sync::Mutex::new(()),
        }
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().expect("settings lock").clone()
    }

    /// Save, and build the next engine from the new settings.
    pub async fn set_settings(&self, new: Settings) -> Result<()> {
        settings::save(&self.settings_path, &new)?;
        *self.settings.lock().expect("settings lock") = new;
        *self.engine.lock().await = None;
        Ok(())
    }

    pub fn tools_status(&self) -> ToolsStatus {
        match self.store.current() {
            Some(installed) => ToolsStatus::Ready {
                version: installed.version,
            },
            None => ToolsStatus::Missing,
        }
    }

    /// Install yt-dlp if missing, or update it when `force` or a day has passed. Progress and
    /// errors also go to the UI as `tools` events.
    pub async fn update_ytdlp(&self, force: bool) -> Result<ToolsStatus> {
        let _one_at_a_time = self.updating.lock().await;
        let result = self.try_update(force).await;
        let status = match &result {
            Ok(status) => status.clone(),
            Err(e) => {
                log::error!("yt-dlp update: {e:#}");
                ToolsStatus::Error {
                    message: format!("{e:#}"),
                    version: self.store.current().map(|c| c.version),
                }
            }
        };
        let _ = self.app.emit("tools", &status);
        result
    }

    async fn try_update(&self, force: bool) -> Result<ToolsStatus> {
        let current = self.store.current();
        let now = SystemTime::now();
        if let Some(c) = &current
            && !force
            && !self.store.due_for_check(now)
        {
            return Ok(ToolsStatus::Ready {
                version: c.version.clone(),
            });
        }
        let tag = ytdlp::latest_tag(&self.client).await?;
        if current.as_ref().is_some_and(|c| c.version == tag) {
            let _ = self.store.mark_checked(now);
            return Ok(ToolsStatus::Ready { version: tag });
        }

        let _ = self
            .app
            .emit("tools", ToolsStatus::Downloading { version: tag.clone() });
        let (zip, sums) = ytdlp::fetch(&self.client, &tag).await?;
        let root = self.store.root().to_owned();
        let installed =
            tokio::task::spawn_blocking(move || Store::new(root).install(&tag, &zip, &sums)).await??;
        // New links use the new yt-dlp; running ones keep theirs, whose folder stays until the
        // next start (Store::prune).
        *self.engine.lock().await = None;
        log::info!("yt-dlp {} installed", installed.version);
        // The first run of fresh executables is slow on macOS (malware scan): pay it now, but
        // never wait on it forever.
        let mut warm_up = url2mp3_core::command(&installed.exe);
        warm_up.arg("--version").kill_on_drop(true);
        let _ = tokio::time::timeout(Duration::from_secs(90), warm_up.output()).await;
        let _ = self.store.mark_checked(now);
        Ok(ToolsStatus::Ready {
            version: installed.version,
        })
    }

    /// Start downloading each link or search phrase. Its events follow on `engine`.
    pub async fn enqueue(self: &Arc<Self>, inputs: Vec<String>) -> Result<usize> {
        let (generation, engine) = self.engine().await?;
        let inputs: Vec<String> = inputs
            .iter()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        for input in &inputs {
            let id = engine.next_id();
            let task = engine.clone().process(id, input.clone());
            let jobs = self.clone();
            // Held across spawn + insert, so a task that ends at once removes its entry after.
            let mut running = self.running.lock().expect("running lock");
            let handle = tauri::async_runtime::spawn(async move {
                task.await;
                jobs.running
                    .lock()
                    .expect("running lock")
                    .remove(&(generation, id));
            });
            running.insert((generation, id), handle);
        }
        Ok(inputs.len())
    }

    /// Stop a link (and every track of a playlist) and tell the UI once it has stopped.
    pub async fn cancel(&self, generation: u32, id: Id) {
        let handle = self
            .running
            .lock()
            .expect("running lock")
            .remove(&(generation, id));
        // Already finished: nothing to cancel, and the UI already shows how it ended.
        let Some(handle) = handle else { return };
        handle.abort();
        // Its claims are given back only once the task is gone (see Engine::process).
        let _ = handle.await;
        let _ = self.app.emit(
            "engine",
            json!({ "gen": generation, "event": { "kind": "cancelled", "id": id } }),
        );
    }

    /// Stop every download before the app exits, so no yt-dlp or ffmpeg outlives it and the
    /// engines remove their temp folders.
    pub async fn shutdown(&self) {
        let handles: Vec<_> = self.running.lock().expect("running lock").drain().collect();
        for (_, handle) in &handles {
            handle.abort();
        }
        for (_, handle) in handles {
            let _ = handle.await;
        }
        *self.engine.lock().await = None;
    }

    async fn engine(&self) -> Result<(u32, Arc<Engine>)> {
        let mut slot = self.engine.lock().await;
        if let Some((generation, engine)) = &*slot {
            return Ok((*generation, engine.clone()));
        }
        let ytdlp = self.store.current().context("ytdlp-missing")?;
        let tools = Tools {
            ytdlp: ytdlp.exe,
            ffmpeg: self.sidecars.ffmpeg.clone(),
            js_runtime: Some(format!("quickjs:{}", self.sidecars.qjs.display())),
            env: Vec::new(),
        };
        let options = self.settings().options()?;
        let generation = self.generation.fetch_add(1, Relaxed) + 1;
        let app = self.app.clone();
        let engine = Engine::start(tools, options, move |event: Event| {
            if let Event::Failed { label, message, .. } = &event {
                log::warn!("{label}: {message}");
            }
            let _ = app.emit("engine", json!({ "gen": generation, "event": event }));
        })
        .await?;
        *slot = Some((generation, engine.clone()));
        Ok((generation, engine))
    }
}
