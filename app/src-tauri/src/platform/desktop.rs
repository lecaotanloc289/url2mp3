//! Windows and macOS: sidecar ffmpeg/qjs, downloaded yt-dlp.

use std::{
    path::PathBuf,
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result};
use url2mp3_core::Tools;

use super::{ToolsStatus, Updated};
use crate::ytdlp::{self, Store};

pub struct Platform {
    store: Store,
    /// ffmpeg and qjs, which Tauri puts next to the app's own executable.
    ffmpeg: PathBuf,
    qjs: PathBuf,
}

impl Platform {
    pub fn new(data_dir: PathBuf) -> Result<Self> {
        let exe = std::env::current_exe()?;
        let dir = exe.parent().context("app executable has no folder")?;
        let beside = |name: &str| dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        let store = Store::new(data_dir.join("yt-dlp"));
        // Nothing runs yet: old versions can go.
        store.prune();
        Ok(Self {
            store,
            ffmpeg: beside("ffmpeg"),
            qjs: beside("qjs"),
        })
    }

    pub fn default_out_dir() -> Option<PathBuf> {
        url2mp3_core::default_out_dir()
    }

    pub fn status(&self) -> ToolsStatus {
        match self.store.current() {
            Some(installed) => ToolsStatus::Ready {
                version: installed.version,
            },
            None => ToolsStatus::Missing,
        }
    }

    pub fn version(&self) -> Option<String> {
        self.store.current().map(|c| c.version)
    }

    /// Install yt-dlp if missing, or update it when `force` or a day has passed.
    /// `downloading` is told before a download starts.
    pub async fn update(
        &self,
        client: &reqwest::Client,
        force: bool,
        downloading: impl Fn(&str),
    ) -> Result<Updated> {
        let current = self.store.current();
        let now = SystemTime::now();
        if let Some(c) = &current
            && !force
            && !self.store.due_for_check(now)
        {
            return Ok(Updated {
                version: c.version.clone(),
                changed: false,
            });
        }
        let tag = ytdlp::latest_tag(client).await?;
        if current.as_ref().is_some_and(|c| c.version == tag) {
            let _ = self.store.mark_checked(now);
            return Ok(Updated {
                version: tag,
                changed: false,
            });
        }

        downloading(&tag);
        let (zip, sums) = ytdlp::fetch(client, &tag).await?;
        let root = self.store.root().to_owned();
        // Old versions stay until the next start (prune): running downloads may use one.
        let installed =
            tokio::task::spawn_blocking(move || Store::new(root).install(&tag, &zip, &sums)).await??;
        log::info!("yt-dlp {} installed", installed.version);
        // The first run of fresh executables is slow on macOS (malware scan): pay it now, but
        // never wait on it forever.
        let mut warm_up = url2mp3_core::command(&installed.exe);
        warm_up.arg("--version").kill_on_drop(true);
        let _ = tokio::time::timeout(Duration::from_secs(90), warm_up.output()).await;
        let _ = self.store.mark_checked(now);
        Ok(Updated {
            version: installed.version,
            changed: true,
        })
    }

    pub async fn tools(&self) -> Result<Tools> {
        let ytdlp = self.store.current().context("ytdlp-missing")?;
        Ok(Tools {
            ytdlp: ytdlp.exe,
            ytdlp_args: Vec::new(),
            ffmpeg: self.ffmpeg.clone(),
            js_runtime: Some(format!("quickjs:{}", self.qjs.display())),
            env: Vec::new(),
        })
    }

    /// Android only: keep downloads alive in the background.
    pub fn keep_alive(&self, _active: bool, _text: &str) {}

    /// Android only: a link shared from another app.
    pub fn take_shared(&self) -> Option<String> {
        None
    }
}
