//! The "ready to download" tray: tracks that were resolved but not saved yet, and where each
//! one caches its audio and preview copy (`<app cache>/tray/run-*/<id>/`, the run folder being
//! this app instance's own).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use url2mp3_core::{Id, Track};

#[derive(Clone)]
pub struct Entry {
    /// As yt-dlp read it, for the format of that moment (the track remembers which): fetched
    /// or saved without cached audio after a format change, it is read again first.
    pub track: Track,
    /// The playlist row this track belongs to.
    pub parent: Option<Id>,
    /// The playlist's folder name inside the output folder.
    pub playlist: Option<String>,
    /// Held by whoever reads or writes the row's cache folder.
    pub cache: Arc<tokio::sync::Mutex<()>>,
}

impl Entry {
    pub fn new(track: Track, parent: Option<Id>, playlist: Option<String>) -> Self {
        Self {
            track,
            parent,
            playlist,
            cache: Arc::default(),
        }
    }
}

pub struct Tray {
    root: PathBuf,
    entries: HashMap<Id, Entry>,
}

impl Tray {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            entries: HashMap::new(),
        }
    }

    pub fn insert(&mut self, id: Id, entry: Entry) {
        self.entries.insert(id, entry);
    }

    pub fn get(&self, id: Id) -> Option<Entry> {
        self.entries.get(&id).cloned()
    }

    /// Replaces the track of an entry still in the tray (a flat playlist entry, once probed).
    pub fn set_track(&mut self, id: Id, track: Track) -> bool {
        match self.entries.get_mut(&id) {
            Some(entry) => {
                entry.track = track;
                true
            }
            None => false,
        }
    }

    /// The tracks of playlist `id`.
    pub fn children(&self, id: Id) -> Vec<Id> {
        let children = self.entries.iter().filter(|(_, e)| e.parent == Some(id));
        children.map(|(child, _)| *child).collect()
    }

    /// Forgets `id` and, for a playlist, its tracks. Returns every id concerned, `id` first,
    /// whether or not it had an entry (a playlist row has none, a link still resolving neither).
    pub fn remove(&mut self, id: Id) -> Vec<Id> {
        let mut removed = vec![id];
        removed.extend(self.children(id));
        for id in &removed {
            self.entries.remove(id);
        }
        removed
    }

    /// The folder holding everything cached for `id`, removed whole with it.
    pub fn dir(&self, id: Id) -> PathBuf {
        self.root.join(id.to_string())
    }

    /// The fetched audio stream and its AAC preview copy.
    pub fn paths(&self, id: Id) -> (PathBuf, PathBuf) {
        let dir = self.dir(id);
        (dir.join("audio"), dir.join("preview.m4a"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str, parent: Option<Id>) -> Entry {
        Entry::new(Track::for_tests(title), parent, parent.map(|_| "list".into()))
    }

    #[test]
    fn remove_group_removes_children() {
        let mut tray = Tray::new(PathBuf::from("cache").join("tray"));
        tray.insert(2, entry("a", Some(1)));
        tray.insert(3, entry("b", Some(1)));
        tray.insert(4, entry("alone", None));
        let mut removed = tray.remove(1);
        removed.sort_unstable();
        assert_eq!(removed, vec![1, 2, 3]);
        assert!(tray.get(2).is_none());
        assert!(tray.get(3).is_none());
        assert_eq!(tray.get(4).map(|e| e.track.title), Some("alone".into()));
        assert_eq!(tray.remove(4), vec![4]);
    }

    #[test]
    fn paths_live_under_the_tray_dir() {
        let root = PathBuf::from("cache").join("tray").join("run-x1");
        let tray = Tray::new(root.clone());
        assert_eq!(tray.dir(7), root.join("7"));
        assert_eq!(
            tray.paths(7),
            (root.join("7").join("audio"), root.join("7").join("preview.m4a"))
        );
    }
}
