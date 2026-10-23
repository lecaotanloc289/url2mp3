//! Android: youtubedl-android supplies Python + yt-dlp, ffmpeg and QuickJS; the Kotlin
//! `ToolsPlugin` unpacks and updates them, keeps downloads alive in a foreground service and
//! hands over links shared from other apps.

use std::{collections::HashMap, path::PathBuf, sync::Mutex, time::SystemTime};

use anyhow::{Result, anyhow};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tauri::{
    AppHandle, Manager, Runtime, Wry,
    plugin::{Builder, PluginHandle, TauriPlugin},
};
use url2mp3_core::Tools;

use super::{ToolsStatus, Updated};
use crate::ytdlp::Store;

/// Registers the Kotlin plugin; add it to the builder before `Platform::new` runs.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("android-tools")
        .setup(|app, api| {
            let handle = api.register_android_plugin("dev.lecaotanloc.url2mp3", "ToolsPlugin")?;
            app.manage(Kotlin(handle));
            Ok(())
        })
        .build()
}

struct Kotlin<R: Runtime>(PluginHandle<R>);

/// What `ToolsPlugin.prepare` returns.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Prepared {
    python: PathBuf,
    ytdlp_script: PathBuf,
    ffmpeg: PathBuf,
    qjs: PathBuf,
    env: HashMap<String, String>,
    version: Option<String>,
}

#[derive(Deserialize)]
struct Text {
    text: Option<String>,
}

pub struct Platform {
    app: AppHandle,
    prepared: Mutex<Option<Prepared>>,
    /// Only for its once-a-day `last-check` file.
    checks: Store,
}

impl Platform {
    pub fn new(app: &AppHandle, data_dir: PathBuf) -> Result<Self> {
        Ok(Self {
            app: app.clone(),
            prepared: Mutex::default(),
            checks: Store::new(data_dir.join("yt-dlp")),
        })
    }

    /// Download/ accepts any file type (our temp files too), needs no permission on Android 11+
    /// for files the app creates, and music players index it.
    pub fn default_out_dir() -> Option<PathBuf> {
        Some(PathBuf::from("/storage/emulated/0/Download/url2mp3"))
    }

    pub fn status(&self) -> ToolsStatus {
        match self.version() {
            Some(version) => ToolsStatus::Ready { version },
            None => ToolsStatus::Missing,
        }
    }

    pub fn version(&self) -> Option<String> {
        self.prepared
            .lock()
            .expect("prepared lock")
            .as_ref()?
            .version
            .clone()
    }

    pub async fn update(
        &self,
        _client: &reqwest::Client,
        force: bool,
        downloading: impl Fn(&str),
    ) -> Result<Updated> {
        let before = self.prepare().await?.version.unwrap_or_default();
        let now = SystemTime::now();
        if !force && !self.checks.due_for_check(now) {
            return Ok(Updated {
                version: before,
                changed: false,
            });
        }
        downloading("latest");
        let text: Text = self.call("updateYtdlp", json!({})).await?;
        let version = text.text.unwrap_or_else(|| before.clone());
        let _ = self.checks.mark_checked(now);
        if let Some(p) = self.prepared.lock().expect("prepared lock").as_mut() {
            p.version = Some(version.clone());
        }
        Ok(Updated {
            changed: version != before,
            version,
        })
    }

    pub async fn tools(&self) -> Result<Tools> {
        let p = self.prepare().await?;
        Ok(Tools {
            ytdlp: p.python,
            ytdlp_args: vec![p.ytdlp_script.into()],
            ffmpeg: p.ffmpeg,
            js_runtime: Some(format!("quickjs:{}", p.qjs.display())),
            env: p.env.into_iter().map(|(k, v)| (k.into(), v.into())).collect(),
        })
    }

    /// Foreground service + notification while something downloads, so Android doesn't kill
    /// the app when the user switches away.
    pub fn keep_alive(&self, active: bool, text: &str) {
        let kotlin = self.kotlin();
        let payload = json!({ "active": active, "text": text });
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(e) = kotlin.run_mobile_plugin::<Value>("keepAlive", payload) {
                log::warn!("keepAlive: {e}");
            }
        });
    }

    /// The text of the last share intent, once.
    pub fn take_shared(&self) -> Option<String> {
        let text: Text = self.kotlin().run_mobile_plugin("takeShared", json!({})).ok()?;
        text.text.filter(|t| !t.trim().is_empty())
    }

    /// Unpacks Python and ffmpeg on first use (a few seconds), then cached.
    async fn prepare(&self) -> Result<Prepared> {
        if let Some(p) = self.prepared.lock().expect("prepared lock").clone() {
            return Ok(p);
        }
        let p: Prepared = self.call("prepare", json!({})).await?;
        *self.prepared.lock().expect("prepared lock") = Some(p.clone());
        Ok(p)
    }

    /// Kotlin commands block until they resolve: run them off the async runtime.
    async fn call<T: DeserializeOwned + Send + 'static>(
        &self,
        command: &'static str,
        payload: Value,
    ) -> Result<T> {
        let kotlin = self.kotlin();
        tauri::async_runtime::spawn_blocking(move || kotlin.run_mobile_plugin(command, payload))
            .await?
            .map_err(|e| anyhow!("{command}: {e}"))
    }

    fn kotlin(&self) -> PluginHandle<Wry> {
        self.app.state::<Kotlin<Wry>>().0.clone()
    }
}
