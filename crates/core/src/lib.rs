//! url2mp3's engine: yt-dlp resolves a link, our own downloader fetches the audio over parallel
//! connections, ffmpeg converts and tags it. Used by the `url2mp3` CLI and the apps.

mod download;
mod engine;
mod names;
mod resolve;
mod tools;
mod transcode;

use std::path::PathBuf;

pub use engine::{Engine, Event, Id, Options, Outcome, Skip};
pub use tools::Tools;
pub use transcode::Format;

/// `~/Downloads`, the default output folder.
pub fn default_out_dir() -> Option<PathBuf> {
    dirs::download_dir().or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
}
