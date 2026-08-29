#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod network;
mod serial;
mod ssh;
mod telnet;
mod terminal;

use mobarust_core::{AppSettings, AuthMethod, Protocol, SerialProfile, SessionId, SessionRecord};
use mobarust_network::{TcpCheckOptions, check_tcp, resolve_host};
use mobarust_store::{OpenSshImportReport, SessionImportReport, SessionStore, SettingsStore};
use mobarust_vault::PlatformVault;
use network::{NetworkManager, NetworkScanRequest};
use serde::Serialize;
use serial::{SerialConnectRequest, SerialManager};
use ssh::{
    SshAuthRequest, SshConnectRequest, SshDynamicForwardRequest, SshLocalForwardRequest,
    SshManager, SshRemoteForwardRequest, SshTransferRequest,
};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::Manager;
use tauri::State;
use telnet::{TelnetConnectRequest, TelnetManager};
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

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveSerialSessionRequest {
    name: String,
    request: SerialConnectRequest,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportOpenSshRequest {
    path: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportSessionRequest {
    json: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct NetworkResolveRequest {
    host: String,
    timeout_ms: u64,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct NetworkTcpCheckRequest {
    host: String,
    port: u16,
    timeout_ms: u64,
}

fn diagnostic_timeout(timeout_ms: u64) -> Result<std::time::Duration, String> {
    if !(50..=60_000).contains(&timeout_ms) {
        return Err("diagnostic timeout must be between 50 and 60000 milliseconds".into());
    }
    Ok(std::time::Duration::from_millis(timeout_ms))
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
fn session_import_openssh(
    store: State<'_, Mutex<SessionStore>>,
    payload: ImportOpenSshRequest,
) -> Result<OpenSshImportReport, String> {
    let path = payload
        .path
        .filter(|path| !path.trim().is_empty())
        .map(|path| expand_user_path(path.trim()))
        .unwrap_or_else(default_openssh_config_path);
    store
        .lock()
        .map_err(|_| "session store lock poisoned".to_owned())?
        .import_openssh_config(path)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn session_export(store: State<'_, Mutex<SessionStore>>) -> Result<String, String> {
    store
        .lock()
        .map_err(|_| "session store lock poisoned".to_owned())?
        .export_json()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn session_import(
    store: State<'_, Mutex<SessionStore>>,
    payload: ImportSessionRequest,
) -> Result<SessionImportReport, String> {
    store
        .lock()
        .map_err(|_| "session store lock poisoned".to_owned())?
        .import_json(&payload.json)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn session_set_favorite(
    store: State<'_, Mutex<SessionStore>>,
    session_id: SessionId,
    favorite: bool,
) -> Result<bool, String> {
    store
        .lock()
        .map_err(|_| "session store lock poisoned".to_owned())?
        .set_favorite(session_id, favorite)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn settings_get(store: State<'_, Mutex<SettingsStore>>) -> Result<AppSettings, String> {
    store
        .lock()
        .map_err(|_| "settings store lock poisoned".to_owned())
        .map(|store| store.get().clone())
}

#[tauri::command]
fn settings_save(
    store: State<'_, Mutex<SettingsStore>>,
    settings: AppSettings,
) -> Result<AppSettings, String> {
    store
        .lock()
        .map_err(|_| "settings store lock poisoned".to_owned())?
        .save(settings)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn settings_reset(store: State<'_, Mutex<SettingsStore>>) -> Result<AppSettings, String> {
    store
        .lock()
        .map_err(|_| "settings store lock poisoned".to_owned())?
        .reset()
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
        jump_hosts,
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
        jump_hosts: jump_hosts.into_iter().map(|jump| jump.host).collect(),
        notes: None,
        serial_profile: None,
    };
    store
        .lock()
        .map_err(|_| "session store lock poisoned".to_owned())?
        .save(session)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn session_save_serial(
    store: State<'_, Mutex<SessionStore>>,
    payload: SaveSerialSessionRequest,
) -> Result<SessionRecord, String> {
    let SaveSerialSessionRequest { name, request } = payload;
    let session = SessionRecord {
        id: SessionId::new(),
        name,
        protocol: Protocol::Serial,
        hostname: request.device.clone(),
        port: 0,
        username: None,
        auth: AuthMethod::None,
        known_hosts_path: None,
        pinned_fingerprint: None,
        folder: Some("Serial devices".into()),
        tags: vec!["serial".into()],
        favorite: false,
        startup_directory: None,
        startup_command: None,
        environment: Vec::new(),
        jump_hosts: Vec::new(),
        notes: None,
        serial_profile: Some(SerialProfile {
            device: request.device,
            baud_rate: request.baud_rate,
            data_bits: format!("{:?}", request.data_bits).to_lowercase(),
            stop_bits: format!("{:?}", request.stop_bits).to_lowercase(),
            parity: format!("{:?}", request.parity).to_lowercase(),
            flow_control: format!("{:?}", request.flow_control).to_lowercase(),
            line_ending: match request.line_ending {
                mobarust_serial::LineEnding::None => "none",
                mobarust_serial::LineEnding::CrLf => "cr-lf",
                mobarust_serial::LineEnding::Cr => "cr",
                mobarust_serial::LineEnding::Lf => "lf",
            }
            .into(),
        }),
    };
    store
        .lock()
        .map_err(|_| "session store lock poisoned".to_owned())?
        .save(session)
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn network_resolve_host(request: NetworkResolveRequest) -> Result<Vec<String>, String> {
    let timeout = diagnostic_timeout(request.timeout_ms)?;
    resolve_host(request.host, timeout)
        .await
        .map(|addresses| {
            addresses
                .into_iter()
                .map(|address| address.to_string())
                .collect()
        })
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn network_check_tcp(
    request: NetworkTcpCheckRequest,
) -> Result<mobarust_network::TcpCheckResult, String> {
    let timeout = diagnostic_timeout(request.timeout_ms)?;
    check_tcp(TcpCheckOptions {
        host: request.host,
        port: request.port,
        timeout,
    })
    .await
    .map_err(|error| error.to_string())
}

#[tauri::command]
async fn network_scan_start(
    app: tauri::AppHandle,
    manager: State<'_, NetworkManager>,
    request: NetworkScanRequest,
) -> Result<network::NetworkScanResponse, String> {
    manager
        .start_scan(app, request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn network_scan_cancel(
    manager: State<'_, NetworkManager>,
    scan_id: String,
) -> Result<bool, String> {
    manager
        .cancel_scan(&scan_id)
        .map_err(|error| error.to_string())
}

fn default_openssh_config_path() -> PathBuf {
    std::env::home_dir()
        .map(|home| home.join(".ssh").join("config"))
        .unwrap_or_else(|| PathBuf::from(".ssh/config"))
}

fn expand_user_path(path: &str) -> PathBuf {
    if path == "~" {
        return std::env::home_dir().unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(relative) = path.strip_prefix("~/")
        && let Some(home) = std::env::home_dir()
    {
        return home.join(relative);
    }
    PathBuf::from(path)
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
async fn ssh_start_local_forward(
    app: tauri::AppHandle,
    manager: State<'_, SshManager>,
    terminal_id: String,
    request: SshLocalForwardRequest,
) -> Result<ssh::SshTunnelResponse, String> {
    manager
        .start_local_forward(app, terminal_id, request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn ssh_start_dynamic_forward(
    app: tauri::AppHandle,
    manager: State<'_, SshManager>,
    terminal_id: String,
    request: SshDynamicForwardRequest,
) -> Result<ssh::SshTunnelResponse, String> {
    manager
        .start_dynamic_forward(app, terminal_id, request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn ssh_start_remote_forward(
    manager: State<'_, SshManager>,
    terminal_id: String,
    request: SshRemoteForwardRequest,
) -> Result<ssh::SshTunnelResponse, String> {
    manager
        .start_remote_forward(terminal_id, request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn ssh_cancel_tunnel(manager: State<'_, SshManager>, tunnel_id: String) -> Result<bool, String> {
    manager
        .cancel_tunnel(&tunnel_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn telnet_connect(
    app: tauri::AppHandle,
    manager: State<'_, TelnetManager>,
    request: TelnetConnectRequest,
) -> Result<telnet::TelnetConnectResponse, String> {
    manager
        .connect(app, request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn telnet_write(
    manager: State<'_, TelnetManager>,
    terminal_id: String,
    data: String,
) -> Result<(), String> {
    manager
        .write(&terminal_id, data)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn telnet_resize(
    manager: State<'_, TelnetManager>,
    terminal_id: String,
    columns: u16,
    rows: u16,
) -> Result<(), String> {
    manager
        .resize(&terminal_id, columns, rows)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn telnet_attach(
    manager: State<'_, TelnetManager>,
    terminal_id: String,
) -> Result<Vec<String>, String> {
    manager
        .attach(&terminal_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn telnet_close(
    manager: State<'_, TelnetManager>,
    terminal_id: String,
) -> Result<(), String> {
    manager
        .close(&terminal_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn serial_connect(
    app: tauri::AppHandle,
    manager: State<'_, SerialManager>,
    request: SerialConnectRequest,
) -> Result<serial::SerialConnectResponse, String> {
    manager
        .connect(app, request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn serial_list_devices() -> Result<Vec<mobarust_serial::SerialDeviceInfo>, String> {
    SerialManager::list_devices()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn serial_write(
    manager: State<'_, SerialManager>,
    terminal_id: String,
    data: String,
) -> Result<(), String> {
    manager
        .write(&terminal_id, data)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn serial_attach(
    manager: State<'_, SerialManager>,
    terminal_id: String,
) -> Result<Vec<String>, String> {
    manager
        .attach(&terminal_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn serial_close(
    manager: State<'_, SerialManager>,
    terminal_id: String,
) -> Result<(), String> {
    manager
        .close(&terminal_id)
        .await
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
        .manage(SerialManager::default())
        .manage(NetworkManager::default())
        .manage(TelnetManager::default())
        .manage(PlatformVault::default())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_local_data_dir()
                .map_err(|error| error.to_string())?;
            let mut store = SessionStore::open(data_dir.join("sessions.json"))
                .map_err(|error| error.to_string())?;
            let settings = SettingsStore::open(data_dir.join("settings.json"))
                .map_err(|error| error.to_string())?;
            if store.list().is_empty() {
                store
                    .save(SessionRecord::local_terminal("Local workstation"))
                    .map_err(|error| error.to_string())?;
            }
            app.manage(Mutex::new(store));
            app.manage(Mutex::new(settings));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_snapshot,
            session_list,
            session_save,
            session_import_openssh,
            session_export,
            session_import,
            session_set_favorite,
            settings_get,
            settings_save,
            settings_reset,
            session_save_ssh,
            session_save_serial,
            session_delete,
            network_resolve_host,
            network_check_tcp,
            network_scan_start,
            network_scan_cancel,
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
            ssh_start_local_forward,
            ssh_start_dynamic_forward,
            ssh_start_remote_forward,
            ssh_cancel_tunnel,
            telnet_connect,
            telnet_write,
            telnet_resize,
            telnet_attach,
            telnet_close,
            serial_connect,
            serial_list_devices,
            serial_write,
            serial_attach,
            serial_close,
            terminal_spawn,
            terminal_write,
            terminal_resize,
            terminal_close
        ])
        .run(tauri::generate_context!())
        .expect("error while running MobaRust");
}
