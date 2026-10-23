//! Where yt-dlp, ffmpeg and QuickJS come from, which differs per platform:
//! - desktop: ffmpeg and qjs ship next to the app; yt-dlp is downloaded into the app data
//!   folder and kept up to date by `crate::ytdlp::Store`;
//! - Android: the youtubedl-android library packs Python, yt-dlp, ffmpeg and QuickJS as
//!   native libraries; a Kotlin plugin (`gen/android/…/ToolsPlugin.kt`) prepares and updates them.
//!
//! Both expose the same `Platform` API to `crate::jobs`.

use serde::Serialize;

#[cfg(target_os = "android")]
mod android;
#[cfg(not(target_os = "android"))]
mod desktop;

#[cfg(target_os = "android")]
pub use android::{Platform, init};
#[cfg(not(target_os = "android"))]
pub use desktop::Platform;

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

/// Result of an install/update check.
pub struct Updated {
    pub version: String,
    /// A different yt-dlp than before: the next links need a new engine.
    pub changed: bool,
}
