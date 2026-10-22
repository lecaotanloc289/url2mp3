//! Terminal output: an indicatif bar per running link or track, a line per finished one.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

use anyhow::Result;
use indicatif::{HumanBytes, MultiProgress, ProgressBar, ProgressFinish, ProgressStyle};
use url2mp3_core::{Event, Format, Id, Skip};

pub struct Render {
    multi: MultiProgress,
    bar_style: ProgressStyle,
    spin_style: ProgressStyle,
    format: Format,
    out: PathBuf,
    /// Running link or track → its bar and title. Removing a bar clears it from the screen.
    bars: Mutex<HashMap<Id, (ProgressBar, String)>>,
}

impl Render {
    pub fn new(format: Format, out: PathBuf) -> Result<Self> {
        Ok(Self {
            multi: MultiProgress::new(),
            bar_style: ProgressStyle::with_template(
                "{spinner:.cyan} [{bar:22.cyan/blue}] {bytes:>9}/{total_bytes:<9} {binary_bytes_per_sec:>11} {wide_msg}",
            )?
            .progress_chars("=> "),
            spin_style: ProgressStyle::with_template("{spinner:.cyan} {wide_msg}")?,
            format,
            out,
            bars: Mutex::default(),
        })
    }

    pub fn handle(&self, event: Event) {
        let mut bars = self.bars.lock().expect("bars lock");
        match event {
            Event::Probing { id, label, .. } => {
                let pb = self.add(ProgressBar::new_spinner(), &self.spin_style);
                pb.set_message(format!("đang đọc link: {label}"));
                bars.insert(id, (pb, label));
            }
            Event::Playlist { id, title, count } => {
                bars.remove(&id);
                self.println(format!("▸ Playlist {title}: {count} bài"));
            }
            Event::Track { id, title, .. } => {
                let pb = self.add(ProgressBar::new(0), &self.bar_style);
                pb.set_message(title.clone());
                // Replaces (and so clears) the link's probing spinner.
                bars.insert(id, (pb, title));
            }
            Event::Progress { id, done, total } => {
                if let Some((pb, _)) = bars.get(&id) {
                    pb.set_length(total);
                    pb.set_position(done);
                }
            }
            Event::Fallback { id, reason } => {
                if let Some((pb, title)) = bars.get(&id) {
                    if let Some(reason) = reason {
                        self.println(format!(
                            "  {title}: tải trực tiếp lỗi ({reason}), chuyển sang yt-dlp"
                        ));
                    }
                    pb.set_style(self.spin_style.clone());
                    pb.set_message(format!("yt-dlp đang tải: {title}"));
                }
            }
            Event::Converting { id } => {
                if let Some((pb, title)) = bars.get(&id) {
                    pb.set_style(self.spin_style.clone());
                    pb.set_message(format!("đang chuyển sang {}: {title}", self.format.ext()));
                }
            }
            Event::Done {
                id,
                path,
                bytes,
                secs,
            } => {
                bars.remove(&id);
                self.println(format!(
                    "✓ {}  ({}, {secs:.1}s)",
                    self.relative(&path).display(),
                    HumanBytes(bytes)
                ));
            }
            Event::Skipped {
                id, title, reason, ..
            } => {
                bars.remove(&id);
                match reason {
                    Skip::Duplicate => self.println(format!("• trùng bài, bỏ qua: {title}")),
                    Skip::Exists(path) => self.println(format!(
                        "• đã có: {} (--force để tải lại)",
                        self.relative(&path).display()
                    )),
                }
            }
            Event::Failed {
                id, label, message, ..
            } => {
                bars.remove(&id);
                self.println(format!("✗ {label}: {message}"));
            }
        }
    }

    pub fn println(&self, line: impl AsRef<str>) {
        if self.multi.is_hidden() {
            println!("{}", line.as_ref());
        } else {
            let _ = self.multi.println(line);
        }
    }

    pub fn clear(&self) {
        self.bars.lock().expect("bars lock").clear();
        let _ = self.multi.clear();
    }

    fn add(&self, pb: ProgressBar, style: &ProgressStyle) -> ProgressBar {
        let pb = self.multi.add(pb.with_finish(ProgressFinish::AndClear));
        pb.set_style(style.clone());
        pb.enable_steady_tick(Duration::from_millis(120));
        pb
    }

    fn relative<'a>(&self, path: &'a Path) -> &'a Path {
        path.strip_prefix(&self.out).unwrap_or(path)
    }
}
