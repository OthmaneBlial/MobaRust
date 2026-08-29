#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod terminal;

use serde::Serialize;
use tauri::State;
use terminal::TerminalManager;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppSnapshot {
    product: &'static str,
    version: &'static str,
    platform: &'static str,
    local_terminal_available: bool,
}

#[tauri::command]
fn app_snapshot() -> AppSnapshot {
    AppSnapshot {
        product: "MobaRust",
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        local_terminal_available: true,
    }
}

#[tauri::command]
fn terminal_spawn(
    app: tauri::AppHandle,
    manager: State<'_, TerminalManager>,
    cols: u16,
    rows: u16,
) -> Result<String, String> {
    manager
        .spawn(app, cols, rows)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn terminal_write(
    manager: State<'_, TerminalManager>,
    terminal_id: String,
    data: String,
) -> Result<(), String> {
    manager
        .write(&terminal_id, data.as_bytes())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn terminal_resize(
    manager: State<'_, TerminalManager>,
    terminal_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    manager
        .resize(&terminal_id, cols, rows)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn terminal_close(manager: State<'_, TerminalManager>, terminal_id: String) -> Result<(), String> {
    manager
        .close(&terminal_id)
        .map_err(|error| error.to_string())
}

fn main() {
    tauri::Builder::default()
        .manage(TerminalManager::default())
        .invoke_handler(tauri::generate_handler![
            app_snapshot,
            terminal_spawn,
            terminal_write,
            terminal_resize,
            terminal_close
        ])
        .run(tauri::generate_context!())
        .expect("error while running MobaRust");
}
