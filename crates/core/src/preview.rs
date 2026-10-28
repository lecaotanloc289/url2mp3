//! Listening before saving: an AAC `.m4a` preview copy (the one format every webview plays) and
//! the waveform peaks drawn under the trim handles.

use std::{path::Path, process::Stdio};

use anyhow::{Context, Result};
use tokio::io::AsyncReadExt;

use crate::tools::Tools;

/// Bars in the waveform.
pub const PEAK_BUCKETS: usize = 1200;

/// Peak extraction runs on 4 kHz mono PCM: plenty for a waveform, 8 KB/s.
const PEAK_RATE: u64 = 4000;
/// Without a length hint, peaks are first kept per 0.1 s.
const FINE_WINDOW: usize = (PEAK_RATE / 10) as usize;

/// Reduces a stream of samples to `buckets` peak values without holding the samples.
pub(crate) struct PeakReducer {
    buckets: usize,
    mode: Mode,
}

enum Mode {
    /// Known length: bucket boundaries are fixed up front.
    Hinted { total: u64, seen: u64, peaks: Vec<f32> },
    /// Unknown length: maxima per `FINE_WINDOW`, resampled in `finish`.
    Unhinted { fine: Vec<f32>, in_window: usize },
}

fn level(sample: i16) -> f32 {
    f32::from(sample).abs() / 32768.0
}

impl PeakReducer {
    pub(crate) fn new(total_samples_hint: Option<u64>, buckets: usize) -> Self {
        let mode = match total_samples_hint.filter(|t| *t > 0) {
            Some(total) => Mode::Hinted {
                total,
                seen: 0,
                peaks: vec![0.0; buckets],
            },
            None => Mode::Unhinted {
                fine: Vec::new(),
                in_window: 0,
            },
        };
        Self { buckets, mode }
    }

    pub(crate) fn push(&mut self, samples: &[i16]) {
        if self.buckets == 0 {
            return;
        }
        match &mut self.mode {
            Mode::Hinted { total, seen, peaks } => {
                for &s in samples {
                    let i = (*seen * self.buckets as u64 / *total) as usize;
                    let slot = &mut peaks[i.min(self.buckets - 1)];
                    *slot = slot.max(level(s));
                    *seen += 1;
                }
            }
            Mode::Unhinted { fine, in_window } => {
                for &s in samples {
                    if *in_window == 0 {
                        fine.push(0.0);
                    }
                    if let Some(last) = fine.last_mut() {
                        *last = last.max(level(s));
                    }
                    *in_window = (*in_window + 1) % FINE_WINDOW;
                }
            }
        }
    }

    pub(crate) fn finish(self) -> Vec<f32> {
        match self.mode {
            Mode::Hinted { peaks, .. } => peaks,
            Mode::Unhinted { fine, .. } => {
                let n = fine.len();
                (0..self.buckets)
                    .map(|i| {
                        if n == 0 {
                            return 0.0;
                        }
                        let from = i * n / self.buckets;
                        let to = ((i + 1) * n / self.buckets).clamp(from + 1, n);
                        fine[from..to].iter().copied().fold(0.0, f32::max)
                    })
                    .collect()
            }
        }
    }
}

/// What `peaks` read from a track.
#[derive(Clone, Debug, PartialEq)]
pub struct Waveform {
    /// `buckets` values in `0.0..=1.0`.
    pub peaks: Vec<f32>,
    /// The audio's real length in seconds, from the samples decoded: yt-dlp's `duration` is
    /// rounded to the second (often down), this one is what a cut can reach.
    pub seconds: f64,
}

/// Waveform of `input`. ffmpeg decodes to PCM on a pipe that is folded as it arrives, so an
/// hour-long mix costs a few dozen KB of memory.
pub async fn peaks(tools: &Tools, input: &Path, buckets: usize) -> Result<Waveform> {
    let mut child = tools
        .ffmpeg_command()
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-i"])
        .arg(input)
        .args(["-vn", "-ac", "1", "-ar", "4000", "-f", "s16le", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("không chạy được ffmpeg")?;
    let mut stdout = child.stdout.take().context("ffmpeg không có stdout")?;
    let mut stderr = child.stderr.take().context("ffmpeg không có stderr")?;
    let err_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        buf
    });

    let mut reducer = PeakReducer::new(None, buckets);
    let mut buf = vec![0u8; 64 * 1024];
    // `pending` holds at most one stray byte between reads (a sample split across chunks).
    let mut pending: Vec<u8> = Vec::with_capacity(buf.len() + 1);
    let mut samples: Vec<i16> = Vec::with_capacity(buf.len() / 2 + 1);
    let mut decoded: u64 = 0;
    loop {
        let n = stdout.read(&mut buf).await.context("đọc PCM từ ffmpeg")?;
        if n == 0 {
            break;
        }
        pending.extend_from_slice(&buf[..n]);
        let (pairs, rest) = pending.as_chunks::<2>();
        samples.clear();
        samples.extend(pairs.iter().map(|p| i16::from_le_bytes(*p)));
        reducer.push(&samples);
        decoded += samples.len() as u64;
        let keep = rest.to_vec();
        pending = keep;
    }

    let status = child.wait().await.context("ffmpeg")?;
    let stderr = err_task.await.unwrap_or_default();
    crate::transcode::check_ffmpeg(status, &stderr)?;
    Ok(Waveform {
        peaks: reducer.finish(),
        seconds: decoded as f64 / PEAK_RATE as f64,
    })
}

/// AAC in MP4 for the `<audio>` element; an AAC source is only remuxed.
pub async fn make_preview(tools: &Tools, input: &Path, src_codec: &str, out: &Path) -> Result<()> {
    let mut cmd = tools.ffmpeg_command();
    cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i"])
        .arg(input)
        .args(["-vn", "-map", "0:a:0"]);
    if src_codec.starts_with("mp4a") || src_codec == "aac" {
        cmd.args(["-c:a", "copy"]);
    } else {
        cmd.args(["-c:a", "aac", "-b:a", "128k"]);
    }
    let output = cmd
        .args(["-f", "ipod"])
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .context("không chạy được ffmpeg")?;
    crate::transcode::check_ffmpeg(output.status, &output.stderr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reducer_keeps_the_loudest_sample_per_bucket() {
        let mut r = PeakReducer::new(Some(8), 4);
        r.push(&[0, 16384, -32768, 0]);
        r.push(&[100, -100, 0, 8192]);
        assert_eq!(r.finish(), vec![0.5, 1.0, 100.0 / 32768.0, 0.25]);
    }

    #[test]
    fn reducer_without_hint_is_bounded_and_sized() {
        let mut r = PeakReducer::new(None, 1200);
        let second = vec![1000i16; 4000];
        for _ in 0..3600 {
            r.push(&second);
        }
        let p = r.finish();
        assert_eq!(p.len(), 1200);
        assert!(p.iter().all(|v| (*v - 1000.0 / 32768.0).abs() < 1e-6));
    }

    #[test]
    fn reducer_pads_short_input_with_silence() {
        let mut r = PeakReducer::new(None, 4);
        assert_eq!(r.finish(), vec![0.0; 4]);
        r = PeakReducer::new(Some(100), 3);
        r.push(&[32767]);
        assert_eq!(r.finish().len(), 3);
    }
}
