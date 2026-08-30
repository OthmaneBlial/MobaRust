use mobarust_core::{TransferEvent, TransferLifecycle, TransferState};
use mobarust_ssh::{
    HostKeyPolicy, Socks5ReplyCode, SshConnectOptions, SshConnection, SshCredentials, SshError,
    SshOutput, X11ForwardingOptions, negotiate_socks5, send_socks5_reply,
};
use mobarust_vault::{CredentialId, CredentialLookup, VaultError};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use thiserror::Error;
use tokio::fs::{self, OpenOptions};
use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tokio::task::JoinSet;
use uuid::Uuid;

const COMMAND_CAPACITY: usize = 64;
const OUTPUT_BUFFER_BYTES: usize = 32 * 1024;
const PENDING_OUTPUT_CHUNKS: usize = 32;
const TRANSFER_PROGRESS_MIN_INTERVAL: Duration = Duration::from_millis(100);
const TRANSFER_PROGRESS_MIN_BYTES: u64 = 8 * 1024 * 1024;
const SSH_RECONNECT_ATTEMPTS: u8 = 3;
const X11_CHANNEL_LIMIT: usize = 8;

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
    #[serde(default)]
    pub x11: Option<SshX11Request>,
    #[serde(default = "default_terminal_cols")]
    pub cols: u32,
    #[serde(default = "default_terminal_rows")]
    pub rows: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshX11Request {
    /// Explicit local display target, for example tcp://127.0.0.1:6000 or
    /// unix:///tmp/.X11-unix/X0. It is never inferred from the environment.
    pub display: String,
    #[serde(default)]
    pub single_connection: bool,
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
    KeyboardInteractive {
        #[serde(rename = "credentialId", alias = "credential_id")]
        credential_id: String,
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
    pub protocol: TransferProtocol,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default)]
    pub recursive: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TransferProtocol {
    #[default]
    Sftp,
    Scp,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshDynamicForwardRequest {
    #[serde(default = "default_bind_host")]
    pub bind_host: String,
    pub bind_port: u16,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshRemoteForwardRequest {
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
enum TunnelKind {
    Local,
    Dynamic,
    Remote,
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
    kind: TunnelKind,
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
    protocol: TransferProtocol,
    source: String,
    destination: String,
    recursive: bool,
    bytes_transferred: u64,
    total_bytes: Option<u64>,
    bytes_per_second: Option<u64>,
    eta_seconds: Option<u64>,
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

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum SshX11State {
    Failed,
    Disconnected,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SshX11Event {
    terminal_id: String,
    state: SshX11State,
    error: Option<String>,
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
    OpenTextFile {
        path: String,
        reply: oneshot::Sender<Result<mobarust_ssh::RemoteTextDocument, String>>,
    },
    CollectMonitor {
        reply: oneshot::Sender<Result<mobarust_ssh::RemoteMonitorSnapshot, String>>,
    },
    SaveTextFile {
        path: String,
        expected_revision: String,
        content: String,
        encoding: mobarust_ssh::RemoteTextEncoding,
        reply: oneshot::Sender<Result<mobarust_ssh::RemoteTextDocument, String>>,
    },
    SaveTextFileAs {
        path: String,
        content: String,
        encoding: mobarust_ssh::RemoteTextEncoding,
        overwrite: bool,
        reply: oneshot::Sender<Result<mobarust_ssh::RemoteTextDocument, String>>,
    },
    FileOperation {
        operation: SshFileOperation,
        reply: oneshot::Sender<Result<(), String>>,
    },
    StartLocalForward {
        job: LocalForwardJob,
    },
    StartDynamicForward {
        job: DynamicForwardJob,
    },
    StartRemoteForward {
        job: RemoteForwardJob,
        reply: oneshot::Sender<Result<SshTunnelResponse, String>>,
    },
    StartTransfer {
        job: TransferJob,
    },
}

enum SshFileOperation {
    Rename { from: String, to: String },
    Delete { path: String },
    CreateDirectory { path: String },
    SetPermissions { path: String, permissions: u32 },
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
    remote_forwards: Arc<Mutex<HashMap<String, String>>>,
    transfer_slots: Arc<Semaphore>,
}

impl Default for SshManager {
    fn default() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            transfers: Arc::new(Mutex::new(HashMap::new())),
            tunnels: Arc::new(Mutex::new(HashMap::new())),
            remote_forwards: Arc::new(Mutex::new(HashMap::new())),
            transfer_slots: Arc::new(Semaphore::new(3)),
        }
    }
}

struct SessionState {
    sender: mpsc::Sender<SshCommand>,
    close: watch::Sender<bool>,
    attached: bool,
    pending_output: Vec<String>,
}

struct RemoteSessionContext {
    app: AppHandle,
    manager: SshManager,
    terminal_id: String,
    request: SshConnectRequest,
    vault: Arc<dyn CredentialLookup>,
    close: watch::Receiver<bool>,
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

struct DynamicForwardJob {
    tunnel_id: String,
    terminal_id: String,
    bind_host: String,
    bind_port: u16,
    listener: TcpListener,
    cancel: watch::Receiver<bool>,
}

struct RemoteForwardJob {
    tunnel_id: String,
    terminal_id: String,
    bind_host: String,
    bind_port: u16,
    target_host: String,
    target_port: u16,
    cancel: watch::Receiver<bool>,
}

impl DynamicForwardJob {
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
            target_host: "SOCKS5".into(),
            target_port: 0,
            kind: TunnelKind::Dynamic,
            state,
            connections,
            bytes_forwarded,
            error,
        }
    }
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
            kind: TunnelKind::Local,
            state,
            connections,
            bytes_forwarded,
            error,
        }
    }
}

impl RemoteForwardJob {
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
            kind: TunnelKind::Remote,
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
    protocol: TransferProtocol,
    remote_path: String,
    local_path: PathBuf,
    overwrite: bool,
    recursive: bool,
    cancel: Option<oneshot::Receiver<()>>,
    source: String,
    destination: String,
    created_at: Instant,
}

impl TransferJob {
    fn event(
        &self,
        bytes_transferred: u64,
        total_bytes: Option<u64>,
        state: TransferState,
        error: Option<String>,
    ) -> SshTransferEvent {
        let (bytes_per_second, eta_seconds) =
            transfer_metrics(bytes_transferred, total_bytes, self.created_at.elapsed());
        SshTransferEvent {
            transfer_id: self.transfer_id.clone(),
            terminal_id: self.terminal_id.clone(),
            direction: self.direction.clone(),
            protocol: self.protocol,
            source: self.source.clone(),
            destination: self.destination.clone(),
            recursive: self.recursive,
            bytes_transferred,
            total_bytes,
            bytes_per_second,
            eta_seconds,
            state,
            error,
        }
    }
}

