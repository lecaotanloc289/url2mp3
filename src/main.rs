mod download;
mod resolve;
mod transcode;

use std::{
    collections::HashSet,
    io::{BufRead, IsTerminal},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use indicatif::{HumanBytes, MultiProgress, ProgressBar, ProgressFinish, ProgressStyle};
use tokio::{sync::Semaphore, task::JoinSet};

use crate::{
    download::Downloader,
    resolve::{Info, Ytdlp},
    transcode::{Format, Tags},
};

/// Tải nhạc từ YouTube, TikTok (và mọi trang yt-dlp hỗ trợ): không quảng cáo, tải song song.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Link bài, link playlist, hoặc tên bài để tìm trên YouTube. Bỏ trống để dán link.
    inputs: Vec<String>,

    /// Định dạng xuất
    #[arg(short, long, value_enum, default_value_t = Format::Mp3)]
    format: Format,

    /// Bitrate kbps khi phải nén lại (mặc định: MP3 VBR V0 ~245, M4A 256, Opus 160)
    #[arg(short, long, value_parser = clap::value_parser!(u32).range(32..=320))]
    bitrate: Option<u32>,

    /// Thư mục lưu [mặc định: ~/Downloads]
    #[arg(short, long)]
    out: Option<PathBuf>,

    /// Số bài xử lý cùng lúc [mặc định: số nhân CPU, tối đa 8]
    #[arg(short, long, default_value_t = default_jobs(), hide_default_value = true,
          value_parser = clap::value_parser!(u16).range(1..=32))]
    jobs: u16,

    /// Số kết nối song song cho mỗi bài
    #[arg(short, long, default_value_t = 8, value_parser = clap::value_parser!(u16).range(1..=32))]
    connections: u16,

    /// Đọc thêm link từ file (mỗi dòng một link, dòng bắt đầu bằng # bị bỏ qua)
    #[arg(short = 'a', long, value_name = "FILE")]
    batch_file: Option<PathBuf>,

    /// Link bài nằm trong playlist (watch?v=..&list=..): tải cả playlist thay vì một bài
    #[arg(short, long)]
    playlist: bool,

    /// Tải tối đa N bài đầu của mỗi playlist
    #[arg(short = 'n', long, value_name = "N")]
    limit: Option<usize>,

    /// Không gắn ảnh bìa
    #[arg(long)]
    no_cover: bool,

    /// Tải lại cả bài đã có
    #[arg(long)]
    force: bool,

    /// Dùng cookie của trình duyệt (chrome, safari, firefox, edge…): video giới hạn tuổi, bản 256 kbps của Premium
    #[arg(long, value_name = "BROWSER")]
    cookies_from_browser: Option<String>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let inputs = match collect_inputs(&cli) {
        Ok(v) if v.is_empty() => {
            eprintln!("Không có link nào. Xem: tainhac --help");
            return ExitCode::from(2);
        }
        Ok(v) => v,
        Err(e) => {
            eprintln!("Lỗi: {e:#}");
            return ExitCode::from(2);
        }
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    match rt.block_on(run(cli, inputs)) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Lỗi: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// MP3 encoding is CPU-bound (LAME is single-threaded), so one track per core.
fn default_jobs() -> u16 {
    std::thread::available_parallelism().map_or(4, |n| n.get().clamp(2, 8) as u16)
}

fn collect_inputs(cli: &Cli) -> Result<Vec<String>> {
    let mut items = cli.inputs.clone();
    if let Some(file) = &cli.batch_file {
        let text = std::fs::read_to_string(file).with_context(|| format!("đọc {}", file.display()))?;
        items.extend(text.lines().map(str::to_owned));
    }
    if items.is_empty() {
        let stdin = std::io::stdin();
        if stdin.is_terminal() {
            eprintln!(
                "Dán link YouTube/TikTok hoặc gõ tên bài, mỗi dòng một mục. Enter ở dòng trống để bắt đầu tải:"
            );
            for line in stdin.lock().lines() {
                let line = line?;
                if line.trim().is_empty() {
                    break;
                }
                items.push(line);
            }
        } else {
            items.extend(stdin.lock().lines().map_while(Result::ok));
        }
    }
    let mut seen = HashSet::new();
    Ok(items
        .into_iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
        .filter(|s| seen.insert(s.clone()))
        .collect())
}

struct Ctx {
    ytdlp: Ytdlp,
    dl: Downloader,
    format: Format,
    bitrate: Option<u32>,
    cover: bool,
    force: bool,
    out: PathBuf,
    tmp: PathBuf,
    sem: Semaphore,
    seq: AtomicUsize,
    /// Destinations taken in this run, so two tracks with the same title don't overwrite each other.
    claimed: Mutex<HashSet<String>>,
    multi: MultiProgress,
    bar_style: ProgressStyle,
    spin_style: ProgressStyle,
}

enum Outcome {
    Done(u64),
    Skipped,
    Failed,
}

async fn run(cli: Cli, inputs: Vec<String>) -> Result<ExitCode> {
    let (ytdlp_version, ffmpeg) = tokio::join!(Ytdlp::version(), transcode::version());
    let ytdlp_version = ytdlp_version.map_err(|_| anyhow!("chưa có yt-dlp. Cài: brew install yt-dlp"))?;
    ffmpeg.map_err(|_| anyhow!("chưa có ffmpeg. Cài: brew install ffmpeg"))?;

    let out = match cli.out {
        Some(p) => p,
        None => dirs::download_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
            .context("không tìm thấy thư mục Downloads, dùng -o")?,
    };
    tokio::fs::create_dir_all(&out)
        .await
        .with_context(|| format!("tạo {}", out.display()))?;
    sweep_stale_temp(&out).await;
    // Inside the output dir so the final rename never crosses filesystems.
    let tmp = tempfile::Builder::new().prefix(".tainhac-").tempdir_in(&out)?;

    let ctx = Arc::new(Ctx {
        ytdlp: Ytdlp::new(cli.format.selector(), cli.playlist, cli.limit, cli.cookies_from_browser.as_deref()),
        dl: Downloader::new(cli.connections.into())?,
        format: cli.format,
        bitrate: cli.bitrate,
        cover: !cli.no_cover,
        force: cli.force,
        out: out.clone(),
        tmp: tmp.path().to_owned(),
        sem: Semaphore::new(cli.jobs.into()),
        seq: AtomicUsize::new(0),
        claimed: Mutex::default(),
        multi: MultiProgress::new(),
        bar_style: ProgressStyle::with_template(
            "{spinner:.cyan} [{bar:22.cyan/blue}] {bytes:>9}/{total_bytes:<9} {binary_bytes_per_sec:>11} {wide_msg}",
        )?
        .progress_chars("=> "),
        spin_style: ProgressStyle::with_template("{spinner:.cyan} {wide_msg}")?,
    });

    ctx.println(format!(
        "tainhac · yt-dlp {ytdlp_version} · {} · {} bài cùng lúc × {} kết nối → {}",
        cli.format.ext(),
        cli.jobs,
        cli.connections,
        out.display()
    ));

    let started = Instant::now();
    let mut set = JoinSet::new();
    for input in inputs {
        set.spawn(process_input(ctx.clone(), input));
    }
    let mut outcomes = Vec::new();
    let all_done = async {
        while let Some(res) = set.join_next().await {
            outcomes.extend(res.unwrap_or_default());
        }
    };
    let cancelled = tokio::select! {
        () = all_done => false,
        _ = tokio::signal::ctrl_c() => true,
    };
    if cancelled {
        // Stop every task (and its ffmpeg/yt-dlp child) before the temp dir is removed.
        set.shutdown().await;
        let _ = ctx.multi.clear();
        eprintln!("\nĐã hủy.");
        return Ok(ExitCode::from(130));
    }

    let (mut done, mut bytes, mut skipped, mut failed) = (0, 0, 0, 0);
    for o in &outcomes {
        match o {
            Outcome::Done(n) => (done, bytes) = (done + 1, bytes + n),
            Outcome::Skipped => skipped += 1,
            Outcome::Failed => failed += 1,
        }
    }
    let mut summary = format!(
        "Xong {done} bài · {} · {:.1}s → {}",
        HumanBytes(bytes),
        started.elapsed().as_secs_f64(),
        out.display()
    );
    if skipped > 0 {
        summary += &format!(" · bỏ qua {skipped} bài đã có hoặc trùng");
    }
    if failed > 0 {
        summary += &format!(" · lỗi {failed} bài");
    }
    ctx.println(summary);
    if failed > 0 {
        ctx.println("Gợi ý: lỗi YouTube thường do yt-dlp cũ, cập nhật: brew upgrade yt-dlp");
    }
    drop(tmp);
    Ok(if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// A link, playlist or search query → one or more tracks.
async fn process_input(ctx: Arc<Ctx>, input: String) -> Vec<Outcome> {
    let search = !(input.starts_with("http://") || input.starts_with("https://"));
    let permit = ctx.sem.acquire().await.expect("semaphore never closed");
    let pb = ctx.spinner(format!("đang đọc link: {input}"));
    // Searches are not flattened: yt-dlp resolves the hit in the same call.
    let target = if search {
        format!("ytsearch1:{input}")
    } else {
        input.clone()
    };
    let probed = ctx.ytdlp.probe(&target, !search).await;
    drop(pb);
    let info = match probed {
        Ok(info) => info,
        Err(e) => return vec![ctx.fail(&input, e)],
    };
    if !info.is_playlist() {
        return vec![process_track(&ctx, info, &ctx.out).await];
    }
    drop(permit);

    let entries: Vec<Info> = info.entries.into_iter().flatten().flatten().collect();
    if search && entries.is_empty() {
        return vec![ctx.fail(&input, anyhow!("không tìm thấy bài nào"))];
    }
    let dir = if search {
        ctx.out.clone()
    } else {
        let name = sanitize(info.title.as_deref().unwrap_or("playlist"));
        ctx.println(format!("▸ Playlist {name}: {} bài", entries.len()));
        ctx.out.join(name)
    };
    let mut set = JoinSet::new();
    for entry in entries {
        set.spawn(process_entry(ctx.clone(), entry, dir.clone()));
    }
    set.join_all().await
}

async fn process_entry(ctx: Arc<Ctx>, entry: Info, dir: PathBuf) -> Outcome {
    let _permit = ctx.sem.acquire().await.expect("semaphore never closed");
    if !entry.is_reference() {
        return process_track(&ctx, entry, &dir).await;
    }
    let label = entry
        .title
        .as_deref()
        .or(entry.url.as_deref())
        .unwrap_or("?")
        .to_owned();
    let Some(url) = entry.url.as_deref() else {
        return ctx.fail(&label, anyhow!("playlist thiếu link bài"));
    };
    let pb = ctx.spinner(format!("đang đọc link: {label}"));
    let probed = ctx.ytdlp.probe(url, true).await;
    drop(pb);
    match probed {
        Ok(info) if info.is_playlist() => ctx.fail(
            &label,
            anyhow!("playlist lồng nhau, hãy dán trực tiếp link playlist con"),
        ),
        Ok(info) => process_track(&ctx, info, &dir).await,
        Err(e) => ctx.fail(&label, e),
    }
}

async fn process_track(ctx: &Ctx, info: Info, dir: &Path) -> Outcome {
    let title = info
        .title
        .clone()
        .or_else(|| info.id.clone())
        .unwrap_or_else(|| "untitled".into());
    match track(ctx, &info, &title, dir).await {
        Ok(o) => o,
        Err(e) => ctx.fail(&title, e),
    }
}

async fn track(ctx: &Ctx, info: &Info, title: &str, dir: &Path) -> Result<Outcome> {
    let started = Instant::now();
    let name = sanitize(title);
    let ext = ctx.format.ext();
    // Same video reached twice (plain link + its &list= link, or two playlists).
    if let Some(id) = info.id.as_deref()
        && !ctx.claim(&format!("id:{id}"))
    {
        ctx.println(format!("• trùng bài, bỏ qua: {title}"));
        return Ok(Outcome::Skipped);
    }
    let mut dest = dir.join(format!("{name}.{ext}"));
    if !ctx.claim(&path_key(&dest)) {
        // Same title twice in one run (e.g. two "original sound" TikToks): add the id.
        let id = sanitize(info.id.as_deref().unwrap_or("2"));
        dest = dir.join(format!("{name} [{id}].{ext}"));
        if !ctx.claim(&path_key(&dest)) {
            ctx.println(format!("• trùng bài, bỏ qua: {title}"));
            return Ok(Outcome::Skipped);
        }
    }
    if !ctx.force && tokio::fs::try_exists(&dest).await.unwrap_or(false) {
        ctx.println(format!(
            "• đã có: {} (--force để tải lại)",
            dest.strip_prefix(&ctx.out).unwrap_or(&dest).display()
        ));
        return Ok(Outcome::Skipped);
    }
    tokio::fs::create_dir_all(dir).await?;

    let n = ctx.seq.fetch_add(1, Ordering::Relaxed);
    let audio = ctx.tmp.join(format!("{n}.audio"));
    let cover = ctx.tmp.join(format!("{n}.cover"));
    let encoded = ctx.tmp.join(format!("{n}.{}", ctx.format.ext()));

    let pb = ctx
        .multi
        .add(ProgressBar::new(0).with_finish(ProgressFinish::AndClear));
    pb.set_style(ctx.bar_style.clone());
    pb.set_message(title.to_owned());
    pb.enable_steady_tick(Duration::from_millis(120));

    let thumb = info
        .thumbnail
        .as_deref()
        .filter(|_| ctx.cover && ctx.format.supports_cover());
    let fetch_cover = async {
        match thumb {
            Some(url) => ctx.dl.fetch_small(url, &info.headers(), &cover).await.is_ok(),
            None => false,
        }
    };
    let (audio_res, has_cover) = tokio::join!(fetch_audio(ctx, info, title, &audio, &pb), fetch_cover);
    audio_res?;

    pb.set_style(ctx.spin_style.clone());
    pb.set_message(format!("đang chuyển sang {}: {title}", ctx.format.ext()));
    let job = |with_cover: bool| transcode::Job {
        input: &audio,
        cover: with_cover.then_some(cover.as_path()),
        output: &encoded,
        format: ctx.format,
        src_codec: info.acodec.as_deref().unwrap_or(""),
        bitrate: ctx.bitrate,
        tags: Tags {
            title,
            artist: info.display_artist(),
            album: info.album.as_deref(),
            comment: info.webpage_url.as_deref(),
        },
    };
    let mut encoded_ok = transcode::encode(job(has_cover)).await;
    if encoded_ok.is_err() && has_cover {
        // A thumbnail ffmpeg cannot decode must not cost the track.
        encoded_ok = transcode::encode(job(false)).await;
    }
    encoded_ok?;
    tokio::fs::rename(&encoded, &dest).await?;
    let _ = tokio::join!(tokio::fs::remove_file(&audio), tokio::fs::remove_file(&cover));

    let size = tokio::fs::metadata(&dest).await.map(|m| m.len()).unwrap_or(0);
    drop(pb);
    ctx.println(format!(
        "✓ {}  ({}, {:.1}s)",
        dest.strip_prefix(&ctx.out).unwrap_or(&dest).display(),
        HumanBytes(size),
        started.elapsed().as_secs_f64()
    ));
    Ok(Outcome::Done(size))
}

/// Fast path: our own parallel range download. If the CDN refuses it, let yt-dlp try.
async fn fetch_audio(ctx: &Ctx, info: &Info, title: &str, path: &Path, pb: &ProgressBar) -> Result<()> {
    if let Some(url) = info.direct_url() {
        match ctx
            .dl
            .fetch(url, &info.media_headers(), info.exact_size(), path, pb)
            .await
        {
            Ok(()) => return Ok(()),
            Err(e) => ctx.println(format!(
                "  {title}: tải trực tiếp lỗi ({e:#}), chuyển sang yt-dlp"
            )),
        }
    }
    let page = info
        .webpage_url
        .as_deref()
        .context("không có link gốc để tải lại")?;
    pb.set_style(ctx.spin_style.clone());
    pb.set_message(format!("yt-dlp đang tải: {title}"));
    ctx.ytdlp.download(page, path, ctx.dl.connections).await
}

impl Ctx {
    /// First caller to claim a key (video id or destination file) in this run wins.
    fn claim(&self, key: &str) -> bool {
        self.claimed.lock().expect("claimed lock").insert(key.to_owned())
    }

    fn println(&self, line: impl AsRef<str>) {
        if self.multi.is_hidden() {
            println!("{}", line.as_ref());
        } else {
            let _ = self.multi.println(line);
        }
    }

    fn fail(&self, what: &str, err: anyhow::Error) -> Outcome {
        let msg = format!("{err:#}");
        self.println(format!("✗ {what}: {msg}"));
        Outcome::Failed
    }

    fn spinner(&self, msg: String) -> ProgressBar {
        let pb = self
            .multi
            .add(ProgressBar::new_spinner().with_finish(ProgressFinish::AndClear));
        pb.set_style(self.spin_style.clone());
        pb.set_message(msg);
        pb.enable_steady_tick(Duration::from_millis(120));
        pb
    }
}

/// Remove `.tainhac-*` temp dirs left by a run that was killed or crashed. Only ones older than
/// an hour, so a second tainhac running right now keeps its own.
async fn sweep_stale_temp(out: &Path) {
    let Ok(mut dir) = tokio::fs::read_dir(out).await else {
        return;
    };
    while let Ok(Some(entry)) = dir.next_entry().await {
        if !entry.file_name().to_string_lossy().starts_with(".tainhac-") {
            continue;
        }
        let stale = entry
            .metadata()
            .await
            .ok()
            .filter(|m| m.is_dir())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(3600));
        if stale {
            let _ = tokio::fs::remove_dir_all(entry.path()).await;
        }
    }
}

/// Case-insensitive: APFS and NTFS treat "Track.mp3" and "TRACK.mp3" as one file.
fn path_key(path: &Path) -> String {
    format!("path:{}", path.to_string_lossy().to_lowercase())
}

/// Safe file name on macOS/Windows/Linux, ≤ 180 bytes.
fn sanitize(name: &str) -> String {
    let mapped: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | '*' | '?' | '"' | '<' | '>' => '_',
            ':' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let squeezed = mapped.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = squeezed.trim_matches(|c: char| c == '.' || c.is_whitespace());
    let mut out = String::new();
    for c in trimmed.chars() {
        if out.len() + c.len_utf8() > 180 {
            break;
        }
        out.push(c);
    }
    let out = out.trim_end().to_owned();
    if out.is_empty() { "untitled".into() } else { out }
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn sanitize_names() {
        assert_eq!(
            sanitize("LẠC TRÔI | OFFICIAL MUSIC VIDEO | SƠN TÙNG M-TP"),
            "LẠC TRÔI - OFFICIAL MUSIC VIDEO - SƠN TÙNG M-TP"
        );
        assert_eq!(sanitize("a/b:c?  \n d"), "a_b-c_ d");
        assert_eq!(sanitize("...hidden"), "hidden");
        assert_eq!(sanitize("   "), "untitled");
        assert!(sanitize(&"ạ".repeat(200)).len() <= 180);
    }
}
