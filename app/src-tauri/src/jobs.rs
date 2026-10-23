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
};

use anyhow::Result;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, async_runtime::JoinHandle};
use url2mp3_core::{Engine, Event, Id};

use crate::{
    platform::{Platform, ToolsStatus},
    settings::{self, Settings},
};

pub struct Jobs {
    app: AppHandle,
    client: reqwest::Client,
    platform: Platform,
    settings_path: PathBuf,
    settings: Mutex<Settings>,
    engine: tokio::sync::Mutex<Option<(u32, Arc<Engine>)>>,
    generation: AtomicU32,
    running: Mutex<HashMap<(u32, Id), JoinHandle<()>>>,
    /// One yt-dlp download at a time.
    updating: tokio::sync::Mutex<()>,
}

impl Jobs {
    pub fn new(app: AppHandle, platform: Platform, config_dir: PathBuf) -> Self {
        let settings_path = config_dir.join("settings.json");
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
            generation: AtomicU32::new(0),
            running: Mutex::default(),
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

    /// Save, and build the next engine from the new settings.
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
        if Platform::UPDATE_IN_PLACE && !self.running.lock().expect("running lock").is_empty() {
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
            // New links use the new yt-dlp; running ones keep theirs.
            *self.engine.lock().await = None;
        }
        let _ = self.app.emit("tools", &status);
        result.map(|_| status)
    }

    /// Start downloading each link or search phrase. Its events follow on `engine`.
    pub async fn enqueue(self: &Arc<Self>, inputs: Vec<String>) -> Result<usize> {
        // An in-place update must finish before yt-dlp runs again; holding the lock while the
        // tasks are registered also keeps an update from starting under them.
        let _no_update = if Platform::UPDATE_IN_PLACE {
            Some(self.updating.lock().await)
        } else {
            None
        };
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
        let tools = self.platform.tools().await?;
        let options = self.settings().options()?;
        let generation = self.generation.fetch_add(1, Relaxed) + 1;
        let app = self.app.clone();
        let engine = Engine::start(tools, options, move |event: Event| {
            match &event {
                Event::Failed { label, message, .. } => log::warn!("{label}: {message}"),
                Event::Done { path, .. } => {
                    if let Some(jobs) = app.try_state::<Arc<Jobs>>() {
                        jobs.platform.media_added(path);
                    }
                }
                _ => {}
            }
            let _ = app.emit("engine", json!({ "gen": generation, "event": event }));
        })
        .await?;
        *slot = Some((generation, engine.clone()));
        Ok((generation, engine))
    }
}
