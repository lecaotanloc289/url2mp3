//! The app's own copy of yt-dlp. YouTube breaks yt-dlp every few weeks, so instead of shipping
//! it inside the app we download yt-dlp's onedir build from its GitHub releases, check it
//! against the release's SHA2-256SUMS, and keep it up to date. Onedir, not onefile: the onefile
//! build unpacks itself on every run (~13 s on macOS), and onedir builds can't `yt-dlp -U`.
//!
//! Layout under `root`: `<tag>/` (one unpacked release), `current` (the tag in use),
//! `last-check` (when we last asked GitHub for a newer tag).

use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::Client;
use sha2::{Digest, Sha256};

#[cfg(target_os = "macos")]
pub const ASSET: &str = "yt-dlp_macos.zip";
#[cfg(target_os = "macos")]
pub const EXE: &str = "yt-dlp_macos";
#[cfg(windows)]
pub const ASSET: &str = "yt-dlp_win.zip";
#[cfg(windows)]
pub const EXE: &str = "yt-dlp.exe";
#[cfg(not(any(target_os = "macos", windows)))]
pub const ASSET: &str = "yt-dlp_linux.zip";
#[cfg(not(any(target_os = "macos", windows)))]
pub const EXE: &str = "yt-dlp_linux";

const RELEASES: &str = "https://github.com/yt-dlp/yt-dlp/releases";
const CHECK_EVERY: Duration = Duration::from_secs(24 * 3600);

#[derive(Clone, Debug, PartialEq)]
pub struct Installed {
    pub version: String,
    pub exe: PathBuf,
}

pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn current(&self) -> Option<Installed> {
        let version = fs::read_to_string(self.root.join("current"))
            .ok()?
            .trim()
            .to_owned();
        let exe = self.root.join(&version).join(EXE);
        exe.is_file().then_some(Installed { version, exe })
    }

    /// Verify `zip` against `sums`, unpack it as release `tag`, make it current and remove
    /// every other version. Blocking: run it off the async runtime.
    pub fn install(&self, tag: &str, zip: &[u8], sums: &str) -> Result<Installed> {
        if tag.is_empty() || tag.contains(['/', '\\']) || tag.starts_with('.') {
            bail!("bad yt-dlp tag {tag:?}");
        }
        let expected =
            expected_sha256(sums, ASSET).with_context(|| format!("{ASSET} not in SHA2-256SUMS"))?;
        let got = hex(&Sha256::digest(zip));
        if got != expected {
            bail!("{ASSET}: checksum mismatch (got {got}, expected {expected})");
        }

        fs::create_dir_all(&self.root)?;
        let partial = self.root.join(format!("{tag}.partial"));
        let _ = fs::remove_dir_all(&partial);
        zip::ZipArchive::new(Cursor::new(zip))?
            .extract(&partial)
            .with_context(|| format!("unpack {ASSET}"))?;
        if !partial.join(EXE).is_file() {
            bail!("{ASSET} has no {EXE}");
        }
        let dir = self.root.join(tag);
        let _ = fs::remove_dir_all(&dir);
        fs::rename(&partial, &dir)?;

        let pointer = self.root.join("current.tmp");
        fs::write(&pointer, tag)?;
        fs::rename(&pointer, self.root.join("current"))?;

        for entry in fs::read_dir(&self.root)?.flatten() {
            let old = entry.path();
            if old.is_dir() && entry.file_name() != tag {
                // A running yt-dlp on Windows keeps its files locked; the next install retries.
                let _ = fs::remove_dir_all(old);
            }
        }
        Ok(Installed {
            version: tag.to_owned(),
            exe: dir.join(EXE),
        })
    }

    /// Whether a day has passed since we last asked GitHub for a newer yt-dlp.
    pub fn due_for_check(&self, now: SystemTime) -> bool {
        let last = fs::read_to_string(self.root.join("last-check"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(|secs| UNIX_EPOCH + Duration::from_secs(secs));
        last.is_none_or(|t| now.duration_since(t).unwrap_or_default() >= CHECK_EVERY)
    }

    pub fn mark_checked(&self, now: SystemTime) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        let secs = now.duration_since(UNIX_EPOCH)?.as_secs();
        fs::write(self.root.join("last-check"), secs.to_string())?;
        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Tag of yt-dlp's latest stable release, e.g. `2026.10.21`.
pub async fn latest_tag(client: &Client) -> Result<String> {
    // github.com/…/releases/latest redirects to …/releases/tag/<tag>: no API rate limit.
    let resp = client
        .get(format!("{RELEASES}/latest"))
        .send()
        .await?
        .error_for_status()?;
    let tag = resp
        .url()
        .path_segments()
        .and_then(|mut s| s.next_back())
        .filter(|t| !t.is_empty() && *t != "latest")
        .ok_or_else(|| anyhow!("no tag in {}", resp.url()))?;
    Ok(tag.to_owned())
}

/// The release's onedir zip and its SHA2-256SUMS.
pub async fn fetch(client: &Client, tag: &str) -> Result<(Vec<u8>, String)> {
    let get = |name: &str| {
        let url = format!("{RELEASES}/download/{tag}/{name}");
        async move {
            let resp = client.get(&url).send().await?.error_for_status()?;
            anyhow::Ok(resp.bytes().await?)
        }
    };
    let (zip, sums) = tokio::try_join!(get(ASSET), get("SHA2-256SUMS"))?;
    Ok((zip.to_vec(), String::from_utf8(sums.to_vec())?))
}

/// `<hex>  <name>` lines → the hex digest for `name`.
fn expected_sha256(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (digest, file) = line.split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then(|| digest.to_ascii_lowercase())
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::write::SimpleFileOptions;

    use super::*;

    /// A zip shaped like yt-dlp's onedir build.
    fn release_zip(marker: &str) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let exe = SimpleFileOptions::default().unix_permissions(0o755);
        zip.start_file(EXE, exe).unwrap();
        zip.write_all(format!("#!/bin/sh\necho {marker}\n").as_bytes())
            .unwrap();
        zip.start_file("_internal/base_library.zip", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"lib").unwrap();
        zip.finish().unwrap().into_inner()
    }

    fn sums_for(zip: &[u8]) -> String {
        format!(
            "{}  yt-dlp.exe\n{}  {ASSET}\n",
            "0".repeat(64),
            hex(&Sha256::digest(zip))
        )
    }

    #[test]
    fn installs_and_reports_current() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("yt-dlp"));
        assert_eq!(store.current(), None);

        let zip = release_zip("one");
        let installed = store.install("2026.10.21", &zip, &sums_for(&zip)).unwrap();

        assert_eq!(store.current(), Some(installed.clone()));
        assert_eq!(installed.version, "2026.10.21");
        assert!(installed.exe.ends_with(format!("2026.10.21/{EXE}")));
        assert!(
            installed
                .exe
                .with_file_name("_internal")
                .join("base_library.zip")
                .is_file()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&installed.exe).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "{mode:o}");
        }
    }

    #[test]
    fn rejects_a_wrong_checksum_and_keeps_the_current_version() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_owned());
        let good = release_zip("one");
        store.install("1", &good, &sums_for(&good)).unwrap();

        let err = store
            .install("2", &release_zip("two"), &sums_for(&good))
            .unwrap_err();

        assert!(format!("{err:#}").contains("checksum mismatch"), "{err:#}");
        assert_eq!(store.current().unwrap().version, "1");
    }

    #[test]
    fn rejects_sums_without_our_asset() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_owned());
        let zip = release_zip("one");
        let sums = format!("{}  something-else.zip\n", hex(&Sha256::digest(&zip)));
        assert!(store.install("1", &zip, &sums).is_err());
        assert_eq!(store.current(), None);
    }

    #[test]
    fn a_new_version_replaces_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_owned());
        let (one, two) = (release_zip("one"), release_zip("two"));
        store.install("1", &one, &sums_for(&one)).unwrap();

        store.install("2", &two, &sums_for(&two)).unwrap();

        assert_eq!(store.current().unwrap().version, "2");
        assert!(!dir.path().join("1").exists());
        assert!(
            fs::read_to_string(store.current().unwrap().exe)
                .unwrap()
                .contains("two")
        );
    }

    #[test]
    fn refuses_tags_that_escape_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_owned());
        let zip = release_zip("one");
        for tag in ["", "../x", "a/b", ".hidden"] {
            assert!(store.install(tag, &zip, &sums_for(&zip)).is_err(), "{tag:?}");
        }
    }

    #[test]
    fn checks_once_a_day() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("yt-dlp"));
        let now = SystemTime::now();
        assert!(store.due_for_check(now));

        store.mark_checked(now).unwrap();

        assert!(!store.due_for_check(now + Duration::from_secs(3600)));
        assert!(store.due_for_check(now + Duration::from_secs(25 * 3600)));
    }

    /// Network: `cargo test -p url2mp3-app --lib -- --ignored installs_the_real`
    #[tokio::test]
    #[ignore]
    async fn installs_the_real_latest_release() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_owned());
        let client = Client::new();
        let tag = latest_tag(&client).await.unwrap();
        let (zip, sums) = fetch(&client, &tag).await.unwrap();

        let installed = store.install(&tag, &zip, &sums).unwrap();

        let out = std::process::Command::new(&installed.exe)
            .arg("--version")
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), tag);
    }

    #[test]
    fn parses_sums_lines() {
        let sums = "AbC  yt-dlp_win.zip\nabc *yt-dlp_macos.zip\n";
        assert_eq!(expected_sha256(sums, "yt-dlp_win.zip").as_deref(), Some("abc"));
        assert_eq!(expected_sha256(sums, "yt-dlp_macos.zip").as_deref(), Some("abc"));
        assert_eq!(expected_sha256(sums, "yt-dlp"), None);
    }
}
