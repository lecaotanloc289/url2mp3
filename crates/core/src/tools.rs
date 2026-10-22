//! Where the external programs live. The CLI finds them on PATH; the apps ship their own copies.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{Result, bail};
use tokio::process::Command;

#[derive(Clone, Debug)]
pub struct Tools {
    pub ytdlp: PathBuf,
    pub ffmpeg: PathBuf,
    /// Passed to yt-dlp as `--js-runtimes` (e.g. `deno:/path/to/deno`) for YouTube's signature
    /// challenge. None: yt-dlp's default, deno on PATH.
    pub js_runtime: Option<String>,
    /// Extra environment for both programs (e.g. PYTHONHOME on Android).
    pub env: Vec<(OsString, OsString)>,
}

impl Tools {
    pub fn from_path() -> Self {
        Self {
            ytdlp: "yt-dlp".into(),
            ffmpeg: "ffmpeg".into(),
            js_runtime: None,
            env: Vec::new(),
        }
    }

    pub async fn ytdlp_version(&self) -> Result<String> {
        let out = self.ytdlp_command().arg("--version").output().await?;
        if !out.status.success() {
            bail!("yt-dlp --version failed");
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    pub async fn check_ffmpeg(&self) -> Result<()> {
        let ok = self
            .ffmpeg_command()
            .arg("-version")
            .stdout(Stdio::null())
            .status()
            .await?
            .success();
        if !ok {
            bail!("ffmpeg -version failed");
        }
        Ok(())
    }

    pub(crate) fn ytdlp_command(&self) -> Command {
        self.command(&self.ytdlp)
    }

    pub(crate) fn ffmpeg_command(&self) -> Command {
        self.command(&self.ffmpeg)
    }

    fn command(&self, program: &Path) -> Command {
        let mut cmd = Command::new(program);
        cmd.envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .kill_on_drop(true);
        cmd
    }
}
