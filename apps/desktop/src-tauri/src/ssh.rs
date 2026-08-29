use mobarust_core::{TransferEvent, TransferLifecycle, TransferState};
use mobarust_ssh::{
    HostKeyPolicy, SshConnectOptions, SshConnection, SshCredentials, SshError, SshOutput,
};
use mobarust_vault::{CredentialId, PlatformVault, VaultError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use thiserror::Error;
use tokio::fs::{self, OpenOptions};
use tokio::io::copy_bidirectional;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tokio::task::JoinSet;
use uuid::Uuid;

const COMMAND_CAPACITY: usize = 64;
const OUTPUT_BUFFER_BYTES: usize = 32 * 1024;
const PENDING_OUTPUT_CHUNKS: usize = 32;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshConnectRequest {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: SshAuthRequest,
    #[serde(default)]
    pub known_hosts_path: Option<String>,
    #[serde(default)]
    pub pinned_fingerprint: Option<String>,
    #[serde(default)]
    pub jump_hosts: Vec<SshJumpHostRequest>,
    #[serde(default = "default_terminal_cols")]
    pub cols: u32,
    #[serde(default = "default_terminal_rows")]
    pub rows: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum SshSessionState {
    Reconnecting,
    Connected,
    Failed,
    Disconnected,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SshSessionEvent {
    terminal_id: String,
    state: SshSessionState,
    attempt: u8,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshJumpHostRequest {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: SshAuthRequest,
    #[serde(default)]
    pub known_hosts_path: Option<String>,
    #[serde(default)]
    pub pinned_fingerprint: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "method", rename_all = "camelCase")]
pub enum SshAuthRequest {
    Agent,
    Password {
        #[serde(rename = "credentialId", alias = "credential_id")]
        credential_id: String,
    },
    PrivateKey {
        path: String,
        #[serde(default)]
        #[serde(rename = "passphraseCredentialId", alias = "passphrase_credential_id")]
        passphrase_credential_id: Option<String>,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshConnectResponse {
    pub terminal_id: String,
    pub host: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshTransferRequest {
    pub remote_path: String,
    pub local_path: String,
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshTransferResponse {
    pub transfer_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshLocalForwardRequest {
    #[serde(default = "default_bind_host")]
    pub bind_host: String,
    pub bind_port: u16,
    pub target_host: String,
    pub target_port: u16,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshTunnelResponse {
    pub tunnel_id: String,
    pub bind_host: String,
    pub bind_port: u16,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum TunnelState {
    Listening,
    Running,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SshTunnelEvent {
    tunnel_id: String,
    terminal_id: String,
    local_host: String,
    local_port: u16,
    target_host: String,
    target_port: u16,
    state: TunnelState,
    connections: usize,
    bytes_forwarded: u64,
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum TransferDirection {
    Download,
    Upload,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SshTransferEvent {
    transfer_id: String,
    terminal_id: String,
    direction: TransferDirection,
    source: String,
    destination: String,
    bytes_transferred: u64,
    total_bytes: Option<u64>,
    state: TransferState,
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SshOutputEvent {
    terminal_id: String,
    data: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SshClosedEvent {
    terminal_id: String,
    reason: String,
}

enum SshCommand {
    Write(Vec<u8>),
    Resize {
        cols: u32,
        rows: u32,
    },
    ListDirectory {
        path: String,
        reply: oneshot::Sender<Result<Vec<mobarust_ssh::RemoteEntry>, String>>,
    },
    FileOperation {
        operation: SshFileOperation,
        reply: oneshot::Sender<Result<(), String>>,
    },
    StartLocalForward {
        job: LocalForwardJob,
    },
    StartTransfer {
        job: TransferJob,
    },
    Close,
}

enum SshFileOperation {
    Rename { from: String, to: String },
    Delete { path: String },
    CreateDirectory { path: String },
}

#[derive(Debug, Error)]
pub enum SshManagerError {
    #[error("SSH session is not found: {0}")]
    MissingSession(String),
    #[error("SSH session command queue is closed")]
    Closed,
    #[error("invalid SSH request: {0}")]
    InvalidRequest(String),
    #[error(transparent)]
    Transport(#[from] SshError),
    #[error(transparent)]
    Vault(#[from] VaultError),
}

#[derive(Clone)]
pub struct SshManager {
    sessions: Arc<Mutex<HashMap<String, SessionState>>>,
    transfers: Arc<Mutex<HashMap<String, TransferControl>>>,
    tunnels: Arc<Mutex<HashMap<String, TunnelControl>>>,
    transfer_slots: Arc<Semaphore>,
}

impl Default for SshManager {
    fn default() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            transfers: Arc::new(Mutex::new(HashMap::new())),
            tunnels: Arc::new(Mutex::new(HashMap::new())),
            transfer_slots: Arc::new(Semaphore::new(3)),
        }
    }
}

struct SessionState {
    sender: mpsc::Sender<SshCommand>,
    attached: bool,
    pending_output: Vec<String>,
}

struct RemoteSessionContext {
    app: AppHandle,
    manager: SshManager,
    terminal_id: String,
    request: SshConnectRequest,
    vault: PlatformVault,
}

struct TransferControl {
    terminal_id: String,
    cancel: oneshot::Sender<()>,
}

struct TunnelControl {
    terminal_id: String,
    cancel: watch::Sender<bool>,
}

struct LocalForwardJob {
    tunnel_id: String,
    terminal_id: String,
    bind_host: String,
    bind_port: u16,
    target_host: String,
    target_port: u16,
    listener: TcpListener,
    cancel: watch::Receiver<bool>,
}

impl LocalForwardJob {
    fn event(
        &self,
        state: TunnelState,
        connections: usize,
        bytes_forwarded: u64,
        error: Option<String>,
    ) -> SshTunnelEvent {
        SshTunnelEvent {
            tunnel_id: self.tunnel_id.clone(),
            terminal_id: self.terminal_id.clone(),
            local_host: self.bind_host.clone(),
            local_port: self.bind_port,
            target_host: self.target_host.clone(),
            target_port: self.target_port,
            state,
            connections,
            bytes_forwarded,
            error,
        }
    }
}

struct TransferJob {
    transfer_id: String,
    terminal_id: String,
    direction: TransferDirection,
    remote_path: String,
    local_path: PathBuf,
    overwrite: bool,
    cancel: Option<oneshot::Receiver<()>>,
    source: String,
    destination: String,
}

impl TransferJob {
    fn event(
        &self,
        bytes_transferred: u64,
        total_bytes: Option<u64>,
        state: TransferState,
        error: Option<String>,
    ) -> SshTransferEvent {
        SshTransferEvent {
            transfer_id: self.transfer_id.clone(),
            terminal_id: self.terminal_id.clone(),
            direction: self.direction.clone(),
            source: self.source.clone(),
            destination: self.destination.clone(),
            bytes_transferred,
            total_bytes,
            state,
            error,
        }
    }
}

impl SshManager {
    pub async fn connect(
        &self,
        app: AppHandle,
        vault: &PlatformVault,
        request: SshConnectRequest,
    ) -> Result<SshConnectResponse, SshManagerError> {
        let host = request.host.clone();
        let connection = connect_transport(vault, &request).await?;
        let connection = Arc::new(connection);
        let shell = connection.open_shell(request.cols, request.rows).await?;
        let (reader, writer) = shell.split();
        let terminal_id = Uuid::new_v4().to_string();
        let (sender, receiver) = mpsc::channel(COMMAND_CAPACITY);
        self.sessions
            .lock()
            .map_err(|_| SshManagerError::Closed)?
            .insert(
                terminal_id.clone(),
                SessionState {
                    sender,
                    attached: false,
                    pending_output: Vec::new(),
                },
            );

        let manager = self.clone();
        let id_for_task = terminal_id.clone();
        let reconnect_vault = vault.clone();
        let context = RemoteSessionContext {
            app,
            manager,
            terminal_id: id_for_task,
            request,
            vault: reconnect_vault,
        };
        tauri::async_runtime::spawn(async move {
            run_remote_session(context, connection, reader, writer, receiver).await;
        });

        Ok(SshConnectResponse { terminal_id, host })
    }

    pub async fn write(&self, terminal_id: &str, data: String) -> Result<(), SshManagerError> {
        self.sender(terminal_id)?
            .send(SshCommand::Write(data.into_bytes()))
            .await
            .map_err(|_| SshManagerError::Closed)
    }

    pub async fn resize(
        &self,
        terminal_id: &str,
        cols: u32,
        rows: u32,
    ) -> Result<(), SshManagerError> {
        self.sender(terminal_id)?
            .send(SshCommand::Resize { cols, rows })
            .await
            .map_err(|_| SshManagerError::Closed)
    }

    pub async fn close(&self, terminal_id: &str) -> Result<(), SshManagerError> {
        self.sender(terminal_id)?
            .send(SshCommand::Close)
            .await
            .map_err(|_| SshManagerError::Closed)
    }

    pub fn attach(&self, terminal_id: &str) -> Result<Vec<String>, SshManagerError> {
        let mut sessions = self.sessions.lock().map_err(|_| SshManagerError::Closed)?;
        let state = sessions
            .get_mut(terminal_id)
            .ok_or_else(|| SshManagerError::MissingSession(terminal_id.to_owned()))?;
        state.attached = true;
        Ok(std::mem::take(&mut state.pending_output))
    }

    pub async fn list_directory(
        &self,
        terminal_id: &str,
        path: String,
    ) -> Result<Vec<mobarust_ssh::RemoteEntry>, SshManagerError> {
        let path = validate_remote_directory_path(&path)?;
        let (reply, response) = oneshot::channel();
        self.sender(terminal_id)?
            .send(SshCommand::ListDirectory { path, reply })
            .await
            .map_err(|_| SshManagerError::Closed)?;
        response
            .await
            .map_err(|_| SshManagerError::Closed)?
            .map_err(SshManagerError::InvalidRequest)
    }

    pub async fn rename_remote(
        &self,
        terminal_id: &str,
        from: String,
        to: String,
    ) -> Result<(), SshManagerError> {
        let from = validate_remote_mutation_path(&from)?;
        let to = validate_remote_mutation_path(&to)?;
        self.run_file_operation(terminal_id, SshFileOperation::Rename { from, to })
            .await
    }

    pub async fn delete_remote(
        &self,
        terminal_id: &str,
        path: String,
    ) -> Result<(), SshManagerError> {
        let path = validate_remote_mutation_path(&path)?;
        self.run_file_operation(terminal_id, SshFileOperation::Delete { path })
            .await
    }

    pub async fn create_remote_directory(
        &self,
        terminal_id: &str,
        path: String,
    ) -> Result<(), SshManagerError> {
        let path = validate_remote_mutation_path(&path)?;
        self.run_file_operation(terminal_id, SshFileOperation::CreateDirectory { path })
            .await
    }

    async fn run_file_operation(
        &self,
        terminal_id: &str,
        operation: SshFileOperation,
    ) -> Result<(), SshManagerError> {
        let (reply, response) = oneshot::channel();
        self.sender(terminal_id)?
            .send(SshCommand::FileOperation { operation, reply })
            .await
            .map_err(|_| SshManagerError::Closed)?;
        response
            .await
            .map_err(|_| SshManagerError::Closed)?
            .map_err(SshManagerError::InvalidRequest)
    }

    pub async fn start_local_forward(
        &self,
        app: AppHandle,
        terminal_id: String,
        request: SshLocalForwardRequest,
    ) -> Result<SshTunnelResponse, SshManagerError> {
        let sender = self.sender(&terminal_id)?;
        let bind_host = if request.bind_host.trim().is_empty() {
            default_bind_host().to_owned()
        } else {
            request.bind_host.trim().to_owned()
        };
        let target_host = request.target_host.trim().to_owned();
        if target_host.is_empty() || target_host.contains('\0') || request.target_port == 0 {
            return Err(SshManagerError::InvalidRequest(
                "tunnel target host and port are required".into(),
            ));
        }
        if bind_host.contains('\0') {
            return Err(SshManagerError::InvalidRequest(
                "tunnel bind host cannot contain NUL".into(),
            ));
        }
        let listener = TcpListener::bind((bind_host.as_str(), request.bind_port))
            .await
            .map_err(|error| {
                SshManagerError::InvalidRequest(format!(
                    "could not bind local tunnel listener: {error}"
                ))
            })?;
        let local_port = listener
            .local_addr()
            .map_err(|error| SshManagerError::InvalidRequest(error.to_string()))?
            .port();
        let tunnel_id = Uuid::new_v4().to_string();
        let (cancel, cancel_receiver) = watch::channel(false);
        self.tunnels
            .lock()
            .map_err(|_| SshManagerError::Closed)?
            .insert(
                tunnel_id.clone(),
                TunnelControl {
                    terminal_id: terminal_id.clone(),
                    cancel,
                },
            );
        self.emit_tunnel(
            &app,
            SshTunnelEvent {
                tunnel_id: tunnel_id.clone(),
                terminal_id: terminal_id.clone(),
                local_host: bind_host.clone(),
                local_port,
                target_host: target_host.clone(),
                target_port: request.target_port,
                state: TunnelState::Listening,
                connections: 0,
                bytes_forwarded: 0,
                error: None,
            },
        );
        let command = SshCommand::StartLocalForward {
            job: LocalForwardJob {
                tunnel_id: tunnel_id.clone(),
                terminal_id: terminal_id.clone(),
                bind_host: bind_host.clone(),
                bind_port: local_port,
                target_host: target_host.clone(),
                target_port: request.target_port,
                listener,
                cancel: cancel_receiver,
            },
        };
        if sender.send(command).await.is_err() {
            self.finish_tunnel(&tunnel_id);
            return Err(SshManagerError::Closed);
        }
        Ok(SshTunnelResponse {
            tunnel_id,
            bind_host,
            bind_port: local_port,
        })
    }

    pub fn cancel_tunnel(&self, tunnel_id: &str) -> Result<bool, SshManagerError> {
        let control = self
            .tunnels
            .lock()
            .map_err(|_| SshManagerError::Closed)?
            .remove(tunnel_id);
        if let Some(control) = control {
            let _ = control.cancel.send(true);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn start_download(
        &self,
        app: AppHandle,
        terminal_id: String,
        request: SshTransferRequest,
    ) -> Result<SshTransferResponse, SshManagerError> {
        self.start_transfer(app, terminal_id, TransferDirection::Download, request)
            .await
    }

    pub async fn start_upload(
        &self,
        app: AppHandle,
        terminal_id: String,
        request: SshTransferRequest,
    ) -> Result<SshTransferResponse, SshManagerError> {
        self.start_transfer(app, terminal_id, TransferDirection::Upload, request)
            .await
    }

    async fn start_transfer(
        &self,
        app: AppHandle,
        terminal_id: String,
        direction: TransferDirection,
        request: SshTransferRequest,
    ) -> Result<SshTransferResponse, SshManagerError> {
        let sender = self.sender(&terminal_id)?;
        let remote_path = validate_remote_file_path(&request.remote_path)?;
        let local_path = validate_local_file_path(&request.local_path)?;
        let transfer_id = Uuid::new_v4().to_string();
        let (cancel, cancel_receiver) = oneshot::channel();
        let job = TransferJob {
            transfer_id: transfer_id.clone(),
            terminal_id: terminal_id.clone(),
            direction: direction.clone(),
            remote_path: remote_path.clone(),
            local_path: local_path.clone(),
            overwrite: request.overwrite,
            cancel: Some(cancel_receiver),
            source: transfer_source(&direction, &remote_path, &local_path),
            destination: transfer_destination(&direction, &remote_path, &local_path),
        };

        self.transfers
            .lock()
            .map_err(|_| SshManagerError::Closed)?
            .insert(
                transfer_id.clone(),
                TransferControl {
                    terminal_id: terminal_id.clone(),
                    cancel,
                },
            );

        self.emit_transfer(&app, job.event(0, None, TransferState::Queued, None));

        let command = SshCommand::StartTransfer { job };
        if sender.send(command).await.is_err() {
            self.finish_transfer(&transfer_id);
            return Err(SshManagerError::Closed);
        }

        Ok(SshTransferResponse { transfer_id })
    }

    pub fn cancel_transfer(&self, transfer_id: &str) -> Result<bool, SshManagerError> {
        let control = self
            .transfers
            .lock()
            .map_err(|_| SshManagerError::Closed)?
            .remove(transfer_id);
        if let Some(control) = control {
            let _ = control.cancel.send(());
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn sender(&self, terminal_id: &str) -> Result<mpsc::Sender<SshCommand>, SshManagerError> {
        self.sessions
            .lock()
            .map_err(|_| SshManagerError::Closed)?
            .get(terminal_id)
            .map(|state| state.sender.clone())
            .ok_or_else(|| SshManagerError::MissingSession(terminal_id.to_owned()))
    }

    fn emit_output(&self, app: &AppHandle, terminal_id: &str, bytes: &[u8]) {
        for chunk in bytes.chunks(OUTPUT_BUFFER_BYTES) {
            let data = String::from_utf8_lossy(chunk).into_owned();
            let should_emit = if let Ok(mut sessions) = self.sessions.lock() {
                if let Some(state) = sessions.get_mut(terminal_id) {
                    if state.attached {
                        true
                    } else {
                        if state.pending_output.len() == PENDING_OUTPUT_CHUNKS {
                            state.pending_output.remove(0);
                        }
                        state.pending_output.push(data.clone());
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };
            if should_emit {
                let _ = app.emit(
                    "ssh://output",
                    SshOutputEvent {
                        terminal_id: terminal_id.to_owned(),
                        data,
                    },
                );
            }
        }
    }

    fn remove(&self, terminal_id: &str) {
        self.cancel_for_terminal(terminal_id);
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.remove(terminal_id);
        }
    }

    fn cancel_for_terminal(&self, terminal_id: &str) {
        let transfers = if let Ok(mut transfers) = self.transfers.lock() {
            let ids = transfers
                .iter()
                .filter(|(_, control)| control.terminal_id == terminal_id)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| transfers.remove(&id))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        for control in transfers {
            let _ = control.cancel.send(());
        }
        let tunnels = if let Ok(mut tunnels) = self.tunnels.lock() {
            let ids = tunnels
                .iter()
                .filter(|(_, control)| control.terminal_id == terminal_id)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| tunnels.remove(&id))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        for control in tunnels {
            let _ = control.cancel.send(true);
        }
    }

    fn finish_transfer(&self, transfer_id: &str) {
        if let Ok(mut transfers) = self.transfers.lock() {
            transfers.remove(transfer_id);
        }
    }

    fn finish_tunnel(&self, tunnel_id: &str) {
        if let Ok(mut tunnels) = self.tunnels.lock() {
            tunnels.remove(tunnel_id);
        }
    }

    fn emit_transfer(&self, app: &AppHandle, event: SshTransferEvent) {
        let _ = app.emit("sftp://transfer", event);
    }

    fn emit_tunnel(&self, app: &AppHandle, event: SshTunnelEvent) {
        let _ = app.emit("ssh://tunnel", event);
    }

    fn emit_session_state(
        &self,
        app: &AppHandle,
        terminal_id: &str,
        state: SshSessionState,
        attempt: u8,
        error: Option<String>,
    ) {
        let _ = app.emit(
            "ssh://state",
            SshSessionEvent {
                terminal_id: terminal_id.to_owned(),
                state,
                attempt,
                error,
            },
        );
    }
}

async fn connect_transport(
    vault: &PlatformVault,
    request: &SshConnectRequest,
) -> Result<SshConnection, SshManagerError> {
    let credentials = credentials_from_request(vault, request)?;
    let host_key_policy = host_key_policy(request)?;
    let options = SshConnectOptions {
        host: request.host.clone(),
        port: request.port,
        host_key_policy,
        timeout: Duration::from_secs(12),
        credentials,
    };
    let mut jump_options = Vec::with_capacity(request.jump_hosts.len());
    for jump in &request.jump_hosts {
        let credentials = credentials_from_jump_request(vault, jump)?;
        let host_key_policy = host_key_policy_for(
            jump.known_hosts_path.clone(),
            jump.pinned_fingerprint.clone(),
        )?;
        jump_options.push(SshConnectOptions {
            host: jump.host.clone(),
            port: jump.port,
            host_key_policy,
            timeout: Duration::from_secs(12),
            credentials,
        });
    }
    if jump_options.is_empty() {
        Ok(SshConnection::connect(options).await?)
    } else {
        Ok(SshConnection::connect_with_jump_chain(options, jump_options).await?)
    }
}

fn credentials_from_request(
    vault: &PlatformVault,
    request: &SshConnectRequest,
) -> Result<SshCredentials, SshManagerError> {
    credentials_from_auth(vault, &request.username, &request.auth)
}

fn credentials_from_jump_request(
    vault: &PlatformVault,
    request: &SshJumpHostRequest,
) -> Result<SshCredentials, SshManagerError> {
    credentials_from_auth(vault, &request.username, &request.auth)
}

fn credentials_from_auth(
    vault: &PlatformVault,
    username: &str,
    auth: &SshAuthRequest,
) -> Result<SshCredentials, SshManagerError> {
    match auth {
        SshAuthRequest::Agent => Ok(SshCredentials::agent(username)),
        SshAuthRequest::Password { credential_id } => {
            let id = CredentialId::new(credential_id.clone())?;
            let secret = vault.get(&id)?;
            Ok(SshCredentials::password(username, secret.as_str()))
        }
        SshAuthRequest::PrivateKey {
            path,
            passphrase_credential_id,
        } => {
            let passphrase = passphrase_credential_id
                .as_ref()
                .map(|credential_id| {
                    CredentialId::new(credential_id.clone())
                        .and_then(|id| vault.get(&id))
                        .map(|secret| secret.as_str().to_owned())
                })
                .transpose()?;
            Ok(SshCredentials::private_key(
                username,
                expand_user_path(path),
                passphrase,
            ))
        }
    }
}

fn host_key_policy(request: &SshConnectRequest) -> Result<HostKeyPolicy, SshManagerError> {
    host_key_policy_for(
        request.known_hosts_path.clone(),
        request.pinned_fingerprint.clone(),
    )
}

fn host_key_policy_for(
    known_hosts_path: Option<String>,
    pinned_fingerprint: Option<String>,
) -> Result<HostKeyPolicy, SshManagerError> {
    match (known_hosts_path, pinned_fingerprint) {
        (Some(_), Some(_)) => Err(SshManagerError::InvalidRequest(
            "choose known_hosts or a pinned fingerprint, not both".into(),
        )),
        (Some(path), None) => Ok(HostKeyPolicy::KnownHosts(expand_user_path(&path))),
        (None, Some(fingerprint)) if fingerprint.trim().is_empty() => Err(
            SshManagerError::InvalidRequest("pinned fingerprint cannot be empty".into()),
        ),
        (None, Some(fingerprint)) => Ok(HostKeyPolicy::PinnedFingerprint(fingerprint.clone())),
        (None, None) => Ok(HostKeyPolicy::default()),
    }
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

async fn run_remote_session(
    context: RemoteSessionContext,
    mut connection: Arc<SshConnection>,
    mut reader: mobarust_ssh::SshShellReader,
    mut writer: mobarust_ssh::SshShellWriter,
    mut commands: mpsc::Receiver<SshCommand>,
) {
    let RemoteSessionContext {
        app,
        manager,
        terminal_id,
        request,
        vault,
    } = context;
    let mut should_report_error = None;
    let mut connection_is_live = true;

    'session: loop {
        match run_shell_once(
            &app,
            &manager,
            &terminal_id,
            &connection,
            &mut reader,
            &writer,
            &mut commands,
        )
        .await
        {
            ShellRunResult::Closed => break 'session,
            ShellRunResult::Lost(error) => {
                should_report_error = Some(error.clone());
                connection_is_live = false;
                let _ = connection.disconnect().await;
                let mut last_error = error;
                let mut reconnected = false;
                for attempt in 1..=3 {
                    manager.emit_session_state(
                        &app,
                        &terminal_id,
                        SshSessionState::Reconnecting,
                        attempt,
                        Some(last_error.clone()),
                    );
                    tokio::time::sleep(Duration::from_secs(1_u64 << (attempt - 1))).await;
                    match connect_transport(&vault, &request).await {
                        Ok(new_connection) => {
                            match new_connection.open_shell(request.cols, request.rows).await {
                                Ok(shell) => {
                                    let (new_reader, new_writer) = shell.split();
                                    connection = Arc::new(new_connection);
                                    reader = new_reader;
                                    writer = new_writer;
                                    connection_is_live = true;
                                    manager.emit_session_state(
                                        &app,
                                        &terminal_id,
                                        SshSessionState::Connected,
                                        attempt,
                                        None,
                                    );
                                    should_report_error = None;
                                    reconnected = true;
                                    break;
                                }
                                Err(error) => last_error = error.to_string(),
                            }
                        }
                        Err(error) => last_error = error.to_string(),
                    }
                }
                if !reconnected {
                    manager.emit_session_state(
                        &app,
                        &terminal_id,
                        SshSessionState::Failed,
                        3,
                        Some(last_error.clone()),
                    );
                    should_report_error =
                        Some(format!("connection lost; reconnect failed: {last_error}"));
                    break 'session;
                }
            }
        }
    }

    if connection_is_live {
        let _ = connection.disconnect().await;
    }
    manager.remove(&terminal_id);
    manager.emit_session_state(
        &app,
        &terminal_id,
        SshSessionState::Disconnected,
        0,
        should_report_error.clone(),
    );
    let _ = app.emit(
        "ssh://closed",
        SshClosedEvent {
            terminal_id,
            reason: should_report_error.unwrap_or_else(|| "closed".into()),
        },
    );
}

enum ShellRunResult {
    Closed,
    Lost(String),
}

async fn run_shell_once(
    app: &AppHandle,
    manager: &SshManager,
    terminal_id: &str,
    connection: &Arc<SshConnection>,
    reader: &mut mobarust_ssh::SshShellReader,
    writer: &mobarust_ssh::SshShellWriter,
    commands: &mut mpsc::Receiver<SshCommand>,
) -> ShellRunResult {
    loop {
        tokio::select! {
            output = reader.next_output() => {
                match output {
                    None => return ShellRunResult::Lost("SSH shell channel closed".into()),
                    Some(Ok(SshOutput::Stdout(bytes) | SshOutput::Stderr(bytes))) => {
                        manager.emit_output(app, terminal_id, &bytes);
                    }
                    Some(Ok(SshOutput::ExitStatus(_))) => return ShellRunResult::Closed,
                    Some(Ok(SshOutput::Control)) => {}
                    Some(Err(error)) => return ShellRunResult::Lost(error.to_string()),
                }
            }
            command = commands.recv() => {
                match command {
                    Some(SshCommand::Write(data)) => {
                        if let Err(error) = writer.write(&data).await {
                            return ShellRunResult::Lost(error.to_string());
                        }
                    }
                    Some(SshCommand::Resize { cols, rows }) => {
                        if let Err(error) = writer.resize(cols, rows).await {
                            return ShellRunResult::Lost(error.to_string());
                        }
                    }
                    Some(SshCommand::ListDirectory { path, reply }) => {
                        let operation_connection = Arc::clone(connection);
                        tauri::async_runtime::spawn(async move {
                            let result = list_remote_directory(&operation_connection, path).await;
                            let _ = reply.send(result);
                        });
                    }
                    Some(SshCommand::FileOperation { operation, reply }) => {
                        let operation_connection = Arc::clone(connection);
                        tauri::async_runtime::spawn(async move {
                            let result = run_file_operation(&operation_connection, operation).await;
                            let _ = reply.send(result);
                        });
                    }
                    Some(SshCommand::StartLocalForward { job }) => {
                        let tunnel_manager = manager.clone();
                        let tunnel_connection = Arc::clone(connection);
                        let tunnel_app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            run_local_forward(tunnel_app, tunnel_manager, tunnel_connection, job).await;
                        });
                    }
                    Some(SshCommand::StartTransfer { job }) => {
                        let transfer_manager = manager.clone();
                        let transfer_connection = Arc::clone(connection);
                        let transfer_app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            run_transfer(transfer_app, transfer_manager, transfer_connection, job).await;
                        });
                    }
                    Some(SshCommand::Close) | None => {
                        let _ = writer.close().await;
                        return ShellRunResult::Closed;
                    }
                }
            }
        }
    }
}

async fn list_remote_directory(
    connection: &SshConnection,
    path: String,
) -> Result<Vec<mobarust_ssh::RemoteEntry>, String> {
    let sftp = open_sftp_with_timeout(connection)
        .await
        .map_err(|error| error.to_string())?;
    let result = sftp.read_dir(path).await.map_err(|error| error.to_string());
    let _ = sftp.close().await;
    result
}

async fn run_file_operation(
    connection: &SshConnection,
    operation: SshFileOperation,
) -> Result<(), String> {
    let sftp = open_sftp_with_timeout(connection)
        .await
        .map_err(|error| error.to_string())?;
    let result = match operation {
        SshFileOperation::Rename { from, to } => sftp
            .rename(from, to)
            .await
            .map_err(|error| error.to_string()),
        SshFileOperation::CreateDirectory { path } => sftp
            .create_dir(path)
            .await
            .map_err(|error| error.to_string()),
        SshFileOperation::Delete { path } => {
            let (_, is_directory) = sftp
                .file_info(&path)
                .await
                .map_err(|error| error.to_string())?;
            if is_directory {
                sftp.remove_dir(path)
                    .await
                    .map_err(|error| error.to_string())
            } else {
                sftp.remove_file(path)
                    .await
                    .map_err(|error| error.to_string())
            }
        }
    };
    let close_result = sftp.close().await;
    result?;
    close_result.map_err(|error| error.to_string())
}

async fn run_local_forward(
    app: AppHandle,
    manager: SshManager,
    connection: Arc<SshConnection>,
    mut job: LocalForwardJob,
) {
    const MAX_CONNECTIONS: usize = 16;
    let mut connections = 0_usize;
    let mut bytes_forwarded = 0_u64;
    let mut failed = false;
    let mut workers = JoinSet::<Result<(u64, u64), String>>::new();

    manager.emit_tunnel(
        &app,
        job.event(TunnelState::Running, connections, bytes_forwarded, None),
    );

    loop {
        tokio::select! {
            changed = job.cancel.changed() => {
                if changed.is_err() || *job.cancel.borrow() {
                    manager.emit_tunnel(&app, job.event(TunnelState::Stopping, connections, bytes_forwarded, None));
                    break;
                }
            }
            worker = workers.join_next(), if !workers.is_empty() => {
                if let Some(result) = worker {
                    match result {
                        Ok(Ok((uploaded, downloaded))) => {
                            bytes_forwarded = bytes_forwarded.saturating_add(uploaded).saturating_add(downloaded);
                            manager.emit_tunnel(&app, job.event(TunnelState::Running, connections, bytes_forwarded, None));
                        }
                        Ok(Err(error)) => {
                            manager.emit_tunnel(&app, job.event(TunnelState::Running, connections, bytes_forwarded, Some(error)));
                        }
                        Err(error) => {
                            manager.emit_tunnel(&app, job.event(TunnelState::Running, connections, bytes_forwarded, Some(error.to_string())));
                        }
                    }
                }
            }
            accepted = job.listener.accept() => {
                match accepted {
                    Ok((mut local, _peer)) => {
                        if workers.len() >= MAX_CONNECTIONS {
                            drop(local);
                            manager.emit_tunnel(&app, job.event(TunnelState::Running, connections, bytes_forwarded, Some("tunnel connection limit reached".into())));
                            continue;
                        }
                        connections = connections.saturating_add(1);
                        let connection = Arc::clone(&connection);
                        let target_host = job.target_host.clone();
                        let target_port = job.target_port;
                        let mut cancel = job.cancel.clone();
                        workers.spawn(async move {
                            let mut remote = connection
                                .open_direct_tcpip(target_host, u32::from(target_port))
                                .await
                                .map_err(|error| error.to_string())?;
                            tokio::select! {
                                _ = cancel.changed() => Err("tunnel connection cancelled".into()),
                                copied = copy_bidirectional(&mut local, &mut remote) => copied.map_err(|error| error.to_string()),
                            }
                        });
                        manager.emit_tunnel(&app, job.event(TunnelState::Running, connections, bytes_forwarded, None));
                    }
                    Err(error) => {
                        failed = true;
                        manager.emit_tunnel(&app, job.event(TunnelState::Failed, connections, bytes_forwarded, Some(format!("local tunnel listener failed: {error}"))));
                        break;
                    }
                }
            }
        }
    }

    workers.shutdown().await;
    if !failed {
        manager.emit_tunnel(
            &app,
            job.event(TunnelState::Stopped, connections, bytes_forwarded, None),
        );
    }
    manager.finish_tunnel(&job.tunnel_id);
}

async fn run_transfer(
    app: AppHandle,
    manager: SshManager,
    connection: Arc<SshConnection>,
    mut job: TransferJob,
) {
    let mut lifecycle = TransferLifecycle::new();
    let mut transferred = 0_u64;
    let mut total_bytes = None;
    let mut cancel = job.cancel.take().expect("transfer cancellation receiver");

    let permit = tokio::select! {
        _ = &mut cancel => {
            let _ = lifecycle.apply(TransferEvent::CancelRequested);
            manager.emit_transfer(&app, job.event(0, None, lifecycle.state(), None));
            let _ = lifecycle.apply(TransferEvent::Cancelled);
            manager.emit_transfer(&app, job.event(0, None, lifecycle.state(), None));
            manager.finish_transfer(&job.transfer_id);
            return;
        }
        permit = manager.transfer_slots.clone().acquire_owned() => match permit {
            Ok(permit) => permit,
            Err(_) => {
                let _ = lifecycle.apply(TransferEvent::CancelRequested);
                let _ = lifecycle.apply(TransferEvent::Cancelled);
                manager.emit_transfer(&app, job.event(0, None, lifecycle.state(), Some("transfer scheduler is unavailable".into())));
                manager.finish_transfer(&job.transfer_id);
                return;
            }
        },
    };
    let _permit = permit;

    let _ = lifecycle.apply(TransferEvent::Prepare);
    manager.emit_transfer(&app, job.event(0, None, lifecycle.state(), None));
    let _ = lifecycle.apply(TransferEvent::Start);
    manager.emit_transfer(&app, job.event(0, None, lifecycle.state(), None));

    let result = match &job.direction {
        TransferDirection::Download => {
            run_download(
                &connection,
                &job.remote_path,
                &job.local_path,
                job.overwrite,
                &mut cancel,
                |bytes, total| {
                    transferred = bytes;
                    total_bytes = total;
                    manager
                        .emit_transfer(&app, job.event(bytes, total, TransferState::Running, None));
                },
            )
            .await
        }
        TransferDirection::Upload => {
            run_upload(
                &connection,
                &job.remote_path,
                &job.local_path,
                job.overwrite,
                &mut cancel,
                |bytes, total| {
                    transferred = bytes;
                    total_bytes = total;
                    manager
                        .emit_transfer(&app, job.event(bytes, total, TransferState::Running, None));
                },
            )
            .await
        }
    };

    match result {
        Ok(bytes) => {
            let _ = lifecycle.apply(TransferEvent::Complete);
            manager.emit_transfer(
                &app,
                job.event(bytes, Some(bytes).or(total_bytes), lifecycle.state(), None),
            );
        }
        Err(SshError::Cancelled) => {
            let _ = lifecycle.apply(TransferEvent::CancelRequested);
            manager.emit_transfer(
                &app,
                job.event(transferred, total_bytes, lifecycle.state(), None),
            );
            let _ = lifecycle.apply(TransferEvent::Cancelled);
            manager.emit_transfer(
                &app,
                job.event(transferred, total_bytes, lifecycle.state(), None),
            );
        }
        Err(error) => {
            let _ = lifecycle.apply(TransferEvent::Fail);
            manager.emit_transfer(
                &app,
                job.event(
                    transferred,
                    total_bytes,
                    lifecycle.state(),
                    Some(error.to_string()),
                ),
            );
        }
    }
    manager.finish_transfer(&job.transfer_id);
}

async fn run_download<F>(
    connection: &SshConnection,
    remote_path: &str,
    destination: &Path,
    overwrite: bool,
    cancel: &mut oneshot::Receiver<()>,
    mut on_progress: F,
) -> Result<u64, SshError>
where
    F: FnMut(u64, Option<u64>),
{
    let sftp = open_sftp_with_timeout(connection).await?;
    let (total, is_directory) = sftp.file_info(remote_path).await?;
    if is_directory {
        return Err(SshError::Sftp("download source is a directory".into()));
    }
    let destination_metadata = fs::metadata(destination).await;
    match destination_metadata {
        Ok(metadata) if metadata.is_dir() => {
            return Err(SshError::Sftp("download destination is a directory".into()));
        }
        Ok(_) if !overwrite => {
            return Err(SshError::Sftp(
                "download destination already exists; enable overwrite explicitly".into(),
            ));
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(SshError::LocalIo(error));
        }
        _ => {}
    }

    let temporary = local_part_path(destination)?;
    let mut file = match OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .await
    {
        Ok(file) => file,
        Err(error) => return Err(SshError::LocalIo(error)),
    };
    let copied = match sftp
        .download_to_with_cancel(remote_path, &mut file, cancel, |bytes| {
            on_progress(bytes, Some(total));
        })
        .await
    {
        Ok(copied) => copied,
        Err(error) => {
            let _ = fs::remove_file(&temporary).await;
            let _ = sftp.close().await;
            return Err(error);
        }
    };
    if let Err(error) = file.sync_all().await {
        let _ = fs::remove_file(&temporary).await;
        let _ = sftp.close().await;
        return Err(SshError::LocalIo(error));
    }
    drop(file);
    if !overwrite
        && fs::try_exists(destination)
            .await
            .map_err(SshError::LocalIo)?
    {
        let _ = fs::remove_file(&temporary).await;
        let _ = sftp.close().await;
        return Err(SshError::Sftp(
            "download destination appeared during transfer".into(),
        ));
    }
    if let Err(error) = commit_local_file(&temporary, destination, overwrite) {
        let _ = fs::remove_file(&temporary).await;
        let _ = sftp.close().await;
        return Err(error);
    }
    let _ = sftp.close().await;
    Ok(copied)
}

async fn run_upload<F>(
    connection: &SshConnection,
    remote_path: &str,
    source: &Path,
    overwrite: bool,
    cancel: &mut oneshot::Receiver<()>,
    mut on_progress: F,
) -> Result<u64, SshError>
where
    F: FnMut(u64, Option<u64>),
{
    let metadata = fs::metadata(source).await.map_err(SshError::LocalIo)?;
    if !metadata.is_file() {
        return Err(SshError::LocalIo(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "upload source is not a regular file",
        )));
    }
    let sftp = open_sftp_with_timeout(connection).await?;
    if sftp.try_exists(remote_path).await? {
        let (_, is_directory) = sftp.file_info(remote_path).await?;
        if is_directory {
            return Err(SshError::Sftp("upload destination is a directory".into()));
        }
        if !overwrite {
            return Err(SshError::Sftp(
                "upload destination already exists; enable overwrite explicitly".into(),
            ));
        }
    }

    let temporary = remote_part_path(remote_path, &Uuid::new_v4().to_string())?;
    let mut file = fs::File::open(source).await.map_err(SshError::LocalIo)?;
    let copied = match sftp
        .upload_from_with_cancel(&mut file, &temporary, cancel, |bytes| {
            on_progress(bytes, Some(metadata.len()));
        })
        .await
    {
        Ok(copied) => copied,
        Err(error) => {
            let _ = sftp.remove_file(&temporary).await;
            let _ = sftp.close().await;
            return Err(error);
        }
    };
    if !overwrite && sftp.try_exists(remote_path).await? {
        let _ = sftp.remove_file(&temporary).await;
        let _ = sftp.close().await;
        return Err(SshError::Sftp(
            "upload destination appeared during transfer".into(),
        ));
    }
    if let Err(error) = sftp.rename(&temporary, remote_path).await {
        let _ = sftp.remove_file(&temporary).await;
        let _ = sftp.close().await;
        return Err(error);
    }
    let _ = sftp.close().await;
    Ok(copied)
}

async fn open_sftp_with_timeout(
    connection: &SshConnection,
) -> Result<mobarust_ssh::SftpConnection, SshError> {
    tokio::time::timeout(Duration::from_secs(12), connection.open_sftp())
        .await
        .map_err(|_| SshError::Timeout)?
}

fn validate_remote_file_path(path: &str) -> Result<String, SshManagerError> {
    let path = path.trim();
    if path.is_empty() || path == "." || path == "/" || path.contains('\0') {
        return Err(SshManagerError::InvalidRequest(
            "remote file path must identify a non-root path".into(),
        ));
    }
    Ok(path.to_owned())
}

fn validate_remote_directory_path(path: &str) -> Result<String, SshManagerError> {
    let path = path.trim();
    if path.is_empty() || path.contains('\0') {
        return Err(SshManagerError::InvalidRequest(
            "remote directory path cannot be empty or contain NUL".into(),
        ));
    }
    Ok(path.to_owned())
}

fn validate_remote_mutation_path(path: &str) -> Result<String, SshManagerError> {
    let path = validate_remote_directory_path(path)?;
    if path == "." || path == "/" {
        return Err(SshManagerError::InvalidRequest(
            "remote root cannot be modified through this command".into(),
        ));
    }
    Ok(path)
}

fn validate_local_file_path(path: &str) -> Result<PathBuf, SshManagerError> {
    if path.trim().is_empty() || path.contains('\0') {
        return Err(SshManagerError::InvalidRequest(
            "local file path cannot be empty or contain NUL".into(),
        ));
    }
    Ok(PathBuf::from(path))
}

fn local_part_path(destination: &Path) -> Result<PathBuf, SshError> {
    let name = destination
        .file_name()
        .ok_or_else(|| SshError::Sftp("download destination must include a file name".into()))?
        .to_string_lossy();
    Ok(destination.with_file_name(format!(".{name}.mobarust.part")))
}

fn remote_part_path(remote_path: &str, transfer_id: &str) -> Result<String, SshError> {
    let trimmed = remote_path.trim_end_matches('/');
    let (parent, name) = trimmed.rsplit_once('/').unwrap_or((".", trimmed));
    if name.is_empty() {
        return Err(SshError::Sftp(
            "remote destination must include a file name".into(),
        ));
    }
    Ok(format!("{parent}/.{name}.mobarust-{transfer_id}.part"))
}

fn commit_local_file(
    temporary: &Path,
    destination: &Path,
    overwrite: bool,
) -> Result<(), SshError> {
    #[cfg(windows)]
    if overwrite && destination.exists() {
        std::fs::remove_file(destination).map_err(SshError::LocalIo)?;
    }
    if !overwrite && destination.exists() {
        return Err(SshError::Sftp("download destination already exists".into()));
    }
    std::fs::rename(temporary, destination).map_err(SshError::LocalIo)
}

fn transfer_source(direction: &TransferDirection, remote_path: &str, local_path: &Path) -> String {
    match direction {
        TransferDirection::Download => remote_path.to_owned(),
        TransferDirection::Upload => local_path.display().to_string(),
    }
}

fn transfer_destination(
    direction: &TransferDirection,
    remote_path: &str,
    local_path: &Path,
) -> String {
    match direction {
        TransferDirection::Download => local_path.display().to_string(),
        TransferDirection::Upload => remote_path.to_owned(),
    }
}

fn default_terminal_cols() -> u32 {
    120
}

fn default_bind_host() -> String {
    "127.0.0.1".to_owned()
}

fn default_terminal_rows() -> u32 {
    32
}
