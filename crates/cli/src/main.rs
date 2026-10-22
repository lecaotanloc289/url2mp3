mod render;

use std::{
    collections::HashSet,
    io::{BufRead, IsTerminal},
    path::PathBuf,
    process::ExitCode,
    sync::Arc,
    time::Instant,
};

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use indicatif::HumanBytes;
use tokio::task::JoinSet;
use url2mp3_core::{Engine, Format, Options, Outcome, Tools};

use crate::render::Render;

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
            eprintln!("Không có link nào. Xem: url2mp3 --help");
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

async fn run(cli: Cli, inputs: Vec<String>) -> Result<ExitCode> {
    let tools = Tools::from_path();
    let (ytdlp_version, ffmpeg) = tokio::join!(tools.ytdlp_version(), tools.check_ffmpeg());
    let ytdlp_version = ytdlp_version.map_err(|_| anyhow!("chưa có yt-dlp. Cài: brew install yt-dlp"))?;
    ffmpeg.map_err(|_| anyhow!("chưa có ffmpeg. Cài: brew install ffmpeg"))?;

    let out = match cli.out {
        Some(p) => p,
        None => url2mp3_core::default_out_dir().context("không tìm thấy thư mục Downloads, dùng -o")?,
    };
    let render = Arc::new(Render::new(cli.format, out.clone())?);
    let sink = {
        let render = render.clone();
        move |event| render.handle(event)
    };
    let opts = Options {
        format: cli.format,
        bitrate: cli.bitrate,
        out: out.clone(),
        jobs: cli.jobs.into(),
        connections: cli.connections.into(),
        playlist: cli.playlist,
        limit: cli.limit,
        cover: !cli.no_cover,
        force: cli.force,
        cookies_from_browser: cli.cookies_from_browser,
    };
    let engine = Engine::start(tools, opts, sink).await?;

    render.println(format!(
        "url2mp3 · yt-dlp {ytdlp_version} · {} · {} bài cùng lúc × {} kết nối → {}",
        cli.format.ext(),
        cli.jobs,
        cli.connections,
        out.display()
    ));

    let started = Instant::now();
    let mut set = JoinSet::new();
    for input in inputs {
        set.spawn(engine.clone().process(engine.next_id(), input));
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
        // Stop every task (and its ffmpeg/yt-dlp child) before the engine removes its temp dir.
        set.shutdown().await;
        render.clear();
        eprintln!("\nĐã hủy.");
        return Ok(ExitCode::from(130));
    }

    let (mut done, mut bytes, mut skipped, mut failed, mut network) = (0, 0, 0, 0, 0);
    for o in &outcomes {
        match o {
            Outcome::Done(n) => (done, bytes) = (done + 1, bytes + n),
            Outcome::Skipped => skipped += 1,
            Outcome::Failed { network: net } => (failed, network) = (failed + 1, network + *net as u32),
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
    render.println(summary);
    if network > 0 {
        render.println(format!(
            "Gợi ý: {network} bài lỗi mạng/DNS (mạng chập chờn, VPN/WARP), không phải lỗi YouTube."
        ));
    }
    if failed > network {
        render.println("Gợi ý: lỗi YouTube thường do yt-dlp cũ, cập nhật: brew upgrade yt-dlp");
    }
    if failed > 0 {
        render.println("Chạy lại đúng lệnh này để tải các bài lỗi: bài đã tải xong sẽ tự bỏ qua.");
    }
    Ok(if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
