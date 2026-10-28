//! url2mp3's engine: yt-dlp resolves a link, our own downloader fetches the audio over parallel
//! connections, ffmpeg converts and tags it. Used by the `url2mp3` CLI and the apps.

mod cut;
mod download;
mod engine;
mod names;
mod preview;
mod resolve;
mod tools;
mod transcode;

use std::path::PathBuf;

pub use cut::{Cut, Range, normalize};
pub use engine::{Engine, Event, Id, Options, Outcome, Skip};
pub use preview::{PEAK_BUCKETS, make_preview, peaks};
pub use tools::{Tools, command};
pub use transcode::Format;
/// Exposed for the real-ffmpeg integration test only.
#[doc(hidden)]
pub use transcode::{Job, Tags, encode};

/// `~/Downloads`, the default output folder.
pub fn default_out_dir() -> Option<PathBuf> {
    dirs::download_dir().or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
}

/// A reqwest client builder that trusts the system's certificate authorities. On Android the
/// platform verifier would need JNI set-up, so the system CA folders are read directly.
pub fn http_client() -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder();
    #[cfg(target_os = "android")]
    let builder = builder.tls_certs_only(android_roots());
    builder
}

/// The CAs Android trusts: the updatable Conscrypt store (Android 14+) and the system one.
#[cfg(target_os = "android")]
fn android_roots() -> Vec<reqwest::Certificate> {
    [
        "/apex/com.android.conscrypt/cacerts",
        "/system/etc/security/cacerts",
    ]
    .iter()
    .filter_map(|dir| std::fs::read_dir(dir).ok())
    .flat_map(|dir| dir.flatten())
    .filter_map(|file| std::fs::read(file.path()).ok())
    .filter_map(|pem| reqwest::Certificate::from_pem(&pem).ok())
    .collect()
}