impl SshManager {
    pub async fn connect(
        &self,
        app: AppHandle,
        vault: Arc<dyn CredentialLookup>,
        request: SshConnectRequest,
    ) -> Result<SshConnectResponse, SshManagerError> {
        let host = request.host.clone();
        let connection = connect_transport(vault.as_ref(), &request).await?;
        let connection = Arc::new(connection);
        let shell = connection.open_shell(request.cols, request.rows).await?;
        let (reader, writer) = shell.split();
        let terminal_id = Uuid::new_v4().to_string();
        let (sender, receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (close, close_receiver) = watch::channel(false);
        self.sessions
            .lock()
            .map_err(|_| SshManagerError::Closed)?
            .insert(
                terminal_id.clone(),
                SessionState {
                    sender,
                    close,
                    attached: false,
                    pending_output: Vec::new(),
                },
            );

        self.start_x11_bridge(
            Arc::clone(&connection),
            close_receiver.clone(),
            app.clone(),
            terminal_id.clone(),
        );

        let manager = self.clone();
        let id_for_task = terminal_id.clone();
        let reconnect_vault = Arc::clone(&vault);
        let context = RemoteSessionContext {
            app,
            manager,
            terminal_id: id_for_task,
            request,
            vault: reconnect_vault,
            close: close_receiver,
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
        let close = self
            .sessions
            .lock()
            .map_err(|_| SshManagerError::Closed)?
            .get(terminal_id)
            .map(|state| state.close.clone())
            .ok_or_else(|| SshManagerError::MissingSession(terminal_id.to_owned()))?;
        close.send(true).map_err(|_| SshManagerError::Closed)
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

    pub async fn open_remote_text_file(
        &self,
        terminal_id: &str,
        path: String,
    ) -> Result<mobarust_ssh::RemoteTextDocument, SshManagerError> {
        let path = validate_remote_file_path(&path)?;
        let (reply, response) = oneshot::channel();
        self.sender(terminal_id)?
            .send(SshCommand::OpenTextFile { path, reply })
            .await
            .map_err(|_| SshManagerError::Closed)?;
        response
            .await
            .map_err(|_| SshManagerError::Closed)?
            .map_err(SshManagerError::InvalidRequest)
    }

    pub async fn collect_remote_monitor(
        &self,
        terminal_id: &str,
    ) -> Result<mobarust_ssh::RemoteMonitorSnapshot, SshManagerError> {
        let (reply, response) = oneshot::channel();
        self.sender(terminal_id)?
            .send(SshCommand::CollectMonitor { reply })
            .await
            .map_err(|_| SshManagerError::Closed)?;
        response
            .await
            .map_err(|_| SshManagerError::Closed)?
            .map_err(SshManagerError::InvalidRequest)
    }

    pub async fn save_remote_text_file(
        &self,
        terminal_id: &str,
        path: String,
        expected_revision: String,
        content: String,
        encoding: mobarust_ssh::RemoteTextEncoding,
    ) -> Result<mobarust_ssh::RemoteTextDocument, SshManagerError> {
        let path = validate_remote_file_path(&path)?;
        if content.len() > mobarust_ssh::MAX_REMOTE_EDITOR_BYTES {
            return Err(SshManagerError::InvalidRequest(
                "remote editor content exceeds the 4 MiB limit".into(),
            ));
        }
        if expected_revision.trim().is_empty() {
            return Err(SshManagerError::InvalidRequest(
                "remote editor revision is required".into(),
            ));
        }
        let (reply, response) = oneshot::channel();
        self.sender(terminal_id)?
            .send(SshCommand::SaveTextFile {
                path,
                expected_revision,
                content,
                encoding,
                reply,
            })
            .await
            .map_err(|_| SshManagerError::Closed)?;
        response
            .await
            .map_err(|_| SshManagerError::Closed)?
            .map_err(SshManagerError::InvalidRequest)
    }

    pub async fn save_remote_text_file_as(
        &self,
        terminal_id: &str,
        path: String,
        content: String,
        encoding: mobarust_ssh::RemoteTextEncoding,
        overwrite: bool,
    ) -> Result<mobarust_ssh::RemoteTextDocument, SshManagerError> {
        let path = validate_remote_file_path(&path)?;
        if content.len() > mobarust_ssh::MAX_REMOTE_EDITOR_BYTES {
            return Err(SshManagerError::InvalidRequest(
                "remote editor content exceeds the 4 MiB limit".into(),
            ));
        }
        let (reply, response) = oneshot::channel();
        self.sender(terminal_id)?
            .send(SshCommand::SaveTextFileAs {
                path,
                content,
                encoding,
                overwrite,
                reply,
            })
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

    pub async fn set_remote_permissions(
        &self,
        terminal_id: &str,
        path: String,
        permissions: u32,
    ) -> Result<(), SshManagerError> {
        let path = validate_remote_mutation_path(&path)?;
        if permissions > 0o7777 {
            return Err(SshManagerError::InvalidRequest(
                "remote permissions must be an octal mode between 0000 and 7777".into(),
            ));
        }
        self.run_file_operation(
            terminal_id,
            SshFileOperation::SetPermissions { path, permissions },
        )
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
                kind: TunnelKind::Local,
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

    pub async fn start_dynamic_forward(
        &self,
        app: AppHandle,
        terminal_id: String,
        request: SshDynamicForwardRequest,
    ) -> Result<SshTunnelResponse, SshManagerError> {
        let sender = self.sender(&terminal_id)?;
        let bind_host = if request.bind_host.trim().is_empty() {
            default_bind_host().to_owned()
        } else {
            request.bind_host.trim().to_owned()
        };
        if bind_host.contains('\0') {
            return Err(SshManagerError::InvalidRequest(
                "SOCKS bind host cannot contain NUL".into(),
            ));
        }
        let listener = TcpListener::bind((bind_host.as_str(), request.bind_port))
            .await
            .map_err(|error| {
                SshManagerError::InvalidRequest(format!("could not bind SOCKS listener: {error}"))
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
        let job = DynamicForwardJob {
            tunnel_id: tunnel_id.clone(),
            terminal_id: terminal_id.clone(),
            bind_host: bind_host.clone(),
            bind_port: local_port,
            listener,
            cancel: cancel_receiver,
        };
        self.emit_tunnel(&app, job.event(TunnelState::Listening, 0, 0, None));
        if sender
            .send(SshCommand::StartDynamicForward { job })
            .await
            .is_err()
        {
            self.finish_tunnel(&tunnel_id);
            return Err(SshManagerError::Closed);
        }
        Ok(SshTunnelResponse {
            tunnel_id,
            bind_host,
            bind_port: local_port,
        })
    }

    pub async fn start_remote_forward(
        &self,
        terminal_id: String,
        request: SshRemoteForwardRequest,
    ) -> Result<SshTunnelResponse, SshManagerError> {
        let sender = self.sender(&terminal_id)?;
        let bind_host = if request.bind_host.trim().is_empty() {
            default_bind_host().to_owned()
        } else {
            request.bind_host.trim().to_owned()
        };
        let target_host = request.target_host.trim().to_owned();
        if bind_host.contains('\0') {
            return Err(SshManagerError::InvalidRequest(
                "remote bind host cannot contain NUL".into(),
            ));
        }
        if target_host.is_empty() || target_host.contains('\0') || request.target_port == 0 {
            return Err(SshManagerError::InvalidRequest(
                "remote forward target host and port are required".into(),
            ));
        }
        let tunnel_id = Uuid::new_v4().to_string();
        {
            let mut remote_forwards = self
                .remote_forwards
                .lock()
                .map_err(|_| SshManagerError::Closed)?;
            if remote_forwards.contains_key(&terminal_id) {
                return Err(SshManagerError::InvalidRequest(
                    "only one remote forward can be active per SSH session".into(),
                ));
            }
            remote_forwards.insert(terminal_id.clone(), tunnel_id.clone());
        }

        let (cancel, cancel_receiver) = watch::channel(false);
        {
            let mut tunnels = match self.tunnels.lock() {
                Ok(tunnels) => tunnels,
                Err(_) => {
                    if let Ok(mut remote_forwards) = self.remote_forwards.lock() {
                        remote_forwards.remove(&terminal_id);
                    }
                    return Err(SshManagerError::Closed);
                }
            };
            tunnels.insert(
                tunnel_id.clone(),
                TunnelControl {
                    terminal_id: terminal_id.clone(),
                    cancel,
                },
            );
        }
        let (reply, response) = oneshot::channel();
        let job = RemoteForwardJob {
            tunnel_id: tunnel_id.clone(),
            terminal_id: terminal_id.clone(),
            bind_host,
            bind_port: request.bind_port,
            target_host,
            target_port: request.target_port,
            cancel: cancel_receiver,
        };
        if sender
            .send(SshCommand::StartRemoteForward { job, reply })
            .await
            .is_err()
        {
            self.finish_tunnel(&tunnel_id);
            return Err(SshManagerError::Closed);
        }
        response
            .await
            .map_err(|_| SshManagerError::Closed)?
            .map_err(SshManagerError::InvalidRequest)
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
        if request.protocol == TransferProtocol::Scp && request.recursive {
            return Err(SshManagerError::InvalidRequest(
                "SCP transfer manager supports single files only; use SFTP for directories".into(),
            ));
        }
        let transfer_id = Uuid::new_v4().to_string();
        let (cancel, cancel_receiver) = oneshot::channel();
        let job = TransferJob {
            transfer_id: transfer_id.clone(),
            terminal_id: terminal_id.clone(),
            direction: direction.clone(),
            protocol: request.protocol,
            remote_path: remote_path.clone(),
            local_path: local_path.clone(),
            overwrite: request.overwrite,
            recursive: request.recursive,
            cancel: Some(cancel_receiver),
            source: transfer_source(&direction, &remote_path, &local_path),
            destination: transfer_destination(&direction, &remote_path, &local_path),
            created_at: Instant::now(),
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
        if let Ok(mut remote_forwards) = self.remote_forwards.lock() {
            remote_forwards.retain(|_, active_tunnel_id| active_tunnel_id != tunnel_id);
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
    vault: &dyn CredentialLookup,
    request: &SshConnectRequest,
) -> Result<SshConnection, SshManagerError> {
    let x11 = request
        .x11
        .as_ref()
        .map(|x11| X11ForwardingOptions::parse(&x11.display, x11.single_connection))
        .transpose()
        .map_err(|error| SshManagerError::InvalidRequest(format!("X11: {error}")))?;
    let credentials = credentials_from_request(vault, request)?;
    let host_key_policy = host_key_policy(request)?;
    let options = SshConnectOptions {
        host: request.host.clone(),
        port: request.port,
        host_key_policy,
        timeout: Duration::from_secs(12),
        credentials,
        x11,
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
            x11: None,
        });
    }
    if jump_options.is_empty() {
        Ok(SshConnection::connect(options).await?)
    } else {
        Ok(SshConnection::connect_with_jump_chain(options, jump_options).await?)
    }
}

fn credentials_from_request(
    vault: &dyn CredentialLookup,
    request: &SshConnectRequest,
) -> Result<SshCredentials, SshManagerError> {
    credentials_from_auth(vault, &request.username, &request.auth)
}

fn credentials_from_jump_request(
    vault: &dyn CredentialLookup,
    request: &SshJumpHostRequest,
) -> Result<SshCredentials, SshManagerError> {
    credentials_from_auth(vault, &request.username, &request.auth)
}

fn credentials_from_auth(
    vault: &dyn CredentialLookup,
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
        SshAuthRequest::KeyboardInteractive { credential_id } => {
            let id = CredentialId::new(credential_id.clone())?;
            let secret = vault.get(&id)?;
            Ok(SshCredentials::keyboard_interactive(
                username,
                secret.as_str(),
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

enum ReconnectOutcome<T> {
    Connected { value: T, attempt: u8 },
    Cancelled,
    Failed { attempts: u8, last_error: String },
}

async fn reconnect_with_backoff<T, Before, Attempt, AttemptFuture, Delay>(
    close: &mut watch::Receiver<bool>,
    initial_error: String,
    mut before_attempt: Before,
    mut attempt: Attempt,
    delay_for: Delay,
) -> ReconnectOutcome<T>
where
    Before: FnMut(u8, &str),
    Attempt: FnMut(u8) -> AttemptFuture,
    AttemptFuture: std::future::Future<Output = Result<T, String>>,
    Delay: Fn(u8) -> Duration,
{
    let mut last_error = initial_error;
    for attempt_number in 1..=SSH_RECONNECT_ATTEMPTS {
        if *close.borrow() {
            return ReconnectOutcome::Cancelled;
        }
        before_attempt(attempt_number, &last_error);
        tokio::select! {
            changed = close.changed() => {
                if changed.is_err() || *close.borrow() {
                    return ReconnectOutcome::Cancelled;
                }
                continue;
            }
            _ = tokio::time::sleep(delay_for(attempt_number)) => {}
        }

        let result = tokio::select! {
            changed = close.changed() => {
                if changed.is_err() || *close.borrow() {
                    return ReconnectOutcome::Cancelled;
                }
                continue;
            }
            result = attempt(attempt_number) => result,
        };
        match result {
            Ok(value) => {
                return ReconnectOutcome::Connected {
                    value,
                    attempt: attempt_number,
                };
            }
            Err(error) => last_error = error,
        }
    }
    ReconnectOutcome::Failed {
        attempts: SSH_RECONNECT_ATTEMPTS,
        last_error,
    }
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
        mut close,
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
            &mut close,
        )
        .await
        {
            ShellRunResult::Closed => break 'session,
            ShellRunResult::Lost(error) => {
                connection_is_live = false;
                let _ = connection.disconnect().await;
                let outcome = reconnect_with_backoff(
                    &mut close,
                    error,
                    |attempt, last_error| {
                        manager.emit_session_state(
                            &app,
                            &terminal_id,
                            SshSessionState::Reconnecting,
                            attempt,
                            Some(last_error.to_owned()),
                        );
                    },
                    |_attempt| async {
                        let new_connection = connect_transport(vault.as_ref(), &request)
                            .await
                            .map_err(|error| error.to_string())?;
                        let shell = new_connection
                            .open_shell(request.cols, request.rows)
                            .await
                            .map_err(|error| error.to_string())?;
                        Ok((new_connection, shell.split()))
                    },
                    |attempt| Duration::from_secs(1_u64 << (attempt - 1)),
                )
                .await;
                match outcome {
                    ReconnectOutcome::Connected {
                        value: (new_connection, (new_reader, new_writer)),
                        attempt,
                    } => {
                        connection = Arc::new(new_connection);
                        reader = new_reader;
                        writer = new_writer;
                        manager.start_x11_bridge(
                            Arc::clone(&connection),
                            close.clone(),
                            app.clone(),
                            terminal_id.clone(),
                        );
                        connection_is_live = true;
                        manager.emit_session_state(
                            &app,
                            &terminal_id,
                            SshSessionState::Connected,
                            attempt,
                            None,
                        );
                        should_report_error = None;
                    }
                    ReconnectOutcome::Cancelled => {
                        should_report_error = None;
                        break 'session;
                    }
                    ReconnectOutcome::Failed {
                        attempts,
                        last_error,
                    } => {
                        manager.emit_session_state(
                            &app,
                            &terminal_id,
                            SshSessionState::Failed,
                            attempts,
                            Some(last_error.clone()),
                        );
                        should_report_error =
                            Some(format!("connection lost; reconnect failed: {last_error}"));
                        break 'session;
                    }
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

impl SshManager {
    fn start_x11_bridge(
        &self,
        connection: Arc<SshConnection>,
        close: watch::Receiver<bool>,
        app: AppHandle,
        terminal_id: String,
    ) {
        if connection.x11_display().is_none() {
            return;
        }
        tauri::async_runtime::spawn(async move {
            run_x11_bridge(connection, close, app, terminal_id).await;
        });
    }
}

async fn run_x11_bridge(
    connection: Arc<SshConnection>,
    mut close: watch::Receiver<bool>,
    app: AppHandle,
    terminal_id: String,
) {
    let slots = Arc::new(Semaphore::new(X11_CHANNEL_LIMIT));
    let mut workers = JoinSet::new();

    loop {
        tokio::select! {
            changed = close.changed() => {
                if changed.is_err() || *close.borrow() {
                    break;
                }
            }
            channel = connection.next_x11_channel() => {
                let Some(channel) = channel else { break; };
                let Ok(slot) = Arc::clone(&slots).try_acquire_owned() else {
                    // Do not let a remote peer create unbounded local display
                    // connections. The accepted channel is dropped and the
                    // SSH transport closes it through russh's RAII wrapper.
                    continue;
                };
                let worker_connection = Arc::clone(&connection);
                let mut worker_close = close.clone();
                let worker_app = app.clone();
                let worker_terminal_id = terminal_id.clone();
                workers.spawn(async move {
                    let _slot = slot;
                    tokio::select! {
                        result = worker_connection.bridge_x11_channel(channel) => {
                            if let Err(error) = result {
                                let _ = worker_app.emit(
                                    "ssh://x11",
                                    SshX11Event {
                                        terminal_id: worker_terminal_id,
                                        state: SshX11State::Failed,
                                        error: Some(error.to_string()),
                                    },
                                );
                            }
                        }
                        changed = worker_close.changed() => {
                            if changed.is_err() || *worker_close.borrow() {
                                let _ = worker_app.emit(
                                    "ssh://x11",
                                    SshX11Event {
                                        terminal_id: worker_terminal_id,
                                        state: SshX11State::Disconnected,
                                        error: None,
                                    },
                                );
                            }
                        }
                    }
                });
            }
        }

        while workers.try_join_next().is_some() {}
    }

    workers.abort_all();
    while workers.join_next().await.is_some() {}
}

enum ShellRunResult {
    Closed,
    Lost(String),
}

#[allow(clippy::too_many_arguments)]
async fn run_shell_once(
    app: &AppHandle,
    manager: &SshManager,
    terminal_id: &str,
    connection: &Arc<SshConnection>,
    reader: &mut mobarust_ssh::SshShellReader,
    writer: &mobarust_ssh::SshShellWriter,
    commands: &mut mpsc::Receiver<SshCommand>,
    close: &mut watch::Receiver<bool>,
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
                    Some(SshCommand::OpenTextFile { path, reply }) => {
                        let operation_connection = Arc::clone(connection);
                        tauri::async_runtime::spawn(async move {
                            let result = read_remote_text_file(&operation_connection, path).await;
                            let _ = reply.send(result);
                        });
                    }
                    Some(SshCommand::CollectMonitor { reply }) => {
                        let operation_connection = Arc::clone(connection);
                        tauri::async_runtime::spawn(async move {
                            let result = operation_connection
                                .remote_monitor_snapshot()
                                .await
                                .map_err(|error| error.to_string());
                            let _ = reply.send(result);
                        });
                    }
                    Some(SshCommand::SaveTextFile { path, expected_revision, content, encoding, reply }) => {
                        let operation_connection = Arc::clone(connection);
                        tauri::async_runtime::spawn(async move {
                            let result = save_remote_text_file(
                                &operation_connection,
                                path,
                                expected_revision,
                                content,
                                encoding,
                            )
                            .await;
                            let _ = reply.send(result);
                        });
                    }
                    Some(SshCommand::SaveTextFileAs { path, content, encoding, overwrite, reply }) => {
                        let operation_connection = Arc::clone(connection);
                        tauri::async_runtime::spawn(async move {
                            let result = save_remote_text_file_as(
                                &operation_connection,
                                path,
                                content,
                                encoding,
                                overwrite,
                            )
                            .await;
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
                    Some(SshCommand::StartDynamicForward { job }) => {
                        let tunnel_manager = manager.clone();
                        let tunnel_connection = Arc::clone(connection);
                        let tunnel_app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            run_dynamic_forward(tunnel_app, tunnel_manager, tunnel_connection, job).await;
                        });
                    }
                    Some(SshCommand::StartRemoteForward { job, reply }) => {
                        let tunnel_manager = manager.clone();
                        let tunnel_connection = Arc::clone(connection);
                        let tunnel_app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            run_remote_forward(
                                tunnel_app,
                                tunnel_manager,
                                tunnel_connection,
                                job,
                                reply,
                            )
                            .await;
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
                    None => {
                        let _ = writer.close().await;
                        return ShellRunResult::Closed;
                    }
                }
            }
            changed = close.changed() => {
                if changed.is_err() || *close.borrow() {
                    let _ = writer.close().await;
                    return ShellRunResult::Closed;
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

async fn read_remote_text_file(
    connection: &SshConnection,
    path: String,
) -> Result<mobarust_ssh::RemoteTextDocument, String> {
    let sftp = open_sftp_with_timeout(connection)
        .await
        .map_err(|error| error.to_string())?;
    let result = sftp
        .read_text_document(path)
        .await
        .map_err(|error| error.to_string());
    let _ = sftp.close().await;
    result
}

async fn save_remote_text_file(
    connection: &SshConnection,
    path: String,
    expected_revision: String,
    content: String,
    encoding: mobarust_ssh::RemoteTextEncoding,
) -> Result<mobarust_ssh::RemoteTextDocument, String> {
    let sftp = open_sftp_with_timeout(connection)
        .await
        .map_err(|error| error.to_string())?;
    let result = sftp
        .save_text_document_with_encoding(path, &expected_revision, &content, encoding)
        .await
        .map_err(|error| error.to_string());
    let _ = sftp.close().await;
    result
}

async fn save_remote_text_file_as(
    connection: &SshConnection,
    path: String,
    content: String,
    encoding: mobarust_ssh::RemoteTextEncoding,
    overwrite: bool,
) -> Result<mobarust_ssh::RemoteTextDocument, String> {
    let sftp = open_sftp_with_timeout(connection)
        .await
        .map_err(|error| error.to_string())?;
    let result = sftp
        .save_text_document_as(path, &content, encoding, overwrite)
        .await
        .map_err(|error| error.to_string());
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
        SshFileOperation::SetPermissions { path, permissions } => sftp
            .set_permissions(path, permissions)
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

async fn run_dynamic_forward(
    app: AppHandle,
    manager: SshManager,
    connection: Arc<SshConnection>,
    mut job: DynamicForwardJob,
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
                            manager.emit_tunnel(&app, job.event(TunnelState::Running, connections, bytes_forwarded, Some("SOCKS connection limit reached".into())));
                            continue;
                        }
                        connections = connections.saturating_add(1);
                        let connection = Arc::clone(&connection);
                        let mut cancel = job.cancel.clone();
                        workers.spawn(async move {
                            let request = tokio::select! {
                                _ = cancel.changed() => {
                                    let _ = send_socks5_reply(&mut local, Socks5ReplyCode::GeneralFailure).await;
                                    return Err("SOCKS connection cancelled".into());
                                }
                                result = tokio::time::timeout(Duration::from_secs(12), negotiate_socks5(&mut local)) => {
                                    match result {
                                        Ok(Ok(request)) => request,
                                        Ok(Err(error)) => {
                                            let _ = send_socks5_reply(&mut local, Socks5ReplyCode::GeneralFailure).await;
                                            return Err(error.to_string());
                                        }
                                        Err(_) => {
                                            let _ = send_socks5_reply(&mut local, Socks5ReplyCode::TtlExpired).await;
                                            return Err("SOCKS handshake timed out".into());
                                        }
                                    }
                                }
                            };
                            let mut remote = tokio::select! {
                                _ = cancel.changed() => {
                                    let _ = send_socks5_reply(&mut local, Socks5ReplyCode::GeneralFailure).await;
                                    return Err("SOCKS connection cancelled".into());
                                }
                                result = tokio::time::timeout(
                                    Duration::from_secs(12),
                                    connection.open_direct_tcpip(request.target_host, u32::from(request.target_port)),
                                ) => {
                                    match result {
                                        Ok(Ok(remote)) => remote,
                                        Ok(Err(error)) => {
                                            let _ = send_socks5_reply(&mut local, Socks5ReplyCode::ConnectionRefused).await;
                                            return Err(error.to_string());
                                        }
                                        Err(_) => {
                                            let _ = send_socks5_reply(&mut local, Socks5ReplyCode::TtlExpired).await;
                                            return Err("SOCKS target connection timed out".into());
                                        }
                                    }
                                }
                            };
                            send_socks5_reply(&mut local, Socks5ReplyCode::Succeeded)
                                .await
                                .map_err(|error| error.to_string())?;
                            tokio::select! {
                                _ = cancel.changed() => Err("SOCKS connection cancelled".into()),
                                copied = copy_bidirectional(&mut local, &mut remote) => copied.map_err(|error| error.to_string()),
                            }
                        });
                        manager.emit_tunnel(&app, job.event(TunnelState::Running, connections, bytes_forwarded, None));
                    }
                    Err(error) => {
                        failed = true;
                        manager.emit_tunnel(&app, job.event(TunnelState::Failed, connections, bytes_forwarded, Some(format!("SOCKS listener failed: {error}"))));
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

async fn run_remote_forward(
    app: AppHandle,
    manager: SshManager,
    connection: Arc<SshConnection>,
    mut job: RemoteForwardJob,
    reply: oneshot::Sender<Result<SshTunnelResponse, String>>,
) {
    const MAX_CONNECTIONS: usize = 16;
    let mut connections = 0_usize;
    let mut bytes_forwarded = 0_u64;
    let mut failed = false;
    let mut workers = JoinSet::<Result<(u64, u64), String>>::new();
    let requested_port = job.bind_port;
    let mut request_cancel = job.cancel.clone();

    let remote_port = tokio::select! {
        changed = request_cancel.changed() => {
            let message = if changed.is_err() || *request_cancel.borrow() {
                "remote forward cancelled before the server listener was ready".to_owned()
            } else {
                "remote forward cancellation channel closed".to_owned()
            };
            let _ = reply.send(Err(message));
            manager.finish_tunnel(&job.tunnel_id);
            return;
        }
        result = connection.request_remote_forward(job.bind_host.clone(), u32::from(requested_port)) => {
            match result {
                Ok(port) => port,
                Err(error) => {
                    let message = error.to_string();
                    manager.emit_tunnel(&app, job.event(TunnelState::Failed, 0, 0, Some(message.clone())));
                    let _ = reply.send(Err(message));
                    manager.finish_tunnel(&job.tunnel_id);
                    return;
                }
            }
        }
    };
    job.bind_port = remote_port;

    manager.emit_tunnel(
        &app,
        job.event(TunnelState::Listening, connections, bytes_forwarded, None),
    );
    let response = SshTunnelResponse {
        tunnel_id: job.tunnel_id.clone(),
        bind_host: job.bind_host.clone(),
        bind_port: remote_port,
    };
    if reply.send(Ok(response)).is_err() {
        let _ = connection
            .cancel_remote_forward(job.bind_host.clone(), u32::from(remote_port))
            .await;
        manager.finish_tunnel(&job.tunnel_id);
        return;
    }
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
            forwarded = connection.next_forwarded_channel() => {
                match forwarded {
                    Some(channel) => {
                        if workers.len() >= MAX_CONNECTIONS {
                            drop(channel);
                            manager.emit_tunnel(&app, job.event(TunnelState::Running, connections, bytes_forwarded, Some("remote tunnel connection limit reached".into())));
                            continue;
                        }
                        connections = connections.saturating_add(1);
                        let target_host = job.target_host.clone();
                        let target_port = job.target_port;
                        let mut cancel = job.cancel.clone();
                        workers.spawn(async move {
                            let mut local = tokio::select! {
                                _ = cancel.changed() => return Err("remote tunnel connection cancelled".into()),
                                result = tokio::time::timeout(
                                    Duration::from_secs(12),
                                    TcpStream::connect((target_host.as_str(), target_port)),
                                ) => {
                                    match result {
                                        Ok(Ok(stream)) => stream,
                                        Ok(Err(error)) => return Err(format!("local remote-forward target connection failed: {error}")),
                                        Err(_) => return Err("local remote-forward target connection timed out".into()),
                                    }
                                }
                            };
                            let mut remote = channel.into_stream();
                            tokio::select! {
                                _ = cancel.changed() => Err("remote tunnel connection cancelled".into()),
                                copied = copy_bidirectional(&mut local, &mut remote) => copied.map_err(|error| error.to_string()),
                            }
                        });
                        manager.emit_tunnel(&app, job.event(TunnelState::Running, connections, bytes_forwarded, None));
                    }
                    None => {
                        failed = true;
                        manager.emit_tunnel(&app, job.event(TunnelState::Failed, connections, bytes_forwarded, Some("SSH connection closed while remote forwarding was active".into())));
                        break;
                    }
                }
            }
        }
    }

    workers.shutdown().await;
    if let Err(error) = connection
        .cancel_remote_forward(job.bind_host.clone(), u32::from(remote_port))
        .await
    {
        failed = true;
        manager.emit_tunnel(
            &app,
            job.event(
                TunnelState::Failed,
                connections,
                bytes_forwarded,
                Some(format!("could not cancel remote listener: {error}")),
            ),
        );
    }
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

    let mut last_progress_at = Instant::now();
    let mut last_progress_bytes = 0_u64;
    let mut emit_progress = |bytes, total| {
        transferred = bytes;
        total_bytes = total;
        if should_emit_transfer_progress(
            bytes,
            total,
            Instant::now(),
            &mut last_progress_at,
            &mut last_progress_bytes,
        ) {
            manager.emit_transfer(&app, job.event(bytes, total, TransferState::Running, None));
        }
    };

    let result = match (&job.protocol, &job.direction) {
        (TransferProtocol::Sftp, TransferDirection::Download) => {
            run_download(
                &connection,
                &job.remote_path,
                &job.local_path,
                job.overwrite,
                job.recursive,
                &mut cancel,
                &mut emit_progress,
            )
            .await
        }
        (TransferProtocol::Sftp, TransferDirection::Upload) => {
            run_upload(
                &connection,
                &job.remote_path,
                &job.local_path,
                job.overwrite,
                job.recursive,
                &mut cancel,
                &mut emit_progress,
            )
            .await
        }
        (TransferProtocol::Scp, TransferDirection::Download) => {
            run_scp_download(
                &connection,
                &job.remote_path,
                &job.local_path,
                job.overwrite,
                job.recursive,
                &mut cancel,
                &mut emit_progress,
            )
            .await
        }
        (TransferProtocol::Scp, TransferDirection::Upload) => {
            run_scp_upload(
                &connection,
                &job.remote_path,
                &job.local_path,
                job.overwrite,
                job.recursive,
                &mut cancel,
                &mut emit_progress,
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

async fn run_scp_download<F>(
    connection: &SshConnection,
    remote_path: &str,
    destination: &Path,
    overwrite: bool,
    recursive: bool,
    cancel: &mut oneshot::Receiver<()>,
    mut on_progress: F,
) -> Result<u64, SshError>
where
    F: FnMut(u64, Option<u64>),
{
    if recursive {
        return Err(SshError::Scp(
            "recursive SCP transfers are not supported; use SFTP".into(),
        ));
    }
    let destination_metadata = fs::metadata(destination).await;
    match destination_metadata {
        Ok(metadata) if metadata.is_dir() => {
            return Err(SshError::Scp("download destination is a directory".into()));
        }
        Ok(_) if !overwrite => {
            return Err(SshError::Scp(
                "download destination already exists; enable overwrite explicitly".into(),
            ));
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(SshError::LocalIo(error));
        }
        _ => {}
    }

    let temporary = local_part_path(destination)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .await
        .map_err(SshError::LocalIo)?;
    let copied = match connection
        .scp_download_with_cancel(remote_path, &mut file, cancel, |bytes, total| {
            on_progress(bytes, Some(total));
        })
        .await
    {
        Ok(copied) => copied,
        Err(error) => {
            let _ = fs::remove_file(&temporary).await;
            return Err(error);
        }
    };
    if let Err(error) = file.sync_all().await {
        let _ = fs::remove_file(&temporary).await;
        return Err(SshError::LocalIo(error));
    }
    drop(file);
    if !overwrite
        && fs::try_exists(destination)
            .await
            .map_err(SshError::LocalIo)?
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(SshError::Scp(
            "download destination appeared during transfer".into(),
        ));
    }
    if let Err(error) = commit_local_file(&temporary, destination, overwrite) {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    Ok(copied)
}

async fn run_scp_upload<F>(
    connection: &SshConnection,
    remote_path: &str,
    source: &Path,
    overwrite: bool,
    recursive: bool,
    cancel: &mut oneshot::Receiver<()>,
    mut on_progress: F,
) -> Result<u64, SshError>
where
    F: FnMut(u64, Option<u64>),
{
    if recursive {
        return Err(SshError::Scp(
            "recursive SCP transfers are not supported; use SFTP".into(),
        ));
    }
    let metadata = fs::metadata(source).await.map_err(SshError::LocalIo)?;
    if !metadata.is_file() {
        return Err(SshError::LocalIo(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "SCP upload source is not a regular file",
        )));
    }

    let sftp = open_sftp_with_timeout(connection).await?;
    if sftp.try_exists(remote_path).await? {
        let (_, is_directory) = sftp.file_info(remote_path).await?;
        if is_directory {
            let _ = sftp.close().await;
            return Err(SshError::Scp("upload destination is a directory".into()));
        }
        if !overwrite {
            let _ = sftp.close().await;
            return Err(SshError::Scp(
                "upload destination already exists; enable overwrite explicitly".into(),
            ));
        }
    }

    let temporary = remote_part_path(remote_path, &Uuid::new_v4().to_string())?;
    let mut file = fs::File::open(source).await.map_err(SshError::LocalIo)?;
    let copied = match connection
        .scp_upload_with_cancel(&temporary, metadata.len(), &mut file, cancel, |bytes| {
            on_progress(bytes, Some(metadata.len()))
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
        return Err(SshError::Scp(
            "upload destination appeared during transfer".into(),
        ));
    }
    if let Err(error) = sftp.rename(&temporary, remote_path).await {
        let _ = sftp.remove_file(&temporary).await;
        let _ = sftp.close().await;
        return Err(error);
    }
    sftp.close().await?;
    Ok(copied)
}

async fn run_download<F>(
    connection: &SshConnection,
    remote_path: &str,
    destination: &Path,
    overwrite: bool,
    recursive: bool,
    cancel: &mut oneshot::Receiver<()>,
    mut on_progress: F,
) -> Result<u64, SshError>
where
    F: FnMut(u64, Option<u64>),
{
    let sftp = open_sftp_with_timeout(connection).await?;
    let (total, is_directory) = sftp.file_info(remote_path).await?;
    if is_directory {
        if !recursive {
            return Err(SshError::Sftp(
                "download source is a directory; enable recursive transfer".into(),
            ));
        }
        let result = download_directory(
            &sftp,
            remote_path,
            destination,
            overwrite,
            cancel,
            &mut on_progress,
        )
        .await;
        let close_result = sftp.close().await;
        let copied = result?;
        close_result?;
        return Ok(copied);
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
    recursive: bool,
    cancel: &mut oneshot::Receiver<()>,
    mut on_progress: F,
) -> Result<u64, SshError>
where
    F: FnMut(u64, Option<u64>),
{
    let metadata = fs::metadata(source).await.map_err(SshError::LocalIo)?;
    if metadata.is_dir() {
        if !recursive {
            return Err(SshError::LocalIo(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "upload source is a directory; enable recursive transfer",
            )));
        }
        let sftp = open_sftp_with_timeout(connection).await?;
        let result = upload_directory(
            &sftp,
            source,
            remote_path,
            overwrite,
            cancel,
            &mut on_progress,
        )
        .await;
        let close_result = sftp.close().await;
        let copied = result?;
        close_result?;
        return Ok(copied);
    }
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

const MAX_RECURSIVE_ENTRIES: usize = 100_000;

type RemoteDownloadFile = (String, PathBuf, u64);
type LocalUploadFile = (PathBuf, String, u64);

struct FileTransferProgress<'a, F> {
    base: u64,
    total: u64,
    cancel: &'a mut oneshot::Receiver<()>,
    on_progress: &'a mut F,
}

async fn download_directory<F>(
    sftp: &mobarust_ssh::SftpConnection,
    remote_root: &str,
    local_root: &Path,
    overwrite: bool,
    cancel: &mut oneshot::Receiver<()>,
    on_progress: &mut F,
) -> Result<u64, SshError>
where
    F: FnMut(u64, Option<u64>),
{
    match fs::symlink_metadata(local_root).await {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(SshError::Sftp(
                "recursive download refuses a symlink destination".into(),
            ));
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(SshError::Sftp(
                "recursive download destination is not a directory".into(),
            ));
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(SshError::LocalIo(error));
        }
        _ => {}
    }
    fs::create_dir_all(local_root)
        .await
        .map_err(SshError::LocalIo)?;

    let (files, directories, total) =
        collect_remote_files(sftp, remote_root, local_root, cancel).await?;
    for directory in directories {
        fs::create_dir_all(directory)
            .await
            .map_err(SshError::LocalIo)?;
    }

    let mut transferred = 0_u64;
    on_progress(0, Some(total));
    for (remote_path, local_path, size) in files {
        if cancel.try_recv().is_ok() {
            return Err(SshError::Cancelled);
        }
        let mut progress = FileTransferProgress {
            base: transferred,
            total,
            cancel,
            on_progress,
        };
        let copied = download_file_atomically(
            sftp,
            &remote_path,
            &local_path,
            size,
            overwrite,
            &mut progress,
        )
        .await?;
        transferred = transferred.saturating_add(copied);
        on_progress(transferred, Some(total));
    }
    Ok(transferred)
}

async fn collect_remote_files(
    sftp: &mobarust_ssh::SftpConnection,
    remote_root: &str,
    local_root: &Path,
    cancel: &mut oneshot::Receiver<()>,
) -> Result<(Vec<RemoteDownloadFile>, Vec<PathBuf>, u64), SshError> {
    let mut pending = VecDeque::from([(remote_root.to_owned(), local_root.to_owned())]);
    let mut files = Vec::new();
    let mut directories = vec![local_root.to_owned()];
    let mut total = 0_u64;
    let mut seen = 0_usize;

    while let Some((remote_directory, local_directory)) = pending.pop_front() {
        if cancel.try_recv().is_ok() {
            return Err(SshError::Cancelled);
        }
        let entries = tokio::select! {
            _ = &mut *cancel => return Err(SshError::Cancelled),
            result = sftp.read_dir(remote_directory.clone()) => result?,
        };
        for entry in entries {
            seen = seen.saturating_add(1);
            if seen > MAX_RECURSIVE_ENTRIES {
                return Err(SshError::Sftp(format!(
                    "recursive transfer exceeds the {MAX_RECURSIVE_ENTRIES} entry limit"
                )));
            }
            validate_transfer_component(&entry.name)?;
            let remote_path = remote_child_path(&remote_directory, &entry.name);
            let local_path = local_directory.join(&entry.name);
            if entry.is_directory {
                directories.push(local_path.clone());
                pending.push_back((remote_path, local_path));
            } else {
                total = total.saturating_add(entry.size);
                files.push((remote_path, local_path, entry.size));
            }
        }
    }
    Ok((files, directories, total))
}

async fn download_file_atomically<F>(
    sftp: &mobarust_ssh::SftpConnection,
    remote_path: &str,
    destination: &Path,
    total_size: u64,
    overwrite: bool,
    progress: &mut FileTransferProgress<'_, F>,
) -> Result<u64, SshError>
where
    F: FnMut(u64, Option<u64>),
{
    match fs::symlink_metadata(destination).await {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(SshError::Sftp(
                "recursive download refuses a symlink destination".into(),
            ));
        }
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
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(SshError::LocalIo)?;
    }
    let temporary = local_part_path(destination)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .await
        .map_err(SshError::LocalIo)?;
    let copied = match sftp
        .download_to_with_cancel(remote_path, &mut file, progress.cancel, |bytes| {
            (progress.on_progress)(
                progress.base.saturating_add(bytes),
                Some(progress.total.max(total_size)),
            );
        })
        .await
    {
        Ok(copied) => copied,
        Err(error) => {
            let _ = fs::remove_file(&temporary).await;
            return Err(error);
        }
    };
    file.sync_all().await.map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        SshError::LocalIo(error)
    })?;
    drop(file);
    if !overwrite
        && fs::try_exists(destination)
            .await
            .map_err(SshError::LocalIo)?
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(SshError::Sftp(
            "download destination appeared during transfer".into(),
        ));
    }
    if let Err(error) = commit_local_file(&temporary, destination, overwrite) {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    Ok(copied)
}

async fn upload_directory<F>(
    sftp: &mobarust_ssh::SftpConnection,
    local_root: &Path,
    remote_root: &str,
    overwrite: bool,
    cancel: &mut oneshot::Receiver<()>,
    on_progress: &mut F,
) -> Result<u64, SshError>
where
    F: FnMut(u64, Option<u64>),
{
    let (files, directories, total) = collect_local_files(local_root, remote_root, cancel).await?;
    if cancel.try_recv().is_ok() {
        return Err(SshError::Cancelled);
    }
    ensure_remote_directory(sftp, remote_root).await?;
    for directory in directories {
        ensure_remote_directory(sftp, &directory).await?;
    }

    let mut transferred = 0_u64;
    on_progress(0, Some(total));
    for (local_path, remote_path, size) in files {
        if cancel.try_recv().is_ok() {
            return Err(SshError::Cancelled);
        }
        let mut progress = FileTransferProgress {
            base: transferred,
            total,
            cancel,
            on_progress,
        };
        let copied = upload_file_atomically(
            sftp,
            &local_path,
            &remote_path,
            size,
            overwrite,
            &mut progress,
        )
        .await?;
        transferred = transferred.saturating_add(copied);
        on_progress(transferred, Some(total));
    }
    Ok(transferred)
}

async fn collect_local_files(
    local_root: &Path,
    remote_root: &str,
    cancel: &mut oneshot::Receiver<()>,
) -> Result<(Vec<LocalUploadFile>, Vec<String>, u64), SshError> {
    let metadata = fs::symlink_metadata(local_root)
        .await
        .map_err(SshError::LocalIo)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(SshError::Sftp(
            "recursive upload source must be a real directory".into(),
        ));
    }
    let mut pending = VecDeque::from([(local_root.to_owned(), remote_root.to_owned())]);
    let mut files = Vec::new();
    let mut directories = Vec::new();
    let mut total = 0_u64;
    let mut seen = 0_usize;

    while let Some((local_directory, remote_directory)) = pending.pop_front() {
        if cancel.try_recv().is_ok() {
            return Err(SshError::Cancelled);
        }
        let mut entries = fs::read_dir(&local_directory)
            .await
            .map_err(SshError::LocalIo)?;
        loop {
            let entry = tokio::select! {
                _ = &mut *cancel => return Err(SshError::Cancelled),
                result = entries.next_entry() => result.map_err(SshError::LocalIo)?,
            };
            let Some(entry) = entry else { break };
            seen = seen.saturating_add(1);
            if seen > MAX_RECURSIVE_ENTRIES {
                return Err(SshError::Sftp(format!(
                    "recursive transfer exceeds the {MAX_RECURSIVE_ENTRIES} entry limit"
                )));
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            validate_transfer_component(&name)?;
            let file_type = entry.file_type().await.map_err(SshError::LocalIo)?;
            let local_path = entry.path();
            let remote_path = remote_child_path(&remote_directory, &name);
            if file_type.is_symlink() {
                return Err(SshError::Sftp(
                    "recursive upload refuses to follow symlinks".into(),
                ));
            }
            if file_type.is_dir() {
                directories.push(remote_path.clone());
                pending.push_back((local_path, remote_path));
            } else if file_type.is_file() {
                let size = entry.metadata().await.map_err(SshError::LocalIo)?.len();
                total = total.saturating_add(size);
                files.push((local_path, remote_path, size));
            } else {
                return Err(SshError::Sftp(
                    "recursive upload supports regular files and directories only".into(),
                ));
            }
        }
    }
    Ok((files, directories, total))
}

async fn upload_file_atomically<F>(
    sftp: &mobarust_ssh::SftpConnection,
    source: &Path,
    remote_path: &str,
    total_size: u64,
    overwrite: bool,
    progress: &mut FileTransferProgress<'_, F>,
) -> Result<u64, SshError>
where
    F: FnMut(u64, Option<u64>),
{
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
        .upload_from_with_cancel(&mut file, &temporary, progress.cancel, |bytes| {
            (progress.on_progress)(
                progress.base.saturating_add(bytes),
                Some(progress.total.max(total_size)),
            );
        })
        .await
    {
        Ok(copied) => copied,
        Err(error) => {
            let _ = sftp.remove_file(&temporary).await;
            return Err(error);
        }
    };
    if !overwrite && sftp.try_exists(remote_path).await? {
        let _ = sftp.remove_file(&temporary).await;
        return Err(SshError::Sftp(
            "upload destination appeared during transfer".into(),
        ));
    }
    if let Err(error) = sftp.rename(&temporary, remote_path).await {
        let _ = sftp.remove_file(&temporary).await;
        return Err(error);
    }
    Ok(copied)
}

async fn ensure_remote_directory(
    sftp: &mobarust_ssh::SftpConnection,
    path: &str,
) -> Result<(), SshError> {
    let path = path.trim();
    if path.is_empty() || path == "." || path == "/" {
        return Ok(());
    }
    let absolute = path.starts_with('/');
    let mut current = if absolute {
        "/".to_owned()
    } else {
        String::new()
    };
    for component in path
        .split('/')
        .filter(|component| !component.is_empty() && *component != ".")
    {
        if component == ".." {
            return Err(SshError::Sftp(
                "recursive upload refuses parent-directory traversal".into(),
            ));
        }
        validate_transfer_component(component)?;
        current = if current == "/" {
            format!("/{component}")
        } else if current.is_empty() {
            component.to_owned()
        } else {
            format!("{current}/{component}")
        };
        if !sftp.try_exists(current.clone()).await? {
            sftp.create_dir(current.clone()).await?;
        }
        let (_, is_directory) = sftp.file_info(current.clone()).await?;
        if !is_directory {
            return Err(SshError::Sftp(format!(
                "remote path component is not a directory: {current}"
            )));
        }
    }
    Ok(())
}

fn validate_transfer_component(component: &str) -> Result<(), SshError> {
    if component.is_empty()
        || component == "."
        || component == ".."
        || component.contains('/')
        || component.contains('\\')
        || component.contains('\0')
    {
        return Err(SshError::Sftp(
            "recursive transfer encountered an unsafe path component".into(),
        ));
    }
    Ok(())
}

fn remote_child_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else if parent.ends_with('/') {
        format!("{parent}{name}")
    } else {
        format!("{parent}/{name}")
    }
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

fn transfer_metrics(
    bytes_transferred: u64,
    total_bytes: Option<u64>,
    elapsed: Duration,
) -> (Option<u64>, Option<u64>) {
    if bytes_transferred == 0 || elapsed.is_zero() {
        return (None, None);
    }
    let bytes_per_second =
        ((bytes_transferred as f64 / elapsed.as_secs_f64()).round() as u64).max(1);
    let eta_seconds = total_bytes.map(|total| {
        let remaining = total.saturating_sub(bytes_transferred);
        (remaining as f64 / bytes_per_second as f64).ceil() as u64
    });
    (Some(bytes_per_second), eta_seconds)
}

fn should_emit_transfer_progress(
    bytes_transferred: u64,
    total_bytes: Option<u64>,
    now: Instant,
    last_emitted_at: &mut Instant,
    last_emitted_bytes: &mut u64,
) -> bool {
    let initial = bytes_transferred == 0 && *last_emitted_bytes == 0;
    let completed = total_bytes.is_some_and(|total| bytes_transferred >= total);
    let byte_threshold =
        bytes_transferred.saturating_sub(*last_emitted_bytes) >= TRANSFER_PROGRESS_MIN_BYTES;
    let time_threshold = now.duration_since(*last_emitted_at) >= TRANSFER_PROGRESS_MIN_INTERVAL;
    if initial || completed || byte_threshold || time_threshold {
        *last_emitted_at = now;
        *last_emitted_bytes = bytes_transferred;
        true
    } else {
        false
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

#[cfg(test)]
mod tests {
    use super::{
        ReconnectOutcome, SshTransferRequest, TRANSFER_PROGRESS_MIN_INTERVAL, TransferProtocol,
        reconnect_with_backoff, remote_child_path, should_emit_transfer_progress, transfer_metrics,
        validate_transfer_component,
    };
    use std::time::{Duration, Instant};
    use tokio::sync::{oneshot, watch};

    #[test]
    fn recursive_remote_paths_keep_root_boundaries() {
        assert_eq!(remote_child_path("/", "etc"), "/etc");
        assert_eq!(remote_child_path("/var/", "log"), "/var/log");
        assert_eq!(remote_child_path("./tree", "file.txt"), "./tree/file.txt");
    }

    #[test]
    fn recursive_transfer_rejects_path_escape_components() {
        for component in ["", ".", "..", "a/b", "a\\b", "a\0b"] {
            assert!(
                validate_transfer_component(component).is_err(),
                "{component:?}"
            );
        }
        assert!(validate_transfer_component("safe-name.txt").is_ok());
    }

    #[test]
    fn transfer_requests_default_to_sftp_and_accept_explicit_scp() {
        let default_request: SshTransferRequest = serde_json::from_value(serde_json::json!({
            "remotePath": "/tmp/file",
            "localPath": "/tmp/file"
        }))
        .expect("deserialize default transfer request");
        assert_eq!(default_request.protocol, TransferProtocol::Sftp);

        let scp_request: SshTransferRequest = serde_json::from_value(serde_json::json!({
            "remotePath": "/tmp/file",
            "localPath": "/tmp/file",
            "protocol": "scp"
        }))
        .expect("deserialize SCP transfer request");
        assert_eq!(scp_request.protocol, TransferProtocol::Scp);
    }

    #[test]
    fn transfer_metrics_are_bounded_and_deterministic() {
        assert_eq!(
            transfer_metrics(50, Some(100), Duration::from_secs(2)),
            (Some(25), Some(2))
        );
        assert_eq!(
            transfer_metrics(100, Some(100), Duration::from_secs(1)),
            (Some(100), Some(0))
        );
        assert_eq!(
            transfer_metrics(0, Some(100), Duration::from_secs(1)),
            (None, None)
        );
    }

    #[test]
    fn transfer_progress_is_throttled_but_emits_initial_and_completion_events() {
        let start = Instant::now();
        let mut last_at = start;
        let mut last_bytes = 0;
        assert!(should_emit_transfer_progress(
            0,
            Some(100),
            start,
            &mut last_at,
            &mut last_bytes,
        ));
        assert!(!should_emit_transfer_progress(
            1,
            Some(100),
            start + Duration::from_millis(10),
            &mut last_at,
            &mut last_bytes,
        ));
        assert!(should_emit_transfer_progress(
            1,
            Some(100),
            start + TRANSFER_PROGRESS_MIN_INTERVAL,
            &mut last_at,
            &mut last_bytes,
        ));
        assert!(should_emit_transfer_progress(
            100,
            Some(100),
            start + Duration::from_millis(110),
            &mut last_at,
            &mut last_bytes,
        ));
    }

    #[tokio::test]
    async fn reconnect_policy_reports_bounded_failure_and_last_error() {
        let (_close_sender, mut close) = watch::channel(false);
        let mut attempts = Vec::new();
        let result = reconnect_with_backoff(
            &mut close,
            "shell channel closed".to_owned(),
            |attempt, error| attempts.push((attempt, error.to_owned())),
            |attempt| async move { Err::<(), String>(format!("fixture failure {attempt}")) },
            |_| Duration::ZERO,
        )
        .await;

        assert_eq!(
            attempts,
            vec![
                (1, "shell channel closed".to_owned()),
                (2, "fixture failure 1".to_owned()),
                (3, "fixture failure 2".to_owned()),
            ]
        );
        assert!(matches!(
            result,
            ReconnectOutcome::Failed {
                attempts: 3,
                last_error
            } if last_error == "fixture failure 3"
        ));
    }

    #[tokio::test]
    async fn reconnect_policy_returns_on_first_success_after_a_failure() {
        let (_close_sender, mut close) = watch::channel(false);
        let result = reconnect_with_backoff(
            &mut close,
            "shell channel closed".to_owned(),
            |_attempt, _error| {},
            |attempt| async move {
                if attempt == 2 {
                    Ok::<_, String>("fixture reconnected")
                } else {
                    Err(format!("fixture failure {attempt}"))
                }
            },
            |_| Duration::ZERO,
        )
        .await;

        assert!(matches!(
            result,
            ReconnectOutcome::Connected {
                value: "fixture reconnected",
                attempt: 2
            }
        ));
    }

    #[tokio::test]
    async fn reconnect_policy_cancels_an_inflight_attempt() {
        let (close_sender, mut close) = watch::channel(false);
        let (started_sender, started_receiver) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut started_sender = Some(started_sender);
            reconnect_with_backoff(
                &mut close,
                "shell channel closed".to_owned(),
                move |_attempt, _error| {
                    if let Some(sender) = started_sender.take() {
                        let _ = sender.send(());
                    }
                },
                |_attempt| async {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    Ok::<_, String>(())
                },
                |_| Duration::ZERO,
            )
            .await
        });

        started_receiver
            .await
            .expect("reconnect attempt should start");
        close_sender
            .send(true)
            .expect("reconnect cancellation receiver should remain alive");
        let result = tokio::time::timeout(Duration::from_millis(500), task)
            .await
            .expect("reconnect cancellation should be prompt")
            .expect("reconnect task should not panic");
        assert!(matches!(result, ReconnectOutcome::Cancelled));
    }
}
