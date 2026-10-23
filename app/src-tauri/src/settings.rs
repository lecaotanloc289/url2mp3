//! What the user chose in Settings, kept as JSON in the app's config folder.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url2mp3_core::{Format, Options};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub format: Format,
    /// kbps when re-encoding; None: MP3 VBR V0, M4A 256, Opus 160.
    pub bitrate: Option<u32>,
    /// None: the Downloads folder.
    pub out_dir: Option<PathBuf>,
    pub jobs: usize,
    pub connections: usize,
    pub playlist_limit: Option<usize>,
    pub cover: bool,
    /// Browser whose login yt-dlp borrows (chrome, edge, firefox, safari, brave).
    pub cookies_browser: Option<String>,
    /// "vi" or "en"; None follows the system.
    pub language: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            format: Format::Mp3,
            bitrate: None,
            out_dir: None,
            // MP3 encoding is CPU-bound (LAME is single-threaded), so one track per core.
            jobs: std::thread::available_parallelism().map_or(4, |n| n.get().clamp(2, 8)),
            connections: 8,
            playlist_limit: None,
            cover: true,
            cookies_browser: None,
            language: None,
        }
    }
}

impl Settings {
    /// Engine options, with every number brought into the range the CLI accepts.
    pub fn options(&self) -> Result<Options> {
        let out = match &self.out_dir {
            Some(dir) => dir.clone(),
            None => url2mp3_core::default_out_dir().context("no Downloads folder")?,
        };
        Ok(Options {
            format: self.format,
            bitrate: self.bitrate.map(|b| b.clamp(32, 320)),
            out,
            jobs: self.jobs.clamp(1, 32),
            connections: self.connections.clamp(1, 32),
            // A song opened from a YouTube Mix is `watch?v=…&list=RD…`: take the song, not the
            // endless mix. `playlist?list=…` links still download the playlist.
            playlist: false,
            limit: self.playlist_limit.filter(|n| *n > 0),
            cover: self.cover,
            force: false,
            cookies_from_browser: self.cookies_browser.clone().filter(|b| !b.is_empty()),
        })
    }
}

/// Missing or unreadable file: defaults, so a bad settings file never stops the app. A field
/// this version can't read (say a format from a newer version) falls back alone; the others
/// are kept.
pub fn load(path: &Path) -> Settings {
    let Ok(bytes) = fs::read(path) else {
        return Settings::default();
    };
    let Ok(Value::Object(saved)) = serde_json::from_slice::<Value>(&bytes) else {
        log::warn!("{} is not a JSON object, using defaults", path.display());
        return Settings::default();
    };
    let Ok(Value::Object(mut merged)) = serde_json::to_value(Settings::default()) else {
        return Settings::default();
    };
    for (key, value) in saved {
        let previous = merged.insert(key.clone(), value);
        if serde_json::from_value::<Settings>(Value::Object(merged.clone())).is_err() {
            log::warn!("ignoring setting {key} in {}", path.display());
            match previous {
                Some(v) => merged.insert(key, v),
                None => merged.remove(&key),
            };
        }
    }
    serde_json::from_value(Value::Object(merged)).unwrap_or_default()
}

pub fn save(path: &Path, settings: &Settings) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(settings)?)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/settings.json");
        let settings = Settings {
            format: Format::Opus,
            out_dir: Some("/music".into()),
            language: Some("en".into()),
            ..Settings::default()
        };

        save(&path, &settings).unwrap();

        assert_eq!(load(&path), settings);
    }

    #[test]
    fn missing_or_broken_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(load(&path), Settings::default());
        fs::write(&path, "{not json").unwrap();
        assert_eq!(load(&path), Settings::default());
    }

    #[test]
    fn tolerates_missing_and_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, r#"{"format":"m4a","someday":1}"#).unwrap();
        let settings = load(&path);
        assert_eq!(settings.format, Format::M4a);
        assert_eq!(settings.connections, 8);
    }

    #[test]
    fn one_unreadable_field_does_not_reset_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(
            &path,
            r#"{"format":"flac","outDir":"/music","jobs":"many","cover":false}"#,
        )
        .unwrap();
        let settings = load(&path);
        assert_eq!(settings.format, Format::Mp3);
        assert_eq!(settings.out_dir, Some(PathBuf::from("/music")));
        assert_eq!(settings.jobs, Settings::default().jobs);
        assert!(!settings.cover);
    }

    #[test]
    fn options_are_clamped() {
        let settings = Settings {
            out_dir: Some("/music".into()),
            bitrate: Some(999),
            jobs: 0,
            connections: 100,
            playlist_limit: Some(0),
            cookies_browser: Some(String::new()),
            ..Settings::default()
        };
        let opts = settings.options().unwrap();
        assert_eq!(opts.bitrate, Some(320));
        assert_eq!((opts.jobs, opts.connections), (1, 32));
        assert_eq!(opts.limit, None);
        assert_eq!(opts.cookies_from_browser, None);
        assert_eq!(opts.out, PathBuf::from("/music"));
    }
}
