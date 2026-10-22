//! url2mp3 desktop app: the Tauri side of the UI in `app/src`.

mod jobs;
mod settings;
mod update;
mod ytdlp;

use std::{path::PathBuf, sync::Arc};

use tauri::{Manager, State};
use url2mp3_core::Id;

use crate::{
    jobs::{Jobs, Sidecars, ToolsStatus},
    settings::Settings,
    update::NewVersion,
};

type JobsState<'a> = State<'a, Arc<Jobs>>;

/// Commands fail with a message (or an error code the UI translates, like `ytdlp-missing`).
fn message(e: anyhow::Error) -> String {
    format!("{e:#}")
}

#[tauri::command]
fn get_settings(jobs: JobsState) -> Settings {
    jobs.settings()
}

#[tauri::command]
async fn set_settings(jobs: JobsState<'_>, settings: Settings) -> Result<(), String> {
    jobs.set_settings(settings).await.map_err(message)
}

#[tauri::command]
fn default_out_dir() -> Option<PathBuf> {
    url2mp3_core::default_out_dir()
}

#[tauri::command]
fn tools_status(jobs: JobsState) -> ToolsStatus {
    jobs.tools_status()
}

#[tauri::command]
async fn update_ytdlp(jobs: JobsState<'_>, force: bool) -> Result<ToolsStatus, String> {
    jobs.update_ytdlp(force).await.map_err(message)
}

#[tauri::command]
async fn enqueue(jobs: JobsState<'_>, inputs: Vec<String>) -> Result<usize, String> {
    jobs.enqueue(inputs).await.map_err(message)
}

#[tauri::command]
async fn cancel(jobs: JobsState<'_>, generation: u32, id: Id) -> Result<(), String> {
    jobs.cancel(generation, id).await;
    Ok(())
}

#[tauri::command]
fn reveal(path: PathBuf) -> Result<(), String> {
    tauri_plugin_opener::reveal_item_in_dir(path).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
async fn check_app_update(jobs: JobsState<'_>) -> Result<Option<NewVersion>, String> {
    update::check(jobs.client()).await.map_err(message)
}

/// The last 200 lines of the log, for bug reports.
#[tauri::command]
fn read_log(app: tauri::AppHandle) -> Result<String, String> {
    let dir = app.path().app_log_dir().map_err(|e| e.to_string())?;
    let newest = std::fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
    let Some(file) = newest else {
        return Ok(String::new());
    };
    let text = std::fs::read_to_string(file.path()).map_err(|e| e.to_string())?;
    let lines: Vec<&str> = text.lines().collect();
    Ok(lines[lines.len().saturating_sub(200)..].join("\n"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .max_file_size(1_000_000)
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let paths = app.path();
            let jobs = Jobs::new(
                app.handle().clone(),
                Sidecars::beside_exe()?,
                paths.app_data_dir()?,
                paths.app_config_dir()?,
            );
            app.manage(Arc::new(jobs));
            log::info!("url2mp3 {} started", env!("CARGO_PKG_VERSION"));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_settings,
            default_out_dir,
            tools_status,
            update_ytdlp,
            enqueue,
            cancel,
            reveal,
            open_url,
            check_app_update,
            read_log,
        ])
        .run(tauri::generate_context!())
        .expect("error while running url2mp3");
}
