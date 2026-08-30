#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod network;
mod serial;
mod ssh;
mod telnet;
mod terminal;

use mobarust_core::{
    AppSettings, AuthMethod, JumpHostRecord, Protocol, SerialProfile, SessionId, SessionRecord,
};
use mobarust_network::{TcpCheckOptions, check_tcp, resolve_host};
use mobarust_ssh::{SshFingerprintOptions, inspect_host_key};
use mobarust_store::{
    MacroStore, OpenSshImportReport, SessionImportReport, SessionStore, SettingsStore, SnippetStore,
};
use mobarust_vault::{
    CredentialId, CredentialLookup, PlatformVault, PortableVault, SecretMaterial, VaultError,
};
use network::{NetworkManager, NetworkScanRequest};
use serde::Serialize;
use serial::{SerialConnectRequest, SerialManager};
use ssh::{
    SshAuthRequest, SshConnectRequest, SshDynamicForwardRequest, SshLocalForwardRequest,
    SshManager, SshRemoteForwardRequest, SshTransferRequest,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::Manager;
use tauri::State;
use telnet::{TelnetConnectRequest, TelnetManager};
use terminal::TerminalManager;
use zeroize::Zeroizing;

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

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct NetworkTracerouteRequest {
    host: String,
    timeout_ms: u64,
    max_hops: u8,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SshFingerprintRequest {
    host: String,
    port: u16,
    timeout_ms: u64,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct VaultPutRequest {
    credential_id: String,
    secret: Zeroizing<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct VaultCredentialRequest {
    credential_id: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PortableVaultPassphraseRequest {
    passphrase: Zeroizing<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PortableVaultPutRequest {
    credential_id: String,
    secret: Zeroizing<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PortableVaultStatus {
    enabled: bool,
    unlocked: bool,
    exists: bool,
    path: String,
}

#[derive(Clone)]
struct PortableVaultState {
    enabled: bool,
    path: PathBuf,
    vault: Arc<Mutex<Option<PortableVault>>>,
}

#[derive(Clone)]
struct CredentialResolver {
    platform: PlatformVault,
    portable: Arc<Mutex<Option<PortableVault>>>,
}

impl CredentialLookup for CredentialResolver {
    fn get(&self, credential_id: &CredentialId) -> Result<SecretMaterial, VaultError> {
        let portable = self.portable.lock().map_err(|_| {
            VaultError::PortableStateUnavailable("credential lookup lock poisoned".into())
        })?;
        if let Some(vault) = portable.as_ref() {
            match vault.get(credential_id) {
                Ok(secret) => return Ok(secret),
                Err(VaultError::PortableCredentialMissing(_)) => {}
                Err(error) => return Err(error),
            }
        }
        self.platform.get(credential_id)
    }
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

/// Store a secret only after an explicit user action. The secret is never
/// returned to the renderer and is owned by a zeroizing native wrapper while
/// the platform keyring call is in progress.
#[tauri::command]
fn vault_put(vault: State<'_, PlatformVault>, payload: VaultPutRequest) -> Result<String, String> {
    let credential_id =
        CredentialId::new(payload.credential_id).map_err(|error| error.to_string())?;
    if payload.secret.is_empty() {
        return Err("vault secret cannot be empty".into());
    }
    let secret = SecretMaterial::from_zeroizing(payload.secret);
    vault
        .put(&credential_id, &secret)
        .map_err(|error| error.to_string())?;
    Ok(credential_id.to_string())
}

/// Delete is idempotent in the native vault and returns only the validated
/// opaque reference, never credential material.
#[tauri::command]
fn vault_delete(
    vault: State<'_, PlatformVault>,
    payload: VaultCredentialRequest,
) -> Result<String, String> {
    let credential_id =
        CredentialId::new(payload.credential_id).map_err(|error| error.to_string())?;
    vault
        .delete(&credential_id)
        .map_err(|error| error.to_string())?;
    Ok(credential_id.to_string())
}

fn portable_vault_status_for(state: &PortableVaultState) -> Result<PortableVaultStatus, String> {
    let vault = state
        .vault
        .lock()
        .map_err(|_| "portable vault state lock poisoned".to_owned())?;
    Ok(PortableVaultStatus {
        enabled: state.enabled,
        unlocked: vault.is_some(),
        exists: state.path.is_file(),
        path: state.path.display().to_string(),
    })
}

#[tauri::command]
fn portable_vault_status(
    state: State<'_, PortableVaultState>,
) -> Result<PortableVaultStatus, String> {
    portable_vault_status_for(state.inner())
}

#[tauri::command]
fn portable_vault_create(
    state: State<'_, PortableVaultState>,
    payload: PortableVaultPassphraseRequest,
) -> Result<PortableVaultStatus, String> {
    if !state.enabled {
        return Err("portable mode is disabled; place portable.flag beside the executable".into());
    }
    let passphrase = SecretMaterial::from_zeroizing(payload.passphrase);
    let vault =
        PortableVault::create(&state.path, &passphrase).map_err(|error| error.to_string())?;
    let mut current = state
        .vault
        .lock()
        .map_err(|_| "portable vault state lock poisoned".to_owned())?;
    *current = Some(vault);
    drop(current);
    portable_vault_status_for(state.inner())
}

#[tauri::command]
fn portable_vault_unlock(
    state: State<'_, PortableVaultState>,
    payload: PortableVaultPassphraseRequest,
) -> Result<PortableVaultStatus, String> {
    if !state.enabled {
        return Err("portable mode is disabled; place portable.flag beside the executable".into());
    }
    let passphrase = SecretMaterial::from_zeroizing(payload.passphrase);
    let vault = PortableVault::open(&state.path, &passphrase).map_err(|error| error.to_string())?;
    let mut current = state
        .vault
        .lock()
        .map_err(|_| "portable vault state lock poisoned".to_owned())?;
    *current = Some(vault);
    drop(current);
    portable_vault_status_for(state.inner())
}

#[tauri::command]
fn portable_vault_lock(
    state: State<'_, PortableVaultState>,
) -> Result<PortableVaultStatus, String> {
    let mut current = state
        .vault
        .lock()
        .map_err(|_| "portable vault state lock poisoned".to_owned())?;
    current.take();
    drop(current);
    portable_vault_status_for(state.inner())
}

#[tauri::command]
fn portable_vault_list(state: State<'_, PortableVaultState>) -> Result<Vec<String>, String> {
    let current = state
        .vault
        .lock()
        .map_err(|_| "portable vault state lock poisoned".to_owned())?;
    current
        .as_ref()
        .map(PortableVault::list_ids)
        .ok_or_else(|| "portable vault is locked".to_owned())
}

#[tauri::command]
fn portable_vault_put(
    state: State<'_, PortableVaultState>,
    payload: PortableVaultPutRequest,
) -> Result<String, String> {
    let credential_id =
        CredentialId::new(payload.credential_id).map_err(|error| error.to_string())?;
    let secret = SecretMaterial::from_zeroizing(payload.secret);
    let mut current = state
        .vault
        .lock()
        .map_err(|_| "portable vault state lock poisoned".to_owned())?;
    let vault = current
        .as_mut()
        .ok_or_else(|| "portable vault is locked".to_owned())?;
    vault
        .put(&credential_id, secret)
        .map_err(|error| error.to_string())?;
    Ok(credential_id.to_string())
}

#[tauri::command]
fn portable_vault_delete(
    state: State<'_, PortableVaultState>,
    payload: VaultCredentialRequest,
) -> Result<String, String> {
    let credential_id =
        CredentialId::new(payload.credential_id).map_err(|error| error.to_string())?;
    let mut current = state
        .vault
        .lock()
        .map_err(|_| "portable vault state lock poisoned".to_owned())?;
    let vault = current
        .as_mut()
        .ok_or_else(|| "portable vault is locked".to_owned())?;
    vault
        .delete(&credential_id)
        .map_err(|error| error.to_string())?;
    Ok(credential_id.to_string())
}

#[tauri::command]
fn snippet_list(
    store: State<'_, Mutex<SnippetStore>>,
) -> Result<Vec<mobarust_core::SnippetRecord>, String> {
    store
        .lock()
        .map_err(|_| "snippet store lock poisoned".to_owned())
        .map(|store| store.list().to_vec())
}

#[tauri::command]
fn snippet_save(
    store: State<'_, Mutex<SnippetStore>>,
    snippet: mobarust_core::SnippetRecord,
) -> Result<mobarust_core::SnippetRecord, String> {
    store
        .lock()
        .map_err(|_| "snippet store lock poisoned".to_owned())?
        .save(snippet)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn snippet_delete(
    store: State<'_, Mutex<SnippetStore>>,
    snippet_id: uuid::Uuid,
) -> Result<bool, String> {
    store
        .lock()
        .map_err(|_| "snippet store lock poisoned".to_owned())?
        .delete(snippet_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn macro_list(
    store: State<'_, Mutex<MacroStore>>,
) -> Result<Vec<mobarust_core::MacroRecord>, String> {
    store
        .lock()
        .map_err(|_| "macro store lock poisoned".to_owned())
        .map(|store| store.list().to_vec())
}

#[tauri::command]
fn macro_save(
    store: State<'_, Mutex<MacroStore>>,
    record: mobarust_core::MacroRecord,
) -> Result<mobarust_core::MacroRecord, String> {
    store
        .lock()
        .map_err(|_| "macro store lock poisoned".to_owned())?
        .save(record)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn macro_delete(store: State<'_, Mutex<MacroStore>>, macro_id: uuid::Uuid) -> Result<bool, String> {
    store
        .lock()
        .map_err(|_| "macro store lock poisoned".to_owned())?
        .delete(macro_id)
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
        SshAuthRequest::KeyboardInteractive { credential_id }
            if !credential_id.trim().is_empty() =>
        {
            AuthMethod::KeyboardInteractive {
                credential_ref: credential_id,
            }
        }
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
        jump_hosts: jump_hosts.iter().map(|jump| jump.host.clone()).collect(),
        jump_host_profiles: jump_hosts
            .into_iter()
            .map(|jump| {
                let auth = match jump.auth {
                    SshAuthRequest::Agent => Ok(AuthMethod::Agent),
                    SshAuthRequest::Password { credential_id }
                        if !credential_id.trim().is_empty() =>
                    {
                        Ok(AuthMethod::Password {
                            credential_ref: credential_id,
                        })
                    }
                    SshAuthRequest::PrivateKey {
                        path,
                        passphrase_credential_id,
                    } if !path.trim().is_empty() => Ok(AuthMethod::PrivateKey {
                        key_ref: path,
                        credential_ref: passphrase_credential_id,
                    }),
                    SshAuthRequest::KeyboardInteractive { credential_id }
                        if !credential_id.trim().is_empty() =>
                    {
                        Ok(AuthMethod::KeyboardInteractive {
                            credential_ref: credential_id,
                        })
                    }
                    _ => Err("cannot save a jump host with incomplete authentication".to_owned()),
                }?;
                Ok(JumpHostRecord {
                    host: jump.host,
                    port: jump.port,
                    username: jump.username,
                    auth,
                    known_hosts_path: jump.known_hosts_path,
                    pinned_fingerprint: jump.pinned_fingerprint,
                })
            })
            .collect::<Result<Vec<_>, String>>()?,
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
        jump_host_profiles: Vec::new(),
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
async fn ssh_inspect_host_key(
    request: SshFingerprintRequest,
) -> Result<mobarust_ssh::SshHostKeyInspection, String> {
    let timeout = diagnostic_timeout(request.timeout_ms)?;
    inspect_host_key(SshFingerprintOptions {
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
async fn network_ping_start(
    app: tauri::AppHandle,
    manager: State<'_, NetworkManager>,
    request: NetworkResolveRequest,
) -> Result<network::NetworkDiagnosticResponse, String> {
    manager
        .start_ping(app, request.host, request.timeout_ms)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn network_traceroute_start(
    app: tauri::AppHandle,
    manager: State<'_, NetworkManager>,
    request: NetworkTracerouteRequest,
) -> Result<network::NetworkDiagnosticResponse, String> {
    manager
        .start_traceroute(app, request.host, request.timeout_ms, request.max_hops)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn network_diagnostic_cancel(
    manager: State<'_, NetworkManager>,
    operation_id: String,
) -> Result<bool, String> {
    manager
        .cancel_diagnostic(&operation_id)
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
    vault: State<'_, CredentialResolver>,
    request: SshConnectRequest,
) -> Result<ssh::SshConnectResponse, String> {
    let vault: Arc<dyn CredentialLookup> = Arc::new(vault.inner().clone());
    manager
        .connect(app, vault, request)
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
async fn ssh_open_remote_text_file(
    manager: State<'_, SshManager>,
    terminal_id: String,
    path: String,
) -> Result<mobarust_ssh::RemoteTextDocument, String> {
    manager
        .open_remote_text_file(&terminal_id, path)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn ssh_collect_remote_monitor(
    manager: State<'_, SshManager>,
    terminal_id: String,
) -> Result<mobarust_ssh::RemoteMonitorSnapshot, String> {
    manager
        .collect_remote_monitor(&terminal_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn ssh_save_remote_text_file(
    manager: State<'_, SshManager>,
    terminal_id: String,
    path: String,
    expected_revision: String,
    content: String,
    encoding: mobarust_ssh::RemoteTextEncoding,
) -> Result<mobarust_ssh::RemoteTextDocument, String> {
    manager
        .save_remote_text_file(&terminal_id, path, expected_revision, content, encoding)
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

fn detect_portable_data_dir() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let parent = executable.parent()?;
    parent
        .join("portable.flag")
        .is_file()
        .then(|| parent.join("portable-data"))
}

fn main() {
    tauri::Builder::default()
        .manage(TerminalManager::default())
        .manage(SshManager::default())
        .manage(SerialManager::default())
        .manage(NetworkManager::default())
        .manage(TelnetManager::default())
        .setup(|app| {
            let portable_dir = detect_portable_data_dir();
            let (data_dir, portable_enabled) = match portable_dir {
                Some(path) => (path, true),
                None => (
                    app.path()
                        .app_local_data_dir()
                        .map_err(|error| error.to_string())?,
                    false,
                ),
            };
            let platform_vault = PlatformVault::default();
            let portable_vault = Arc::new(Mutex::new(None));
            app.manage(platform_vault.clone());
            app.manage(CredentialResolver {
                platform: platform_vault,
                portable: Arc::clone(&portable_vault),
            });
            app.manage(PortableVaultState {
                enabled: portable_enabled,
                path: data_dir.join("vault.bin"),
                vault: portable_vault,
            });
            let mut store = SessionStore::open(data_dir.join("sessions.json"))
                .map_err(|error| error.to_string())?;
            let settings = SettingsStore::open(data_dir.join("settings.json"))
                .map_err(|error| error.to_string())?;
            let snippets = SnippetStore::open(data_dir.join("snippets.json"))
                .map_err(|error| error.to_string())?;
            let macros = MacroStore::open(data_dir.join("macros.json"))
                .map_err(|error| error.to_string())?;
            if store.list().is_empty() {
                store
                    .save(SessionRecord::local_terminal("Local workstation"))
                    .map_err(|error| error.to_string())?;
            }
            app.manage(Mutex::new(store));
            app.manage(Mutex::new(settings));
            app.manage(Mutex::new(snippets));
            app.manage(Mutex::new(macros));
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
            vault_put,
            vault_delete,
            portable_vault_status,
            portable_vault_create,
            portable_vault_unlock,
            portable_vault_lock,
            portable_vault_list,
            portable_vault_put,
            portable_vault_delete,
            snippet_list,
            snippet_save,
            snippet_delete,
            macro_list,
            macro_save,
            macro_delete,
            session_save_ssh,
            session_save_serial,
            session_delete,
            network_resolve_host,
            network_check_tcp,
            ssh_inspect_host_key,
            network_ping_start,
            network_traceroute_start,
            network_diagnostic_cancel,
            network_scan_start,
            network_scan_cancel,
            ssh_connect,
            ssh_write,
            ssh_resize,
            ssh_close,
            ssh_attach,
            ssh_list_directory,
            ssh_open_remote_text_file,
            ssh_collect_remote_monitor,
            ssh_save_remote_text_file,
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
