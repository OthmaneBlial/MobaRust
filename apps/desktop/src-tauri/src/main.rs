#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ssh;
mod terminal;

use mobarust_core::{AuthMethod, Protocol, SessionId, SessionRecord};
use mobarust_store::SessionStore;
use mobarust_vault::PlatformVault;
use serde::Serialize;
use ssh::{SshAuthRequest, SshConnectRequest, SshManager, SshTransferRequest};
use std::sync::Mutex;
use tauri::Manager;
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

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveSshSessionRequest {
    name: String,
    request: SshConnectRequest,
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
fn session_list(store: State<'_, Mutex<SessionStore>>) -> Result<Vec<SessionRecord>, String> {
    store
        .lock()
        .map_err(|_| "session store lock poisoned".to_owned())
        .map(|store| store.list().to_vec())
}

#[tauri::command]
fn session_save(
    store: State<'_, Mutex<SessionStore>>,
    session: SessionRecord,
) -> Result<SessionRecord, String> {
    store
        .lock()
        .map_err(|_| "session store lock poisoned".to_owned())?
        .save(session)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn session_delete(
    store: State<'_, Mutex<SessionStore>>,
    session_id: SessionId,
) -> Result<bool, String> {
    store
        .lock()
        .map_err(|_| "session store lock poisoned".to_owned())?
        .delete(session_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn session_save_ssh(
    store: State<'_, Mutex<SessionStore>>,
    payload: SaveSshSessionRequest,
) -> Result<SessionRecord, String> {
    let SaveSshSessionRequest { name, request } = payload;
    let SshConnectRequest {
        host,
        port,
        username,
        auth: request_auth,
        known_hosts_path,
        pinned_fingerprint,
        ..
    } = request;
    let auth = match request_auth {
        SshAuthRequest::Agent => AuthMethod::Agent,
        SshAuthRequest::Password { credential_id } if !credential_id.trim().is_empty() => {
            AuthMethod::Password {
                credential_ref: credential_id,
            }
        }
        SshAuthRequest::PrivateKey {
            path,
            passphrase_credential_id,
        } if !path.trim().is_empty() => AuthMethod::PrivateKey {
            key_ref: path,
            credential_ref: passphrase_credential_id,
        },
        _ => return Err("cannot save an SSH session with incomplete authentication".into()),
    };
    let session = SessionRecord {
        id: SessionId::new(),
        name,
        protocol: Protocol::Ssh,
        hostname: host,
        port,
        username: Some(username),
        auth,
        known_hosts_path,
        pinned_fingerprint,
        folder: Some("Remote sessions".into()),
        tags: Vec::new(),
        favorite: false,
        startup_directory: None,
        startup_command: None,
        environment: Vec::new(),
        jump_hosts: Vec::new(),
        notes: None,
    };
    store
        .lock()
        .map_err(|_| "session store lock poisoned".to_owned())?
        .save(session)
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn ssh_connect(
    app: tauri::AppHandle,
    manager: State<'_, SshManager>,
    vault: State<'_, PlatformVault>,
    request: SshConnectRequest,
) -> Result<ssh::SshConnectResponse, String> {
    manager
        .connect(app, &vault, request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn ssh_write(
    manager: State<'_, SshManager>,
    terminal_id: String,
    data: String,
) -> Result<(), String> {
    manager
        .write(&terminal_id, data)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn ssh_resize(
    manager: State<'_, SshManager>,
    terminal_id: String,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    manager
        .resize(&terminal_id, cols, rows)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn ssh_close(manager: State<'_, SshManager>, terminal_id: String) -> Result<(), String> {
    manager
        .close(&terminal_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn ssh_attach(manager: State<'_, SshManager>, terminal_id: String) -> Result<Vec<String>, String> {
    manager
        .attach(&terminal_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn ssh_list_directory(
    manager: State<'_, SshManager>,
    terminal_id: String,
    path: String,
) -> Result<Vec<mobarust_ssh::RemoteEntry>, String> {
    manager
        .list_directory(&terminal_id, path)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn ssh_rename_remote(
    manager: State<'_, SshManager>,
    terminal_id: String,
    from: String,
    to: String,
) -> Result<(), String> {
    manager
        .rename_remote(&terminal_id, from, to)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn ssh_delete_remote(
    manager: State<'_, SshManager>,
    terminal_id: String,
    path: String,
) -> Result<(), String> {
    manager
        .delete_remote(&terminal_id, path)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn ssh_create_remote_directory(
    manager: State<'_, SshManager>,
    terminal_id: String,
    path: String,
) -> Result<(), String> {
    manager
        .create_remote_directory(&terminal_id, path)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn ssh_download(
    app: tauri::AppHandle,
    manager: State<'_, SshManager>,
    terminal_id: String,
    request: SshTransferRequest,
) -> Result<ssh::SshTransferResponse, String> {
    manager
        .start_download(app, terminal_id, request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn ssh_upload(
    app: tauri::AppHandle,
    manager: State<'_, SshManager>,
    terminal_id: String,
    request: SshTransferRequest,
) -> Result<ssh::SshTransferResponse, String> {
    manager
        .start_upload(app, terminal_id, request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn ssh_cancel_transfer(
    manager: State<'_, SshManager>,
    transfer_id: String,
) -> Result<bool, String> {
    manager
        .cancel_transfer(&transfer_id)
        .map_err(|error| error.to_string())
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
        .manage(SshManager::default())
        .manage(PlatformVault::default())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_local_data_dir()
                .map_err(|error| error.to_string())?;
            let mut store = SessionStore::open(data_dir.join("sessions.json"))
                .map_err(|error| error.to_string())?;
            if store.list().is_empty() {
                store
                    .save(SessionRecord::local_terminal("Local workstation"))
                    .map_err(|error| error.to_string())?;
            }
            app.manage(Mutex::new(store));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_snapshot,
            session_list,
            session_save,
            session_save_ssh,
            session_delete,
            ssh_connect,
            ssh_write,
            ssh_resize,
            ssh_close,
            ssh_attach,
            ssh_list_directory,
            ssh_rename_remote,
            ssh_delete_remote,
            ssh_create_remote_directory,
            ssh_download,
            ssh_upload,
            ssh_cancel_transfer,
            terminal_spawn,
            terminal_write,
            terminal_resize,
            terminal_close
        ])
        .run(tauri::generate_context!())
        .expect("error while running MobaRust");
}
