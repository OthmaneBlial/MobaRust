use mobarust_core::{
    MAX_SERVER_ALIVE_INTERVAL_SECONDS, MAX_SSH_JUMP_HOSTS, TerminalInputError, TransferEvent,
    TransferLifecycle, TransferState, Utf8OutputDecoder, validate_terminal_input,
};
use mobarust_ssh::{
    HostKeyPolicy, KeyboardInteractiveChallenge, MAX_KEYBOARD_INTERACTIVE_RESPONSE_BYTES,
    Secret as SshSecret, Socks5ReplyCode, SshConnectOptions, SshConnection, SshCredentials,
    SshError, SshOutput, X11ForwardingOptions, negotiate_socks5, send_socks5_reply,
    validate_forward_host,
};
use mobarust_vault::{CredentialId, CredentialLookup, VaultError};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::ipc::Channel;
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
const SESSION_OPERATION_LIMIT: usize = 32;
const COMMAND_QUEUE_BUSY: &str =
    "SSH command queue is full; wait for queued actions to finish, then retry explicitly";
const SESSION_OPERATION_BUSY: &str =
    "SSH session is busy; wait for an operation to finish, then retry explicitly";
const QUEUED_COMMAND_CANCELLED: &str =
    "SSH session changed or closed before this queued action ran; retry explicitly";
const OUTPUT_BUFFER_BYTES: usize = 32 * 1024;
const PENDING_OUTPUT_CHUNKS: usize = 32;
const TRANSFER_PROGRESS_MIN_INTERVAL: Duration = Duration::from_millis(100);
const TRANSFER_PROGRESS_MIN_BYTES: u64 = 8 * 1024 * 1024;
const DEFAULT_SSH_RECONNECT_ATTEMPTS: u8 = 3;
const DEFAULT_SSH_CONNECT_TIMEOUT_MS: u64 = 12_000;
const SSH_STABLE_SHELL_DURATION: Duration = Duration::from_secs(30);
const X11_CHANNEL_LIMIT: usize = 8;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
    /// OpenSSH `ServerAliveInterval` value in seconds. Zero disables it.
    #[serde(default)]
    pub server_alive_interval: Option<u64>,
    /// Explicit environment entries for the target shell. Values are
    /// validated and applied only inside the native SSH transport.
    #[serde(default)]
    pub environment: Vec<(String, String)>,
    #[serde(default)]
    pub startup_directory: Option<String>,
    #[serde(default)]
    pub startup_command: Option<String>,
    #[serde(default = "default_ssh_reconnect_attempts")]
    pub reconnect_attempts: u8,
    #[serde(default = "default_ssh_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    #[serde(default = "default_terminal_cols")]
    pub cols: u32,
    #[serde(default = "default_terminal_rows")]
    pub rows: u32,
}

fn default_ssh_reconnect_attempts() -> u8 {
    DEFAULT_SSH_RECONNECT_ATTEMPTS
}

fn default_ssh_connect_timeout_ms() -> u64 {
    DEFAULT_SSH_CONNECT_TIMEOUT_MS
}

pub(crate) fn validate_ssh_connection_policy(
    request: &SshConnectRequest,
) -> Result<(), SshManagerError> {
    if request.jump_hosts.len() > MAX_SSH_JUMP_HOSTS {
        return Err(SshManagerError::InvalidRequest(
            "SSH jump chain cannot exceed 8 hosts".into(),
        ));
    }
    if request.reconnect_attempts > 10 {
        return Err(SshManagerError::InvalidRequest(
            "reconnect attempts must be between 0 and 10".into(),
        ));
    }
    if !(100..=60_000).contains(&request.connect_timeout_ms) {
        return Err(SshManagerError::InvalidRequest(
            "connect timeout must be between 100 and 60000 ms".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SshJumpHostRequest {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: SshAuthRequest,
    #[serde(default)]
    pub known_hosts_path: Option<String>,
    #[serde(default)]
    pub pinned_fingerprint: Option<String>,
    /// OpenSSH `ServerAliveInterval` value in seconds for this hop. Zero
    /// disables it.
    #[serde(default)]
    pub server_alive_interval: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "method", rename_all = "camelCase", deny_unknown_fields)]
pub enum SshAuthRequest {
    Agent,
    KeyboardInteractivePrompt,
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SshLocalForwardRequest {
    #[serde(default = "default_bind_host")]
    pub bind_host: String,
    pub bind_port: u16,
    pub target_host: String,
    pub target_port: u16,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SshDynamicForwardRequest {
    #[serde(default = "default_bind_host")]
    pub bind_host: String,
    pub bind_port: u16,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
pub(crate) struct SshOutputEvent {
    pub(crate) terminal_id: String,
    pub(crate) data: String,
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
    ListDirectory {
        path: String,
        reply: oneshot::Sender<Result<Vec<mobarust_ssh::RemoteEntry>, String>>,
    },
    OpenTextFile {
        path: String,
        encoding: mobarust_ssh::RemoteTextEncoding,
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
        cancel: oneshot::Receiver<()>,
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
    #[error(transparent)]
    Input(#[from] TerminalInputError),
}

#[derive(Clone)]
pub struct SshManager {
    sessions: Arc<Mutex<HashMap<String, SessionState>>>,
    transfers: Arc<Mutex<HashMap<String, TransferControl>>>,
    tunnels: Arc<Mutex<HashMap<String, TunnelControl>>>,
    remote_forwards: Arc<Mutex<HashMap<String, String>>>,
    transfer_slots: Arc<Semaphore>,
    shutdown: watch::Sender<bool>,
    authentication: Arc<Mutex<HashMap<String, PendingAuthentication>>>,
}

impl Default for SshManager {
    fn default() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            transfers: Arc::new(Mutex::new(HashMap::new())),
            tunnels: Arc::new(Mutex::new(HashMap::new())),
            remote_forwards: Arc::new(Mutex::new(HashMap::new())),
            transfer_slots: Arc::new(Semaphore::new(3)),
            shutdown: watch::channel(false).0,
            authentication: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

struct SessionState {
    sender: mpsc::Sender<SshCommand>,
    size: watch::Sender<(u32, u32)>,
    close: watch::Sender<bool>,
    finished: watch::Receiver<bool>,
    attached: bool,
    pending_output: Vec<String>,
    output_decoder: Utf8OutputDecoder,
}

struct RemoteSessionContext {
    app: AppHandle,
    manager: SshManager,
    terminal_id: String,
    request: SshConnectRequest,
    vault: Arc<dyn CredentialLookup>,
    close: watch::Receiver<bool>,
    size: watch::Receiver<(u32, u32)>,
    auth_events: Channel<SshAuthEvent>,
}

#[derive(Serialize)]
#[serde(
    tag = "event",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SshAuthEvent {
    Challenge {
        request_id: String,
        host: String,
        port: u16,
        username: String,
        name: String,
        instructions: String,
        prompts: Vec<String>,
    },
    Closed {
        request_id: String,
    },
}

struct PendingAuthentication {
    reply: oneshot::Sender<Option<Vec<SshSecret>>>,
    count: usize,
    terminal_id: Option<String>,
}

#[derive(Clone)]
struct AuthPromptContext {
    manager: SshManager,
    events: Channel<SshAuthEvent>,
    host: String,
    port: u16,
    username: String,
    terminal_id: Option<String>,
}

struct AuthenticationGuard {
    manager: SshManager,
    events: Channel<SshAuthEvent>,
    id: String,
}

impl Drop for AuthenticationGuard {
    fn drop(&mut self) {
        self.manager
            .authentication
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.id);
        let _ = self.events.send(SshAuthEvent::Closed {
            request_id: self.id.clone(),
        });
    }
}

impl AuthPromptContext {
    async fn ask(
        &self,
        challenge: KeyboardInteractiveChallenge,
    ) -> Result<Vec<SshSecret>, SshError> {
        let mut shutdown = self.manager.shutdown.subscribe();
        let id = Uuid::new_v4().to_string();
        let (reply, response) = oneshot::channel();
        {
            let mut pending = self
                .manager
                .authentication
                .lock()
                .map_err(|_| SshError::AuthenticationCancelled)?;
            if *self.manager.shutdown.borrow() || pending.len() >= 32 {
                return Err(SshError::AuthenticationCancelled);
            }
            pending.insert(
                id.clone(),
                PendingAuthentication {
                    reply,
                    count: challenge.prompts.len(),
                    terminal_id: self.terminal_id.clone(),
                },
            );
        }
        let _guard = AuthenticationGuard {
            manager: self.manager.clone(),
            events: self.events.clone(),
            id: id.clone(),
        };
        self.events
            .send(SshAuthEvent::Challenge {
                request_id: id,
                host: self.host.clone(),
                port: self.port,
                username: self.username.clone(),
                name: challenge.name,
                instructions: challenge.instructions,
                prompts: challenge.prompts,
            })
            .map_err(|_| SshError::AuthenticationCancelled)?;
        tokio::select! {
            biased;
            _ = shutdown.changed() => Err(SshError::AuthenticationCancelled),
            result = response => result.ok().flatten().ok_or(SshError::AuthenticationCancelled),
        }
    }
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
        auth_events: Channel<SshAuthEvent>,
    ) -> Result<SshConnectResponse, SshManagerError> {
        let mut shutdown = self.shutdown.subscribe();
        if *shutdown.borrow() {
            return Err(SshManagerError::Closed);
        }
        validate_ssh_connection_policy(&request)?;
        let host = request.host.clone();
        let (size, size_receiver) = watch::channel((request.cols.max(1), request.rows.max(1)));
        let connection = tokio::select! {
            biased;
            _ = shutdown.changed() => return Err(SshManagerError::Closed),
            result = connect_transport(vault.as_ref(), &request, self, &auth_events, None) => result?,
        };
        let connection = Arc::new(connection);
        let shell = tokio::select! {
            biased;
            _ = shutdown.changed() => return Err(SshManagerError::Closed),
            result = open_shell_at_current_size(&connection, &size_receiver) => result?,
        };
        let (reader, writer) = shell.split();
        let terminal_id = Uuid::new_v4().to_string();
        let (sender, receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (close, close_receiver) = watch::channel(false);
        let (finished, finished_receiver) = watch::channel(false);
        {
            let mut sessions = self.sessions.lock().map_err(|_| SshManagerError::Closed)?;
            // Serialize registration with shutdown's snapshot: a connection
            // finishing setup after Quit must not start an untracked session.
            if *self.shutdown.borrow() {
                return Err(SshManagerError::Closed);
            }
            sessions.insert(
                terminal_id.clone(),
                SessionState {
                    sender,
                    size,
                    close,
                    finished: finished_receiver,
                    attached: false,
                    pending_output: Vec::new(),
                    output_decoder: Utf8OutputDecoder::default(),
                },
            );
        }

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
            size: size_receiver,
            auth_events,
        };
        tauri::async_runtime::spawn(async move {
            run_remote_session(context, connection, reader, writer, receiver).await;
            finished.send_replace(true);
        });

        Ok(SshConnectResponse { terminal_id, host })
    }

    pub fn answer_authentication(
        &self,
        request_id: &str,
        responses: Option<Vec<String>>,
    ) -> Result<(), SshManagerError> {
        let responses =
            responses.map(|values| values.into_iter().map(SshSecret::new).collect::<Vec<_>>());
        let entry = {
            let mut pending = self
                .authentication
                .lock()
                .map_err(|_| SshManagerError::Closed)?;
            let entry = pending.get(request_id).ok_or(SshManagerError::Closed)?;
            if entry.reply.is_closed() || *self.shutdown.borrow() {
                return Err(SshManagerError::Closed);
            }
            if let Some(id) = &entry.terminal_id {
                let sessions = self.sessions.lock().map_err(|_| SshManagerError::Closed)?;
                if sessions.get(id).is_none_or(|state| *state.close.borrow()) {
                    return Err(SshManagerError::Closed);
                }
            }
            // Secret contents are intentionally absent from errors and logs.
            if responses.as_ref().is_some_and(|values| {
                values.len() != entry.count
                    || values
                        .iter()
                        .any(|value| value.len() > MAX_KEYBOARD_INTERACTIVE_RESPONSE_BYTES)
            }) {
                return Err(SshManagerError::InvalidRequest(
                    "authentication response count or size is invalid".into(),
                ));
            }
            pending.remove(request_id).ok_or(SshManagerError::Closed)?
        };
        if responses.is_none()
            && let Some(id) = &entry.terminal_id
        {
            if let Some(state) = self
                .sessions
                .lock()
                .map_err(|_| SshManagerError::Closed)?
                .get(id)
            {
                let _ = state.close.send(true);
            }
            self.cancel_for_terminal(id);
        }
        entry
            .reply
            .send(responses)
            .map_err(|_| SshManagerError::Closed)
    }

    pub async fn write(&self, terminal_id: &str, data: String) -> Result<(), SshManagerError> {
        validate_terminal_input(data.as_bytes())?;
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
        let sessions = self.sessions.lock().map_err(|_| SshManagerError::Closed)?;
        let state = sessions
            .get(terminal_id)
            .ok_or_else(|| SshManagerError::MissingSession(terminal_id.to_owned()))?;
        if *self.shutdown.borrow() || *state.close.borrow() {
            return Err(SshManagerError::Closed);
        }
        // Geometry is current state, not an action to replay. Keep only its
        // latest value, including while the old command queue is retired.
        state
            .size
            .send((cols.max(1), rows.max(1)))
            .map_err(|_| SshManagerError::Closed)
    }

    /// Close every SSH session and wait for its operations and transport teardown.
    /// No timeout aborts a worker that may already be replacing a remote file.
    pub async fn shutdown(&self) {
        let sessions = {
            // Recover the still-owned controls even after an unrelated panic;
            // skipping shutdown because this registry is poisoned loses cleanup.
            let sessions = self
                .sessions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            self.shutdown.send_replace(true);
            sessions
                .iter()
                .map(|(id, state)| (id.clone(), state.close.clone(), state.finished.clone()))
                .collect::<Vec<_>>()
        };
        for (id, close, _) in &sessions {
            self.cancel_for_terminal(id);
            let _ = close.send(true);
        }
        for (_, _, mut finished) in sessions {
            while !*finished.borrow() {
                if finished.changed().await.is_err() {
                    // A terminated session task cannot acknowledge completion.
                    // It is gone; do not wait forever for a dropped sender.
                    tracing::warn!(event = "ssh_cleanup_acknowledgement_lost");
                    break;
                }
            }
        }
    }

    pub async fn close(&self, terminal_id: &str) -> Result<(), SshManagerError> {
        let close = self
            .sessions
            .lock()
            .map_err(|_| SshManagerError::Closed)?
            .get(terminal_id)
            .map(|state| state.close.clone())
            .ok_or_else(|| SshManagerError::MissingSession(terminal_id.to_owned()))?;
        let signalled = close.send(true);
        self.cancel_for_terminal(terminal_id);
        signalled.map_err(|_| SshManagerError::Closed)
    }

    pub fn attach(
        &self,
        terminal_id: &str,
        mut emit: impl FnMut(String),
    ) -> Result<(), SshManagerError> {
        let mut sessions = self.sessions.lock().map_err(|_| SshManagerError::Closed)?;
        let state = sessions
            .get_mut(terminal_id)
            .ok_or_else(|| SshManagerError::MissingSession(terminal_id.to_owned()))?;
        // Emit the backlog under the session lock before enabling live output.
        // Returning it separately races newer events against the IPC response.
        for data in std::mem::take(&mut state.pending_output) {
            emit(data);
        }
        state.attached = true;
        Ok(())
    }

    pub async fn list_directory(
        &self,
        terminal_id: &str,
        path: String,
    ) -> Result<Vec<mobarust_ssh::RemoteEntry>, SshManagerError> {
        let path = validate_remote_directory_path(&path)?;
        let (reply, response) = oneshot::channel();
        let sender = self.sender(terminal_id)?;
        Self::reserve_command(&sender)?.send(SshCommand::ListDirectory { path, reply });
        response
            .await
            .map_err(|_| SshManagerError::Closed)?
            .map_err(SshManagerError::InvalidRequest)
    }

    pub async fn open_remote_text_file(
        &self,
        terminal_id: &str,
        path: String,
        encoding: mobarust_ssh::RemoteTextEncoding,
    ) -> Result<mobarust_ssh::RemoteTextDocument, SshManagerError> {
        let path = validate_remote_file_path(&path)?;
        let (reply, response) = oneshot::channel();
        let sender = self.sender(terminal_id)?;
        Self::reserve_command(&sender)?.send(SshCommand::OpenTextFile {
            path,
            encoding,
            reply,
        });
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
        let sender = self.sender(terminal_id)?;
        Self::reserve_command(&sender)?.send(SshCommand::CollectMonitor { reply });
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
        let sender = self.sender(terminal_id)?;
        Self::reserve_command(&sender)?.send(SshCommand::SaveTextFile {
            path,
            expected_revision,
            content,
            encoding,
            reply,
        });
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
        let sender = self.sender(terminal_id)?;
        Self::reserve_command(&sender)?.send(SshCommand::SaveTextFileAs {
            path,
            content,
            encoding,
            overwrite,
            reply,
        });
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
        let sender = self.sender(terminal_id)?;
        Self::reserve_command(&sender)?.send(SshCommand::FileOperation { operation, reply });
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
        let bind_host = if request.bind_host.trim().is_empty() {
            default_bind_host().to_owned()
        } else {
            request.bind_host.trim().to_owned()
        };
        let target_host = request.target_host.trim().to_owned();
        if request.target_port == 0 {
            return Err(SshManagerError::InvalidRequest(
                "tunnel target host and port are required".into(),
            ));
        }
        validate_tunnel_host(&bind_host, "tunnel bind host")?;
        validate_tunnel_host(&target_host, "tunnel target host")?;
        let sender = self.sender(&terminal_id)?;
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
        let permit = Self::reserve_command(&sender)?;
        let tunnel_id = Uuid::new_v4().to_string();
        let (cancel, cancel_receiver) = watch::channel(false);
        {
            let mut tunnels = self.tunnels.lock().map_err(|_| SshManagerError::Closed)?;
            self.ensure_current_sender(&terminal_id, &sender)?;
            tunnels.insert(
                tunnel_id.clone(),
                TunnelControl {
                    terminal_id: terminal_id.clone(),
                    cancel,
                },
            );
        }
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
        permit.send(command);
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
        let bind_host = if request.bind_host.trim().is_empty() {
            default_bind_host().to_owned()
        } else {
            request.bind_host.trim().to_owned()
        };
        validate_tunnel_host(&bind_host, "SOCKS bind host")?;
        let sender = self.sender(&terminal_id)?;
        let listener = TcpListener::bind((bind_host.as_str(), request.bind_port))
            .await
            .map_err(|error| {
                SshManagerError::InvalidRequest(format!("could not bind SOCKS listener: {error}"))
            })?;
        let local_port = listener
            .local_addr()
            .map_err(|error| SshManagerError::InvalidRequest(error.to_string()))?
            .port();
        let permit = Self::reserve_command(&sender)?;
        let tunnel_id = Uuid::new_v4().to_string();
        let (cancel, cancel_receiver) = watch::channel(false);
        {
            let mut tunnels = self.tunnels.lock().map_err(|_| SshManagerError::Closed)?;
            self.ensure_current_sender(&terminal_id, &sender)?;
            tunnels.insert(
                tunnel_id.clone(),
                TunnelControl {
                    terminal_id: terminal_id.clone(),
                    cancel,
                },
            );
        }
        let job = DynamicForwardJob {
            tunnel_id: tunnel_id.clone(),
            terminal_id: terminal_id.clone(),
            bind_host: bind_host.clone(),
            bind_port: local_port,
            listener,
            cancel: cancel_receiver,
        };
        self.emit_tunnel(&app, job.event(TunnelState::Listening, 0, 0, None));
        permit.send(SshCommand::StartDynamicForward { job });
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
        let bind_host = if request.bind_host.trim().is_empty() {
            default_bind_host().to_owned()
        } else {
            request.bind_host.trim().to_owned()
        };
        let target_host = request.target_host.trim().to_owned();
        if request.target_port == 0 {
            return Err(SshManagerError::InvalidRequest(
                "remote forward target host and port are required".into(),
            ));
        }
        validate_tunnel_host(&bind_host, "remote bind host")?;
        validate_tunnel_host(&target_host, "remote forward target host")?;
        let sender = self.sender(&terminal_id)?;
        let permit = Self::reserve_command(&sender)?;
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
            if let Err(error) = self.ensure_current_sender(&terminal_id, &sender) {
                drop(tunnels);
                self.finish_tunnel(&tunnel_id);
                return Err(error);
            }
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
        permit.send(SshCommand::StartRemoteForward { job, reply });
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
        let permit = Self::reserve_command(&sender)?;
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
            source: transfer_source(&direction, &remote_path, &local_path),
            destination: transfer_destination(&direction, &remote_path, &local_path),
            created_at: Instant::now(),
        };

        {
            let mut controls = self.transfers.lock().map_err(|_| SshManagerError::Closed)?;
            // Serialize late registration with the cancellation sweep. A caller
            // holding an old sender must not add controls after Close or loss.
            self.ensure_current_sender(&terminal_id, &sender)?;
            controls.insert(
                transfer_id.clone(),
                TransferControl {
                    terminal_id: terminal_id.clone(),
                    cancel,
                },
            );
        }

        self.emit_transfer(&app, job.event(0, None, TransferState::Queued, None));

        let command = SshCommand::StartTransfer {
            job,
            cancel: cancel_receiver,
        };
        permit.send(command);

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
        if *self.shutdown.borrow() {
            return Err(SshManagerError::Closed);
        }
        let sessions = self.sessions.lock().map_err(|_| SshManagerError::Closed)?;
        let state = sessions
            .get(terminal_id)
            .ok_or_else(|| SshManagerError::MissingSession(terminal_id.to_owned()))?;
        if *state.close.borrow() || state.sender.is_closed() {
            return Err(SshManagerError::Closed);
        }
        Ok(state.sender.clone())
    }

    fn reserve_command(
        sender: &mpsc::Sender<SshCommand>,
    ) -> Result<mpsc::Permit<'_, SshCommand>, SshManagerError> {
        // No suspended queue producer retains a finite action outside the bound.
        // Reserve before registering controls, then send without another await.
        sender.try_reserve().map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => {
                SshManagerError::InvalidRequest(COMMAND_QUEUE_BUSY.into())
            }
            mpsc::error::TrySendError::Closed(_) => SshManagerError::Closed,
        })
    }

    fn ensure_current_sender(
        &self,
        terminal_id: &str,
        sender: &mpsc::Sender<SshCommand>,
    ) -> Result<(), SshManagerError> {
        if self.sender(terminal_id)?.same_channel(sender) {
            Ok(())
        } else {
            Err(SshManagerError::Closed)
        }
    }

    fn reopen_command_queue(
        &self,
        terminal_id: &str,
    ) -> Result<mpsc::Receiver<SshCommand>, SshManagerError> {
        let mut sessions = self.sessions.lock().map_err(|_| SshManagerError::Closed)?;
        let state = sessions
            .get_mut(terminal_id)
            .ok_or_else(|| SshManagerError::MissingSession(terminal_id.to_owned()))?;
        if *self.shutdown.borrow() || *state.close.borrow() || !state.sender.is_closed() {
            return Err(SshManagerError::Closed);
        }
        let (sender, commands) = mpsc::channel(COMMAND_CAPACITY);
        state.sender = sender;
        Ok(commands)
    }

    async fn retire_command_queue(
        &self,
        terminal_id: &str,
        commands: &mut mpsc::Receiver<SshCommand>,
        mut transfer_event: impl FnMut(SshTransferEvent),
        mut tunnel_event: impl FnMut(SshTunnelEvent),
    ) -> usize {
        commands.close();
        self.cancel_for_terminal(terminal_id);
        let mut discarded = 0;
        // recv, rather than try_recv, also settles permits acquired just before
        // close. Old senders can never feed the replacement connection's queue.
        while let Some(command) = commands.recv().await {
            self.reject_queued_command(command, &mut transfer_event, &mut tunnel_event);
            discarded += 1;
        }
        discarded
    }

    fn reject_queued_command(
        &self,
        command: SshCommand,
        transfer_event: impl FnOnce(SshTransferEvent),
        tunnel_event: impl FnOnce(SshTunnelEvent),
    ) {
        self.reject_command(
            command,
            QUEUED_COMMAND_CANCELLED,
            TransferState::Cancelled,
            TunnelState::Stopped,
            transfer_event,
            tunnel_event,
        );
    }

    fn admit_session_command(
        &self,
        command: SshCommand,
        workers: &mut JoinSet<()>,
        transfer_event: impl FnOnce(SshTransferEvent),
        tunnel_event: impl FnOnce(SshTunnelEvent),
    ) -> Option<SshCommand> {
        // A bounded command queue alone cannot bound spawned workers. Reap
        // completed entries before deciding whether another operation fits.
        while workers.try_join_next().is_some() {}
        if workers.len() >= SESSION_OPERATION_LIMIT
            && matches!(
                command,
                SshCommand::ListDirectory { .. }
                    | SshCommand::OpenTextFile { .. }
                    | SshCommand::CollectMonitor { .. }
                    | SshCommand::SaveTextFile { .. }
                    | SshCommand::SaveTextFileAs { .. }
                    | SshCommand::FileOperation { .. }
                    | SshCommand::StartTransfer { .. }
                    | SshCommand::StartLocalForward { .. }
                    | SshCommand::StartDynamicForward { .. }
                    | SshCommand::StartRemoteForward { .. }
            )
        {
            self.reject_command(
                command,
                SESSION_OPERATION_BUSY,
                TransferState::Failed,
                TunnelState::Failed,
                transfer_event,
                tunnel_event,
            );
            None
        } else {
            Some(command)
        }
    }

    fn reject_command(
        &self,
        command: SshCommand,
        reason: &str,
        transfer_state: TransferState,
        tunnel_state: TunnelState,
        transfer_event: impl FnOnce(SshTransferEvent),
        tunnel_event: impl FnOnce(SshTunnelEvent),
    ) {
        match command {
            SshCommand::Write(_) => {}
            SshCommand::ListDirectory { reply, .. } => {
                let _ = reply.send(Err(reason.into()));
            }
            SshCommand::OpenTextFile { reply, .. }
            | SshCommand::SaveTextFile { reply, .. }
            | SshCommand::SaveTextFileAs { reply, .. } => {
                let _ = reply.send(Err(reason.into()));
            }
            SshCommand::CollectMonitor { reply } => {
                let _ = reply.send(Err(reason.into()));
            }
            SshCommand::FileOperation { reply, .. } => {
                let _ = reply.send(Err(reason.into()));
            }
            SshCommand::StartTransfer { job, .. } => {
                self.finish_transfer(&job.transfer_id);
                transfer_event(job.event(0, None, transfer_state, Some(reason.into())));
            }
            SshCommand::StartLocalForward { job } => {
                self.finish_tunnel(&job.tunnel_id);
                tunnel_event(job.event(tunnel_state, 0, 0, Some(reason.into())));
            }
            SshCommand::StartDynamicForward { job } => {
                self.finish_tunnel(&job.tunnel_id);
                tunnel_event(job.event(tunnel_state, 0, 0, Some(reason.into())));
            }
            SshCommand::StartRemoteForward { job, reply } => {
                self.finish_tunnel(&job.tunnel_id);
                tunnel_event(job.event(tunnel_state, 0, 0, Some(reason.into())));
                let _ = reply.send(Err(reason.into()));
            }
        }
    }

    fn emit_output(&self, app: &AppHandle, terminal_id: &str, bytes: &[u8]) {
        for chunk in bytes.chunks(OUTPUT_BUFFER_BYTES) {
            self.emit_output_chunk(app, terminal_id, Some(chunk));
        }
    }

    fn finish_output(&self, app: &AppHandle, terminal_id: &str) {
        self.emit_output_chunk(app, terminal_id, None);
    }

    fn emit_output_chunk(&self, app: &AppHandle, terminal_id: &str, chunk: Option<&[u8]>) {
        let data = if let Ok(mut sessions) = self.sessions.lock() {
            let Some(state) = sessions.get_mut(terminal_id) else {
                return;
            };
            let data = match chunk {
                Some(bytes) => state.output_decoder.push(bytes),
                None => state.output_decoder.finish(),
            };
            if data.is_empty() {
                return;
            }
            if !state.attached {
                if state.pending_output.len() == PENDING_OUTPUT_CHUNKS {
                    state.pending_output.remove(0);
                }
                state.pending_output.push(data);
                return;
            }
            data
        } else {
            return;
        };
        let _ = app.emit(
            "ssh://output",
            SshOutputEvent {
                terminal_id: terminal_id.to_owned(),
                data,
            },
        );
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

    async fn finish_session_operations(&self, terminal_id: &str, workers: &mut JoinSet<()>) {
        self.cancel_for_terminal(terminal_id);
        // Join tunnels after signalling cancellation; their listeners and connection
        // workers must stop before session completion. Let accepted file/editor
        // operations settle, including part cleanup and promotion/rollback.
        // Aborting those operations before disconnect can strand originals.
        while let Some(result) = workers.join_next().await {
            if result.is_err() {
                tracing::warn!(event = "session_operation_failed_during_close");
            }
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
    manager: &SshManager,
    auth_events: &Channel<SshAuthEvent>,
    terminal_id: Option<&str>,
) -> Result<SshConnection, SshManagerError> {
    mobarust_core::validate_session_environment(&request.environment)
        .map_err(|error| SshManagerError::InvalidRequest(error.to_string()))?;
    mobarust_core::validate_session_startup(
        request.startup_directory.as_deref(),
        request.startup_command.as_deref(),
    )
    .map_err(|error| SshManagerError::InvalidRequest(error.to_string()))?;
    let keepalive_interval = server_alive_interval_duration(request.server_alive_interval)?;
    let x11 = request
        .x11
        .as_ref()
        .map(|x11| X11ForwardingOptions::parse(&x11.display, x11.single_connection))
        .transpose()
        .map_err(|error| SshManagerError::InvalidRequest(format!("X11: {error}")))?;
    let prompt_context = |host: &str, port, username: &str| AuthPromptContext {
        manager: manager.clone(),
        events: auth_events.clone(),
        host: host.to_owned(),
        port,
        username: username.to_owned(),
        terminal_id: terminal_id.map(str::to_owned),
    };
    let credentials = credentials_from_auth(
        vault,
        &request.auth,
        prompt_context(&request.host, request.port, &request.username),
    )?;
    let host_key_policy = host_key_policy(request)?;
    let options = SshConnectOptions {
        host: request.host.clone(),
        port: request.port,
        host_key_policy,
        timeout: Duration::from_millis(request.connect_timeout_ms),
        keepalive_interval,
        credentials,
        x11,
        environment: request.environment.clone(),
        startup_directory: request.startup_directory.clone(),
        startup_command: request.startup_command.clone(),
    };
    let mut jump_options = Vec::with_capacity(request.jump_hosts.len());
    for jump in &request.jump_hosts {
        let credentials = credentials_from_auth(
            vault,
            &jump.auth,
            prompt_context(&jump.host, jump.port, &jump.username),
        )?;
        let host_key_policy = host_key_policy_for(
            jump.known_hosts_path.clone(),
            jump.pinned_fingerprint.clone(),
        )?;
        jump_options.push(SshConnectOptions {
            host: jump.host.clone(),
            port: jump.port,
            host_key_policy,
            timeout: Duration::from_millis(request.connect_timeout_ms),
            keepalive_interval: server_alive_interval_duration(jump.server_alive_interval)?,
            credentials,
            x11: None,
            environment: Vec::new(),
            startup_directory: None,
            startup_command: None,
        });
    }
    if jump_options.is_empty() {
        Ok(SshConnection::connect(options).await?)
    } else {
        Ok(SshConnection::connect_with_jump_chain(options, jump_options).await?)
    }
}

fn server_alive_interval_duration(
    seconds: Option<u64>,
) -> Result<Option<Duration>, SshManagerError> {
    match seconds {
        None | Some(0) => Ok(None),
        Some(seconds) if seconds <= MAX_SERVER_ALIVE_INTERVAL_SECONDS => {
            Ok(Some(Duration::from_secs(seconds)))
        }
        Some(_) => Err(SshManagerError::InvalidRequest(format!(
            "ServerAliveInterval must be between 1 and {MAX_SERVER_ALIVE_INTERVAL_SECONDS} seconds"
        ))),
    }
}

fn credentials_from_auth(
    vault: &dyn CredentialLookup,
    auth: &SshAuthRequest,
    prompt: AuthPromptContext,
) -> Result<SshCredentials, SshManagerError> {
    let username = prompt.username.as_str();
    match auth {
        SshAuthRequest::KeyboardInteractivePrompt => {
            Ok(SshCredentials::keyboard_interactive_prompt(
                prompt.username.clone(),
                move |challenge| {
                    let prompt = prompt.clone();
                    async move { prompt.ask(challenge).await }
                },
            ))
        }
        SshAuthRequest::Agent => Ok(SshCredentials::agent(username)),
        SshAuthRequest::Password { credential_id } => {
            let id = CredentialId::new(credential_id.clone())?;
            let secret = vault.get(&id)?;
            Ok(SshCredentials::password_secret(
                username,
                SshSecret::from_zeroizing(secret.into_zeroizing()),
            ))
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
                        .map(|secret| SshSecret::from_zeroizing(secret.into_zeroizing()))
                })
                .transpose()?;
            Ok(SshCredentials::private_key_secret(
                username,
                expand_user_path(path),
                passphrase,
            ))
        }
        SshAuthRequest::KeyboardInteractive { credential_id } => {
            let id = CredentialId::new(credential_id.clone())?;
            let secret = vault.get(&id)?;
            Ok(SshCredentials::keyboard_interactive_secret(
                username,
                SshSecret::from_zeroizing(secret.into_zeroizing()),
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
        // Do not infer ~/.ssh/known_hosts. A user may opt into an explicit
        // path above, or deliberately pin the observed fingerprint.
        (None, None) => Ok(HostKeyPolicy::RejectUnknown),
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

fn next_shell_reconnect_count(previous: u8, shell_lifetime: Duration, limit: u8) -> Option<u8> {
    let previous = if shell_lifetime >= SSH_STABLE_SHELL_DURATION {
        0
    } else {
        previous
    };
    (previous < limit).then(|| previous + 1)
}

async fn reconnect_with_backoff<T, Before, Attempt, AttemptFuture, Delay>(
    close: &mut watch::Receiver<bool>,
    initial_error: String,
    attempts: u8,
    mut before_attempt: Before,
    mut attempt: Attempt,
    delay_for: Delay,
) -> ReconnectOutcome<T>
where
    Before: FnMut(u8, &str),
    Attempt: FnMut(u8) -> AttemptFuture,
    AttemptFuture: std::future::Future<Output = Result<T, SshManagerError>>,
    Delay: Fn(u8) -> Duration,
{
    let mut last_error = initial_error;
    for attempt_number in 1..=attempts {
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
            Err(error) => {
                // Startup input can be partially delivered. Preserve its type
                // until here so another attempt cannot silently replay it.
                if matches!(
                    error,
                    SshManagerError::Transport(
                        SshError::StartupInputTimeout | SshError::StartupInputFailed(_)
                    )
                ) {
                    return ReconnectOutcome::Failed {
                        attempts: attempt_number,
                        last_error: error.to_string(),
                    };
                }
                last_error = error.to_string();
            }
        }
    }
    ReconnectOutcome::Failed {
        attempts,
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
        mut size,
        auth_events,
    } = context;
    let mut should_report_error = None;
    let mut connection_is_live = true;
    let mut shell_started_at = Instant::now();
    let mut reconnects_since_stable_shell = 0;
    let mut workers = JoinSet::new();

    'session: loop {
        let shell_result = run_shell_once(
            &app,
            &manager,
            &terminal_id,
            &connection,
            &mut reader,
            &writer,
            &mut commands,
            &mut close,
            &mut size,
            &mut workers,
        )
        .await;
        let discarded = manager
            .retire_command_queue(
                &terminal_id,
                &mut commands,
                |event| manager.emit_transfer(&app, event),
                |event| manager.emit_tunnel(&app, event),
            )
            .await;
        retire_shell_output(reader, &writer).await;
        manager
            .finish_session_operations(&terminal_id, &mut workers)
            .await;
        manager.finish_output(&app, &terminal_id);
        if discarded > 0 {
            manager.emit_output(&app, &terminal_id, b"\r\nMobaRust: queued SSH actions were cancelled because the session changed or closed. Retry explicitly.\r\n");
        }
        match shell_result {
            ShellRunResult::Closed => break 'session,
            ShellRunResult::Lost(error) => {
                if *close.borrow() {
                    break 'session;
                }
                let Some(next_count) = next_shell_reconnect_count(
                    reconnects_since_stable_shell,
                    shell_started_at.elapsed(),
                    request.reconnect_attempts,
                ) else {
                    let reason = if request.reconnect_attempts == 0 {
                        "SSH connection lost; reconnect is disabled"
                    } else {
                        "SSH shell closed repeatedly shortly after reconnecting"
                    };
                    manager.emit_session_state(
                        &app,
                        &terminal_id,
                        SshSessionState::Failed,
                        request.reconnect_attempts,
                        Some(reason.into()),
                    );
                    should_report_error = Some(reason.into());
                    break 'session;
                };
                reconnects_since_stable_shell = next_count;
                connection_is_live = false;
                let _ = connection.disconnect().await;
                let outcome = reconnect_with_backoff(
                    &mut close,
                    error,
                    request.reconnect_attempts,
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
                        let new_connection = connect_transport(
                            vault.as_ref(),
                            &request,
                            &manager,
                            &auth_events,
                            Some(&terminal_id),
                        )
                        .await?;
                        let shell = open_shell_at_current_size(&new_connection, &size).await?;
                        Ok((new_connection, shell.split()))
                    },
                    |attempt| Duration::from_secs(1_u64 << (attempt - 1).min(5)),
                )
                .await;
                match outcome {
                    ReconnectOutcome::Connected {
                        value: (new_connection, (new_reader, new_writer)),
                        attempt,
                    } => {
                        commands = match manager.reopen_command_queue(&terminal_id) {
                            Ok(commands) => commands,
                            Err(_) => {
                                let _ = new_connection.disconnect().await;
                                break 'session;
                            }
                        };
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
                        shell_started_at = Instant::now();
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

async fn open_shell_at_current_size(
    connection: &SshConnection,
    size: &watch::Receiver<(u32, u32)>,
) -> Result<mobarust_ssh::SshShell, SshError> {
    let (cols, rows) = *size.borrow();
    connection.open_shell(cols, rows).await
}

async fn retire_shell_output(
    reader: mobarust_ssh::SshShellReader,
    writer: &mobarust_ssh::SshShellWriter,
) {
    // An unread bounded output queue can block the actor that must enqueue EOF
    // and service transfer cleanup on other channels of this transport.
    drop(reader);
    let _ = writer.close().await;
}

async fn run_shell_operation(
    operation: impl std::future::Future<Output = Result<(), SshError>>,
    reader: &mut mobarust_ssh::SshShellReader,
    close: &mut watch::Receiver<bool>,
    mut emit_output: impl FnMut(&[u8]),
) -> Result<(), ShellRunResult> {
    // Keep the same write future across output events: recreating it could replay input.
    tokio::pin!(operation);
    loop {
        if *close.borrow() {
            return Err(ShellRunResult::Closed);
        }
        tokio::select! {
            biased;
            changed = close.changed() => {
                if changed.is_err() || *close.borrow() {
                    return Err(ShellRunResult::Closed);
                }
            }
            result = &mut operation => {
                return result.map_err(|error| ShellRunResult::Lost(error.to_string()));
            }
            output = reader.next_output() => {
                match output {
                    Some(Ok(SshOutput::Stdout(bytes) | SshOutput::Stderr(bytes))) => emit_output(&bytes),
                    Some(Ok(SshOutput::Control)) => {},
                    Some(Ok(SshOutput::ExitStatus(_))) => return Err(ShellRunResult::Closed),
                    Some(Err(error)) => return Err(ShellRunResult::Lost(error.to_string())),
                    None => return Err(ShellRunResult::Lost("SSH shell channel closed".into())),
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "ssh_backpressure_test.rs"]
mod backpressure_tests;

fn spawn_session_operation(
    command: SshCommand,
    connection: Arc<SshConnection>,
    workers: &mut JoinSet<()>,
) -> Option<SshCommand> {
    match command {
        SshCommand::ListDirectory { path, reply } => {
            workers.spawn(async move {
                let result = list_remote_directory(&connection, path).await;
                let _ = reply.send(result);
            });
        }
        SshCommand::OpenTextFile {
            path,
            encoding,
            reply,
        } => {
            workers.spawn(async move {
                let result = read_remote_text_file(&connection, path, encoding).await;
                let _ = reply.send(result);
            });
        }
        SshCommand::CollectMonitor { reply } => {
            workers.spawn(async move {
                let result = connection
                    .remote_monitor_snapshot()
                    .await
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
            });
        }
        SshCommand::SaveTextFile {
            path,
            expected_revision,
            content,
            encoding,
            reply,
        } => {
            workers.spawn(async move {
                let result =
                    save_remote_text_file(&connection, path, expected_revision, content, encoding)
                        .await;
                let _ = reply.send(result);
            });
        }
        SshCommand::SaveTextFileAs {
            path,
            content,
            encoding,
            overwrite,
            reply,
        } => {
            workers.spawn(async move {
                let result =
                    save_remote_text_file_as(&connection, path, content, encoding, overwrite).await;
                let _ = reply.send(result);
            });
        }
        SshCommand::FileOperation { operation, reply } => {
            workers.spawn(async move {
                let result = run_file_operation(&connection, operation).await;
                let _ = reply.send(result);
            });
        }
        command => return Some(command),
    }
    None
}

fn spawn_session_tunnel(
    command: SshCommand,
    manager: SshManager,
    connection: Arc<SshConnection>,
    workers: &mut JoinSet<()>,
    emit_tunnel: impl FnMut(SshTunnelEvent) + Send + 'static,
) -> Option<SshCommand> {
    match command {
        SshCommand::StartLocalForward { job } => {
            workers.spawn(run_local_forward(emit_tunnel, manager, connection, job));
        }
        SshCommand::StartDynamicForward { job } => {
            workers.spawn(run_dynamic_forward(emit_tunnel, manager, connection, job));
        }
        SshCommand::StartRemoteForward { job, reply } => {
            workers.spawn(run_remote_forward(
                emit_tunnel,
                manager,
                connection,
                job,
                reply,
            ));
        }
        command => return Some(command),
    }
    None
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
    size: &mut watch::Receiver<(u32, u32)>,
    workers: &mut JoinSet<()>,
) -> ShellRunResult {
    loop {
        if *close.borrow() || *manager.shutdown.borrow() {
            return ShellRunResult::Closed;
        }
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
                if *close.borrow() || *manager.shutdown.borrow() {
                    if let Some(command) = command {
                        manager.reject_queued_command(command,
                            |event| manager.emit_transfer(app, event),
                            |event| manager.emit_tunnel(app, event));
                    }
                    return ShellRunResult::Closed;
                }
                let command = match command {
                    Some(command) => {
                        let Some(command) = manager.admit_session_command(command, workers,
                            |event| manager.emit_transfer(app, event),
                            |event| manager.emit_tunnel(app, event)) else {
                            continue;
                        };
                        let Some(command) = spawn_session_operation(command, Arc::clone(connection), workers) else {
                            continue;
                        };
                        let event_manager = manager.clone();
                        let event_app = app.clone();
                        let Some(command) = spawn_session_tunnel(command, manager.clone(), Arc::clone(connection), workers,
                            move |event| event_manager.emit_tunnel(&event_app, event)) else {
                            continue;
                        };
                        Some(command)
                    },
                    None => None,
                };
                match command {
                    Some(SshCommand::Write(data)) => {
                        if let Err(result) = run_shell_operation(writer.write(&data), reader, close,
                            |bytes| manager.emit_output(app, terminal_id, bytes)).await {
                            return result;
                        }
                    }
                    Some(SshCommand::StartTransfer { job, cancel }) => {
                        let transfer_manager = manager.clone();
                        let transfer_connection = Arc::clone(connection);
                        let transfer_app = app.clone();
                        workers.spawn(async move {
                            run_transfer(transfer_app, transfer_manager, transfer_connection, job, cancel).await;
                        });
                    }
                    Some(_) => unreachable!("session operations are dispatched above"),
                    None => {
                        return ShellRunResult::Closed;
                    }
                }
            }
            _ = workers.join_next(), if !workers.is_empty() => {}
            changed = size.changed() => {
                if changed.is_err() {
                    return ShellRunResult::Closed;
                }
                let (cols, rows) = *size.borrow_and_update();
                if let Err(result) = run_shell_operation(writer.resize(cols, rows), reader, close,
                    |bytes| manager.emit_output(app, terminal_id, bytes)).await {
                    return result;
                }
            }
            changed = close.changed() => {
                if changed.is_err() || *close.borrow() {
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
    let sftp = connection
        .open_sftp()
        .await
        .map_err(|error| error.to_string())?;
    let result = sftp.read_dir(path).await.map_err(|error| error.to_string());
    let _ = sftp.close().await;
    result
}

async fn read_remote_text_file(
    connection: &SshConnection,
    path: String,
    encoding: mobarust_ssh::RemoteTextEncoding,
) -> Result<mobarust_ssh::RemoteTextDocument, String> {
    let sftp = connection
        .open_sftp()
        .await
        .map_err(|error| error.to_string())?;
    let result = sftp
        .read_text_document_with_encoding(path, encoding)
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
    let sftp = connection
        .open_sftp()
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
    let sftp = connection
        .open_sftp()
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
    let sftp = connection
        .open_sftp()
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
        SshFileOperation::Delete { path } => sftp
            .remove_path(path)
            .await
            .map_err(|error| error.to_string()),
    };
    let close_result = sftp.close().await;
    result?;
    close_result.map_err(|error| error.to_string())
}

async fn run_local_forward(
    mut emit_tunnel: impl FnMut(SshTunnelEvent),
    manager: SshManager,
    connection: Arc<SshConnection>,
    mut job: LocalForwardJob,
) {
    const MAX_CONNECTIONS: usize = 16;
    let mut connections = 0_usize;
    let mut bytes_forwarded = 0_u64;
    let mut failed = false;
    let mut workers = JoinSet::<Result<(u64, u64), String>>::new();

    emit_tunnel(job.event(TunnelState::Running, connections, bytes_forwarded, None));

    loop {
        tokio::select! {
            changed = job.cancel.changed() => {
                if changed.is_err() || *job.cancel.borrow() {
                    emit_tunnel(job.event(TunnelState::Stopping, connections, bytes_forwarded, None));
                    break;
                }
            }
            worker = workers.join_next(), if !workers.is_empty() => {
                if let Some(result) = worker {
                    match result {
                        Ok(Ok((uploaded, downloaded))) => {
                            bytes_forwarded = bytes_forwarded.saturating_add(uploaded).saturating_add(downloaded);
                            emit_tunnel(job.event(TunnelState::Running, connections, bytes_forwarded, None));
                        }
                        Ok(Err(error)) => {
                            emit_tunnel(job.event(TunnelState::Running, connections, bytes_forwarded, Some(error)));
                        }
                        Err(error) => {
                            emit_tunnel(job.event(TunnelState::Running, connections, bytes_forwarded, Some(error.to_string())));
                        }
                    }
                }
            }
            accepted = job.listener.accept() => {
                match accepted {
                    Ok((mut local, _peer)) => {
                        if workers.len() >= MAX_CONNECTIONS {
                            drop(local);
                            emit_tunnel(job.event(TunnelState::Running, connections, bytes_forwarded, Some("tunnel connection limit reached".into())));
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
                        emit_tunnel(job.event(TunnelState::Running, connections, bytes_forwarded, None));
                    }
                    Err(error) => {
                        failed = true;
                        emit_tunnel(job.event(TunnelState::Failed, connections, bytes_forwarded, Some(format!("local tunnel listener failed: {error}"))));
                        break;
                    }
                }
            }
        }
    }

    workers.shutdown().await;
    if !failed {
        emit_tunnel(job.event(TunnelState::Stopped, connections, bytes_forwarded, None));
    }
    manager.finish_tunnel(&job.tunnel_id);
}

async fn run_dynamic_forward(
    mut emit_tunnel: impl FnMut(SshTunnelEvent),
    manager: SshManager,
    connection: Arc<SshConnection>,
    mut job: DynamicForwardJob,
) {
    const MAX_CONNECTIONS: usize = 16;
    let mut connections = 0_usize;
    let mut bytes_forwarded = 0_u64;
    let mut failed = false;
    let mut workers = JoinSet::<Result<(u64, u64), String>>::new();

    emit_tunnel(job.event(TunnelState::Running, connections, bytes_forwarded, None));

    loop {
        tokio::select! {
            changed = job.cancel.changed() => {
                if changed.is_err() || *job.cancel.borrow() {
                    emit_tunnel(job.event(TunnelState::Stopping, connections, bytes_forwarded, None));
                    break;
                }
            }
            worker = workers.join_next(), if !workers.is_empty() => {
                if let Some(result) = worker {
                    match result {
                        Ok(Ok((uploaded, downloaded))) => {
                            bytes_forwarded = bytes_forwarded.saturating_add(uploaded).saturating_add(downloaded);
                            emit_tunnel(job.event(TunnelState::Running, connections, bytes_forwarded, None));
                        }
                        Ok(Err(error)) => {
                            emit_tunnel(job.event(TunnelState::Running, connections, bytes_forwarded, Some(error)));
                        }
                        Err(error) => {
                            emit_tunnel(job.event(TunnelState::Running, connections, bytes_forwarded, Some(error.to_string())));
                        }
                    }
                }
            }
            accepted = job.listener.accept() => {
                match accepted {
                    Ok((mut local, _peer)) => {
                        if workers.len() >= MAX_CONNECTIONS {
                            drop(local);
                            emit_tunnel(job.event(TunnelState::Running, connections, bytes_forwarded, Some("SOCKS connection limit reached".into())));
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
                        emit_tunnel(job.event(TunnelState::Running, connections, bytes_forwarded, None));
                    }
                    Err(error) => {
                        failed = true;
                        emit_tunnel(job.event(TunnelState::Failed, connections, bytes_forwarded, Some(format!("SOCKS listener failed: {error}"))));
                        break;
                    }
                }
            }
        }
    }

    workers.shutdown().await;
    if !failed {
        emit_tunnel(job.event(TunnelState::Stopped, connections, bytes_forwarded, None));
    }
    manager.finish_tunnel(&job.tunnel_id);
}

async fn run_remote_forward(
    mut emit_tunnel: impl FnMut(SshTunnelEvent),
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
                "remote forward cancelled before the server listener was ready; reconnect if the SSH connection closed for listener cleanup".to_owned()
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
                    emit_tunnel(job.event(TunnelState::Failed, 0, 0, Some(message.clone())));
                    let _ = reply.send(Err(message));
                    manager.finish_tunnel(&job.tunnel_id);
                    return;
                }
            }
        }
    };
    job.bind_port = remote_port;

    emit_tunnel(job.event(TunnelState::Listening, connections, bytes_forwarded, None));
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
    emit_tunnel(job.event(TunnelState::Running, connections, bytes_forwarded, None));

    loop {
        tokio::select! {
            changed = job.cancel.changed() => {
                if changed.is_err() || *job.cancel.borrow() {
                    emit_tunnel(job.event(TunnelState::Stopping, connections, bytes_forwarded, None));
                    break;
                }
            }
            worker = workers.join_next(), if !workers.is_empty() => {
                if let Some(result) = worker {
                    match result {
                        Ok(Ok((uploaded, downloaded))) => {
                            bytes_forwarded = bytes_forwarded.saturating_add(uploaded).saturating_add(downloaded);
                            emit_tunnel(job.event(TunnelState::Running, connections, bytes_forwarded, None));
                        }
                        Ok(Err(error)) => {
                            emit_tunnel(job.event(TunnelState::Running, connections, bytes_forwarded, Some(error)));
                        }
                        Err(error) => {
                            emit_tunnel(job.event(TunnelState::Running, connections, bytes_forwarded, Some(error.to_string())));
                        }
                    }
                }
            }
            forwarded = connection.next_forwarded_channel() => {
                match forwarded {
                    Some(channel) => {
                        if workers.len() >= MAX_CONNECTIONS {
                            drop(channel);
                            emit_tunnel(job.event(TunnelState::Running, connections, bytes_forwarded, Some("remote tunnel connection limit reached".into())));
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
                        emit_tunnel(job.event(TunnelState::Running, connections, bytes_forwarded, None));
                    }
                    None => {
                        failed = true;
                        emit_tunnel(job.event(TunnelState::Failed, connections, bytes_forwarded, Some("SSH connection closed while remote forwarding was active".into())));
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
        emit_tunnel(job.event(
            TunnelState::Failed,
            connections,
            bytes_forwarded,
            Some(format!("could not cancel remote listener: {error}")),
        ));
    }
    if !failed {
        emit_tunnel(job.event(TunnelState::Stopped, connections, bytes_forwarded, None));
    }
    manager.finish_tunnel(&job.tunnel_id);
}

async fn run_transfer(
    app: AppHandle,
    manager: SshManager,
    connection: Arc<SshConnection>,
    job: TransferJob,
    mut cancel: oneshot::Receiver<()>,
) {
    let mut lifecycle = TransferLifecycle::new();
    let mut transferred = 0_u64;
    let mut total_bytes = None;
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
    let mut file = create_local_download_file(&temporary).await?;
    let copied = match connection
        .scp_download_with_cancel(remote_path, &mut file, cancel, |bytes, total| {
            on_progress(bytes, Some(total));
        })
        .await
    {
        Ok(copied) => copied,
        Err(error) => {
            remove_partial_download(file, &temporary).await?;
            return Err(error);
        }
    };
    if let Err(error) = file.sync_all().await {
        remove_partial_download(file, &temporary).await?;
        return Err(SshError::LocalIo(error));
    }
    drop(file);
    if !overwrite && download_destination_exists(destination, &temporary).await? {
        remove_partial_download_path(&temporary).await?;
        return Err(SshError::Scp(
            "download destination appeared during transfer".into(),
        ));
    }
    if let Err(error) = commit_local_file(&temporary, destination, overwrite, cancel) {
        remove_partial_download_path(&temporary).await?;
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
    let metadata = local_upload_metadata(source).await?;
    if !metadata.is_file() {
        return Err(SshError::LocalIo(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "SCP upload source is not a regular file",
        )));
    }

    let mut file = open_local_upload_file(source).await?;
    let source_size = file.metadata().await.map_err(SshError::LocalIo)?.len();
    let sftp = connection.open_sftp().await?;
    if sftp.try_exists(remote_path).await? {
        if let Err(error) = sftp.check_upload_destination(remote_path).await {
            let _ = sftp.close().await;
            return Err(error);
        }
        if !overwrite {
            let _ = sftp.close().await;
            return Err(SshError::Scp(
                "upload destination already exists; enable overwrite explicitly".into(),
            ));
        }
    }

    let temporary = remote_part_path(remote_path, &Uuid::new_v4().to_string())?;
    if let Err(error) = sftp.prepare_upload_temporary(&temporary, cancel).await {
        let _ = sftp.close().await;
        return Err(error);
    }
    let copied = match connection
        .scp_upload_with_cancel(&temporary, source_size, &mut file, cancel, |bytes| {
            on_progress(bytes, Some(source_size))
        })
        .await
    {
        Ok(copied) => copied,
        Err(error) => {
            let cleanup = sftp.cleanup_temporary_file(&temporary).await;
            let _ = sftp.close().await;
            cleanup?;
            return Err(error);
        }
    };
    if !overwrite && upload_destination_exists(&sftp, remote_path, &temporary).await? {
        let cleanup = sftp.cleanup_temporary_file(&temporary).await;
        let _ = sftp.close().await;
        cleanup?;
        return Err(SshError::Scp(
            "upload destination appeared during transfer".into(),
        ));
    }
    let promotion = sftp
        .promote_uploaded_file_with_cancel(&temporary, remote_path, overwrite, cancel)
        .await;
    let _ = sftp.close().await;
    promotion?;
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
    let sftp = connection.open_sftp().await?;
    let result = async {
        let (total, is_directory) = sftp.file_info(remote_path).await?;
        if is_directory {
            if !recursive {
                return Err(SshError::RemoteDownloadSourceDirectory);
            }
            return download_directory(
                &sftp,
                remote_path,
                destination,
                overwrite,
                cancel,
                &mut on_progress,
            )
            .await;
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
        let mut file = create_local_download_file(&temporary).await?;
        let copied = match sftp
            .download_to_with_cancel(remote_path, &mut file, cancel, |bytes| {
                on_progress(bytes, total);
            })
            .await
        {
            Ok(copied) => copied,
            Err(error) => {
                remove_partial_download(file, &temporary).await?;
                return Err(error);
            }
        };
        if let Err(error) = file.sync_all().await {
            remove_partial_download(file, &temporary).await?;
            return Err(SshError::LocalIo(error));
        }
        drop(file);
        if !overwrite && download_destination_exists(destination, &temporary).await? {
            remove_partial_download_path(&temporary).await?;
            return Err(SshError::Sftp(
                "download destination appeared during transfer".into(),
            ));
        }
        if let Err(error) = commit_local_file(&temporary, destination, overwrite, cancel) {
            remove_partial_download_path(&temporary).await?;
            return Err(error);
        }
        Ok(copied)
    }
    .await;
    // File closes and local commits decide the transfer result. Session-close
    // failure after a commit must not relabel completed files as failed.
    let _ = sftp.close().await;
    result
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
    let metadata = local_upload_metadata(source).await?;
    if metadata.is_dir() {
        if !recursive {
            return Err(SshError::LocalIo(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "upload source is a directory; enable recursive transfer",
            )));
        }
        let sftp = connection.open_sftp().await?;
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
    let mut file = open_local_upload_file(source).await?;
    let source_size = file.metadata().await.map_err(SshError::LocalIo)?.len();
    let sftp = connection.open_sftp().await?;
    if sftp.try_exists(remote_path).await? {
        if let Err(error) = sftp.check_upload_destination(remote_path).await {
            let _ = sftp.close().await;
            return Err(error);
        }
        if !overwrite {
            let _ = sftp.close().await;
            return Err(SshError::Sftp(
                "upload destination already exists; enable overwrite explicitly".into(),
            ));
        }
    }

    let temporary = remote_part_path(remote_path, &Uuid::new_v4().to_string())?;
    let copied = match sftp
        .upload_temporary_from_with_cancel(&mut file, &temporary, cancel, |bytes| {
            on_progress(bytes, Some(source_size));
        })
        .await
    {
        Ok(copied) => copied,
        Err(error) => {
            let _ = sftp.close().await;
            return Err(error);
        }
    };
    if !overwrite && upload_destination_exists(&sftp, remote_path, &temporary).await? {
        let cleanup = sftp.cleanup_temporary_file(&temporary).await;
        let _ = sftp.close().await;
        cleanup?;
        return Err(SshError::Sftp(
            "upload destination appeared during transfer".into(),
        ));
    }
    let promotion = sftp
        .promote_uploaded_file_with_cancel(&temporary, remote_path, overwrite, cancel)
        .await;
    let _ = sftp.close().await;
    promotion?;
    Ok(copied)
}

const MAX_RECURSIVE_ENTRIES: usize = 100_000;

async fn local_upload_metadata(source: &Path) -> Result<std::fs::Metadata, SshError> {
    let metadata = fs::symlink_metadata(source)
        .await
        .map_err(SshError::LocalIo)?;
    if metadata.file_type().is_symlink() {
        return Err(SshError::LocalUploadSymlink);
    }
    Ok(metadata)
}

async fn open_local_upload_file(source: &Path) -> Result<fs::File, SshError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let file = options.open(source).await.map_err(|error| {
        #[cfg(unix)]
        if error.raw_os_error() == Some(libc::ELOOP) {
            return SshError::LocalUploadSymlink;
        }
        SshError::LocalIo(error)
    })?;
    if !file.metadata().await.map_err(SshError::LocalIo)?.is_file() {
        return Err(SshError::LocalIo(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "upload source is not a regular file",
        )));
    }
    Ok(file)
}

type RemoteDownloadFile = (String, PathBuf, Option<u64>);
type LocalUploadFile = (PathBuf, String, u64);

struct FileTransferProgress<'a, F> {
    base: u64,
    total: Option<u64>,
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
    check_transfer_cancelled(cancel)?;
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
        check_transfer_cancelled(cancel)?;
        ensure_local_download_directory(&directory).await?;
    }

    let mut transferred = 0_u64;
    on_progress(0, total);
    for (remote_path, local_path, size) in files {
        check_transfer_cancelled(cancel)?;
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
        on_progress(transferred, total);
    }
    check_transfer_cancelled(cancel)?;
    Ok(transferred)
}

async fn ensure_local_download_directory(path: &Path) -> Result<(), SshError> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => return Ok(()),
        Ok(_) => {
            return Err(SshError::Sftp(
                "recursive download refuses a symlink or non-directory destination".into(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(SshError::LocalIo(error)),
    }
    match fs::create_dir(path).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(SshError::LocalIo(error)),
    }
    match fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(SshError::Sftp(
            "recursive download refuses a symlink or non-directory destination".into(),
        )),
        Err(error) => Err(SshError::LocalIo(error)),
    }
}

async fn collect_remote_files(
    sftp: &mobarust_ssh::SftpConnection,
    remote_root: &str,
    local_root: &Path,
    cancel: &mut oneshot::Receiver<()>,
) -> Result<(Vec<RemoteDownloadFile>, Vec<PathBuf>, Option<u64>), SshError> {
    let mut pending = VecDeque::from([(remote_root.to_owned(), local_root.to_owned())]);
    let mut files = Vec::new();
    let mut directories = vec![local_root.to_owned()];
    let mut total = Some(0_u64);
    let mut seen = 0_usize;

    while let Some((remote_directory, local_directory)) = pending.pop_front() {
        check_transfer_cancelled(cancel)?;
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
            } else if entry.is_regular {
                total = add_transfer_size(total, entry.size);
                files.push((remote_path, local_path, entry.size));
            } else {
                return Err(SshError::Sftp(
                    "recursive download supports regular files and directories only".into(),
                ));
            }
        }
    }
    Ok((files, directories, total))
}

async fn download_file_atomically<F>(
    sftp: &mobarust_ssh::SftpConnection,
    remote_path: &str,
    destination: &Path,
    total_size: Option<u64>,
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
        ensure_local_download_directory(parent).await?;
    }
    let temporary = local_part_path(destination)?;
    let mut file = create_local_download_file(&temporary).await?;
    let copied = match sftp
        .download_to_with_cancel(remote_path, &mut file, progress.cancel, |bytes| {
            (progress.on_progress)(
                progress.base.saturating_add(bytes),
                progress
                    .total
                    .map(|total| total.max(total_size.unwrap_or_default())),
            );
        })
        .await
    {
        Ok(copied) => copied,
        Err(error) => {
            remove_partial_download(file, &temporary).await?;
            return Err(error);
        }
    };
    if let Err(error) = file.sync_all().await {
        remove_partial_download(file, &temporary).await?;
        return Err(SshError::LocalIo(error));
    }
    drop(file);
    if !overwrite && download_destination_exists(destination, &temporary).await? {
        remove_partial_download_path(&temporary).await?;
        return Err(SshError::Sftp(
            "download destination appeared during transfer".into(),
        ));
    }
    if let Err(error) = commit_local_file(&temporary, destination, overwrite, progress.cancel) {
        remove_partial_download_path(&temporary).await?;
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
    check_transfer_cancelled(cancel)?;
    ensure_remote_directory(sftp, remote_root).await?;
    for directory in directories {
        check_transfer_cancelled(cancel)?;
        ensure_remote_directory(sftp, &directory).await?;
    }

    let mut transferred = 0_u64;
    on_progress(0, Some(total));
    for (local_path, remote_path, size) in files {
        check_transfer_cancelled(cancel)?;
        let mut progress = FileTransferProgress {
            base: transferred,
            total: Some(total),
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
    check_transfer_cancelled(cancel)?;
    Ok(transferred)
}

async fn collect_local_files(
    local_root: &Path,
    remote_root: &str,
    cancel: &mut oneshot::Receiver<()>,
) -> Result<(Vec<LocalUploadFile>, Vec<String>, u64), SshError> {
    check_transfer_cancelled(cancel)?;
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
        check_transfer_cancelled(cancel)?;
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
            let name = local_transfer_name(entry.file_name())?;
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
        sftp.check_upload_destination(remote_path).await?;
        if !overwrite {
            return Err(SshError::Sftp(
                "upload destination already exists; enable overwrite explicitly".into(),
            ));
        }
    }
    let temporary = remote_part_path(remote_path, &Uuid::new_v4().to_string())?;
    let mut file = open_local_upload_file(source).await?;
    let copied = sftp
        .upload_temporary_from_with_cancel(&mut file, &temporary, progress.cancel, |bytes| {
            (progress.on_progress)(
                progress.base.saturating_add(bytes),
                progress.total.map(|total| total.max(total_size)),
            );
        })
        .await?;
    if !overwrite && upload_destination_exists(sftp, remote_path, &temporary).await? {
        sftp.cleanup_temporary_file(&temporary).await?;
        return Err(SshError::Sftp(
            "upload destination appeared during transfer".into(),
        ));
    }
    sftp.promote_uploaded_file_with_cancel(&temporary, remote_path, overwrite, progress.cancel)
        .await?;
    Ok(copied)
}

async fn ensure_remote_directory(
    sftp: &mobarust_ssh::SftpConnection,
    path: &str,
) -> Result<(), SshError> {
    if path.trim().is_empty() || path == "." || path == "/" {
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
        if !sftp.is_real_directory(current.clone()).await? {
            return Err(SshError::Sftp(
                "recursive upload refuses a remote symlink or non-directory component".into(),
            ));
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

fn local_transfer_name(name: std::ffi::OsString) -> Result<String, SshError> {
    name.into_string()
        .map_err(|_| SshError::Sftp("recursive upload requires UTF-8 local file names".into()))
}

fn add_transfer_size(total: Option<u64>, size: Option<u64>) -> Option<u64> {
    total?.checked_add(size?)
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

async fn upload_destination_exists(
    sftp: &mobarust_ssh::SftpConnection,
    remote_path: &str,
    temporary: &str,
) -> Result<bool, SshError> {
    match sftp.try_exists(remote_path).await {
        Ok(exists) => Ok(exists),
        Err(error) => {
            sftp.cleanup_temporary_file(temporary).await?;
            Err(error)
        }
    }
}

async fn download_destination_exists(
    destination: &Path,
    temporary: &Path,
) -> Result<bool, SshError> {
    match fs::try_exists(destination).await {
        Ok(exists) => Ok(exists),
        Err(error) => {
            remove_partial_download_path(temporary).await?;
            Err(SshError::LocalIo(error))
        }
    }
}

fn validate_remote_file_path(path: &str) -> Result<String, SshManagerError> {
    if path.trim().is_empty() || path == "." || path == "/" || path.contains('\0') {
        return Err(SshManagerError::InvalidRequest(
            "remote file path must identify a non-root path".into(),
        ));
    }
    Ok(path.to_owned())
}

fn validate_remote_directory_path(path: &str) -> Result<String, SshManagerError> {
    if path.trim().is_empty() || path.contains('\0') {
        return Err(SshManagerError::InvalidRequest(
            "remote directory path cannot be empty or contain NUL".into(),
        ));
    }
    Ok(path.to_owned())
}

fn validate_remote_mutation_path(path: &str) -> Result<String, SshManagerError> {
    mobarust_ssh::validate_remote_mutation_path(path)
        .map_err(|error| SshManagerError::InvalidRequest(error.to_string()))
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
    Ok(destination.with_file_name(format!(".{name}.mobarust-{}.part", Uuid::new_v4())))
}

async fn create_local_download_file(temporary: &Path) -> Result<fs::File, SshError> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(temporary).await.map_err(SshError::LocalIo)
}

async fn remove_partial_download(file: fs::File, temporary: &Path) -> Result<(), SshError> {
    drop(file);
    remove_partial_download_path(temporary).await
}

async fn remove_partial_download_path(temporary: &Path) -> Result<(), SshError> {
    match fs::remove_file(temporary).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(SshError::LocalPartialDownloadCleanupFailed),
    }
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

fn check_transfer_cancelled(cancel: &mut oneshot::Receiver<()>) -> Result<(), SshError> {
    match cancel.try_recv() {
        Err(oneshot::error::TryRecvError::Empty) => Ok(()),
        _ => Err(SshError::Cancelled),
    }
}

fn commit_local_file(
    temporary: &Path,
    destination: &Path,
    overwrite: bool,
    cancel: &mut oneshot::Receiver<()>,
) -> Result<(), SshError> {
    // The copy and fsync may have finished before a queued cancellation arrived.
    // Do not replace the destination when cancellation preceded this commit.
    check_transfer_cancelled(cancel)?;
    match std::fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(SshError::Sftp(
                "download destination cannot be a symlink".into(),
            ));
        }
        Ok(metadata) if metadata.is_dir() => {
            return Err(SshError::Sftp("download destination is a directory".into()));
        }
        Ok(_) if !overwrite => {
            return Err(SshError::Sftp("download destination already exists".into()));
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(SshError::LocalIo(error));
        }
        _ => {}
    }

    #[cfg(unix)]
    if !overwrite {
        // Linking a complete sibling fails if another process created the destination.
        std::fs::hard_link(temporary, destination).map_err(SshError::LocalIo)?;
        std::fs::remove_file(temporary).map_err(SshError::LocalIo)?;
        return Ok(());
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };

        let temporary = temporary
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let destination = destination
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let flags = if overwrite {
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH
        } else {
            MOVEFILE_WRITE_THROUGH
        };
        if unsafe { MoveFileExW(temporary.as_ptr(), destination.as_ptr(), flags) } == 0 {
            return Err(SshError::LocalIo(std::io::Error::last_os_error()));
        }
        Ok(())
    }

    #[cfg(not(windows))]
    {
        std::fs::rename(temporary, destination).map_err(SshError::LocalIo)
    }
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

fn validate_tunnel_host(value: &str, field: &str) -> Result<(), SshManagerError> {
    validate_forward_host(value).map_err(|_| {
        SshManagerError::InvalidRequest(format!(
            "{field} is empty, too long, or contains control characters"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_SERVER_ALIVE_INTERVAL_SECONDS, MAX_SSH_JUMP_HOSTS, ReconnectOutcome, SshConnectRequest,
        SshManager, SshManagerError, SshTransferRequest, TRANSFER_PROGRESS_MIN_INTERVAL,
        TransferProtocol, add_transfer_size, collect_local_files, commit_local_file,
        local_part_path, next_shell_reconnect_count, reconnect_with_backoff, remote_child_path,
        remove_partial_download, remove_partial_download_path, server_alive_interval_duration,
        should_emit_transfer_progress, transfer_metrics, validate_local_file_path,
        validate_remote_directory_path, validate_remote_file_path, validate_remote_mutation_path,
        validate_ssh_connection_policy, validate_transfer_component, validate_tunnel_host,
    };
    #[cfg(unix)]
    use super::{
        download_destination_exists, ensure_local_download_directory, local_transfer_name,
        local_upload_metadata, open_local_upload_file,
    };
    use std::fs;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;
    use tokio::sync::{oneshot, watch};

    #[test]
    fn attach_replays_pending_output_before_live_output() {
        let manager = SshManager::default();
        let chunks = vec!["\x1b[3".to_owned(), "2mready ☃\x1b[0m\r\n".to_owned()];
        let _commands = queue_test_session(&manager, "fixture");
        manager
            .sessions
            .lock()
            .unwrap()
            .get_mut("fixture")
            .unwrap()
            .pending_output = chunks.clone();
        let mut emitted = Vec::new();
        manager
            .attach("fixture", |data| {
                assert!(
                    matches!(
                        manager.sessions.try_lock(),
                        Err(std::sync::TryLockError::WouldBlock)
                    ),
                    "live publishers cannot enter while the backlog is being emitted"
                );
                emitted.push(data);
            })
            .unwrap();
        assert_eq!(
            emitted, chunks,
            "preserve fragmented ANSI and Unicode output in order"
        );
        let sessions = manager.sessions.lock().unwrap();
        assert!(sessions["fixture"].attached);
        assert!(sessions["fixture"].pending_output.is_empty());
        drop(sessions);
        manager
            .attach("fixture", |_| panic!("backlog must not be replayed twice"))
            .unwrap();
        assert!(matches!(
            manager.attach("missing", |_| panic!("missing session cannot emit")),
            Err(SshManagerError::MissingSession(_))
        ));
    }

    #[test]
    fn file_paths_preserve_significant_whitespace() {
        assert_eq!(
            validate_remote_file_path("/srv/report ").unwrap(),
            "/srv/report "
        );
        assert_eq!(
            validate_remote_directory_path("/srv/project ").unwrap(),
            "/srv/project "
        );
        assert_eq!(
            validate_remote_mutation_path("/srv/report ").unwrap(),
            "/srv/report "
        );
        assert_eq!(
            validate_local_file_path("/tmp/report ").unwrap(),
            std::path::PathBuf::from("/tmp/report ")
        );
        assert!(validate_remote_file_path("   ").is_err());
        assert!(validate_remote_file_path("/").is_err());
        assert!(validate_remote_mutation_path("/").is_err());
    }

    #[tokio::test]
    async fn remote_mutations_require_named_paths_before_session_lookup() {
        let manager = SshManager::default();
        let mut failures = Vec::new();
        for path in [
            "",
            " \t ",
            "/srv/nul\0name",
            "/",
            ".",
            "//",
            "///",
            "./",
            "././",
            "..",
            "../",
            "../../",
            "/./",
            "/..",
            "/../",
            "/./.././",
            "/srv/.",
            "/srv/./",
            "/srv/..",
            "/srv/..///",
            "folder/.",
            "folder/..",
        ] {
            for (operation, result) in [
                (
                    "rename-from",
                    manager
                        .rename_remote("missing", path.into(), "report".into())
                        .await,
                ),
                (
                    "rename-to",
                    manager
                        .rename_remote("missing", "report".into(), path.into())
                        .await,
                ),
                (
                    "delete",
                    manager.delete_remote("missing", path.into()).await,
                ),
                (
                    "mkdir",
                    manager
                        .create_remote_directory("missing", path.into())
                        .await,
                ),
                (
                    "permissions",
                    manager
                        .set_remote_permissions("missing", path.into(), 0o600)
                        .await,
                ),
            ] {
                if !matches!(result, Err(SshManagerError::InvalidRequest(_))) {
                    failures.push(format!("{operation}: {path:?}"));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "mutation aliases reached session lookup: {failures:?}"
        );
        for (path, expected) in [
            ("/srv/report ", "/srv/report "),
            (" ./report ", " ./report "),
            ("./report", "./report"),
            ("../report", "../report"),
            ("/srv/../report", "/srv/../report"),
            (".notes", ".notes"),
            ("...", "..."),
            ("résumé file", "résumé file"),
            ("/srv/folder/", "/srv/folder"),
            ("/srv/folder///", "/srv/folder"),
            ("/srv//report", "/srv//report"),
        ] {
            assert_eq!(validate_remote_mutation_path(path).unwrap(), expected);
        }
    }

    #[test]
    fn server_alive_interval_maps_to_a_bounded_native_duration() {
        assert_eq!(server_alive_interval_duration(None).unwrap(), None);
        assert_eq!(server_alive_interval_duration(Some(0)).unwrap(), None);
        assert_eq!(
            server_alive_interval_duration(Some(30)).unwrap(),
            Some(Duration::from_secs(30))
        );
        assert!(
            server_alive_interval_duration(Some(MAX_SERVER_ALIVE_INTERVAL_SECONDS + 1)).is_err()
        );
    }

    #[tokio::test]
    async fn oversized_ssh_write_is_rejected_before_session_lookup() {
        let data = "x".repeat(mobarust_core::MAX_TERMINAL_INPUT_BYTES + 1);
        let error = SshManager::default()
            .write("missing", data)
            .await
            .expect_err("oversized SSH input must be rejected");
        assert!(matches!(
            error,
            SshManagerError::Input(mobarust_core::TerminalInputError::TooLarge)
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn upload_source_metadata_refuses_symlinks() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let file = directory.path().join("file.txt");
        let missing = directory.path().join("missing");
        fs::write(&file, b"private contents").unwrap();
        assert!(local_upload_metadata(&file).await.unwrap().is_file());
        assert!(
            local_upload_metadata(directory.path())
                .await
                .unwrap()
                .is_dir()
        );

        for (name, target) in [
            ("file-link", file.as_path()),
            ("directory-link", directory.path()),
            ("dangling-link", missing.as_path()),
        ] {
            let link = directory.path().join(name);
            symlink(target, &link).unwrap();
            assert!(matches!(
                local_upload_metadata(&link).await,
                Err(mobarust_ssh::SshError::LocalUploadSymlink)
            ));
        }
        assert_eq!(fs::read(file).unwrap(), b"private contents");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn recursive_download_directory_refuses_existing_symlinks() {
        use std::os::unix::fs::symlink;

        let selected = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let redirected = selected.path().join("subdirectory");
        symlink(outside.path(), &redirected).unwrap();
        assert!(matches!(
            ensure_local_download_directory(&redirected).await,
            Err(mobarust_ssh::SshError::Sftp(_))
        ));
        assert!(fs::read_dir(outside.path()).unwrap().next().is_none());

        let real = selected.path().join("real-subdirectory");
        ensure_local_download_directory(&real).await.unwrap();
        assert!(fs::symlink_metadata(&real).unwrap().is_dir());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn upload_open_refuses_a_symlink_swapped_after_metadata_check() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let selected = directory.path().join("selected.txt");
        let target = directory.path().join("private.txt");
        fs::write(&selected, b"selected").unwrap();
        fs::write(&target, b"private").unwrap();
        assert!(local_upload_metadata(&selected).await.unwrap().is_file());
        assert_eq!(
            open_local_upload_file(&selected)
                .await
                .unwrap()
                .metadata()
                .await
                .unwrap()
                .len(),
            8
        );
        assert!(open_local_upload_file(directory.path()).await.is_err());
        fs::remove_file(&selected).unwrap();
        symlink(&target, &selected).unwrap();

        assert!(matches!(
            open_local_upload_file(&selected).await,
            Err(mobarust_ssh::SshError::LocalUploadSymlink)
        ));
        assert_eq!(fs::read(target).unwrap(), b"private");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn download_recheck_error_removes_temporary_file() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let destination = directory.path().join("download.txt");
        let temporary = directory.path().join(".download.txt.part");
        fs::write(&temporary, b"downloaded contents").unwrap();
        symlink(&destination, &destination).unwrap();
        assert!(fs::metadata(&destination).is_err());

        assert!(
            download_destination_exists(&destination, &temporary)
                .await
                .is_err()
        );
        assert!(!temporary.exists());
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn download_parts_are_private_and_exclusive() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        use tokio::io::AsyncWriteExt;
        let directory = tempdir().unwrap();
        let temporary = directory.path().join("private-download.part");
        let mut file = super::create_local_download_file(&temporary).await.unwrap();
        assert_eq!(
            fs::metadata(&temporary).unwrap().permissions().mode() & 0o7777,
            0o600
        );
        file.write_all(b"private bytes").await.unwrap();
        file.sync_all().await.unwrap();
        drop(file);
        assert!(super::create_local_download_file(&temporary).await.is_err());
        assert_eq!(fs::read(&temporary).unwrap(), b"private bytes");
        let link = directory.path().join("occupied-link.part");
        symlink(&temporary, &link).unwrap();
        assert!(super::create_local_download_file(&link).await.is_err());
        assert_eq!(fs::read_link(&link).unwrap(), temporary);
        assert_eq!(fs::read(&temporary).unwrap(), b"private bytes");
    }

    #[tokio::test]
    async fn failed_download_closes_partial_file_before_removal() {
        let directory = tempdir().unwrap();
        let temporary = directory.path().join(".download.txt.mobarust.part");
        fs::write(&temporary, b"partial download").unwrap();
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&temporary)
            .await
            .unwrap();

        remove_partial_download(file, &temporary).await.unwrap();

        assert!(!temporary.exists());
        fs::write(&temporary, b"next attempt").unwrap();
    }

    #[tokio::test]
    async fn failed_partial_download_cleanup_is_reported() {
        let directory = tempdir().unwrap();
        let open_file_path = directory.path().join("open-file");
        fs::write(&open_file_path, b"partial download").unwrap();
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&open_file_path)
            .await
            .unwrap();

        let error = remove_partial_download(file, directory.path())
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            mobarust_ssh::SshError::LocalPartialDownloadCleanupFailed
        ));
        assert!(directory.path().exists());
    }

    #[test]
    fn local_downloads_use_distinct_sibling_partial_files() {
        let directory = tempdir().unwrap();
        let destination = directory.path().join("download.txt");
        let first = local_part_path(&destination).unwrap();
        let second = local_part_path(&destination).unwrap();

        assert_ne!(first, second);
        assert_eq!(first.parent(), destination.parent());
    }

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
    fn recursive_download_totals_stay_unknown_for_missing_sizes_or_overflow() {
        assert_eq!(add_transfer_size(Some(10), Some(5)), Some(15));
        assert_eq!(add_transfer_size(Some(10), None), None);
        assert_eq!(add_transfer_size(None, Some(5)), None);
        assert_eq!(add_transfer_size(Some(u64::MAX), Some(1)), None);
    }

    #[cfg(unix)]
    #[test]
    fn recursive_upload_rejects_non_utf8_local_file_names() {
        use std::os::unix::ffi::OsStringExt;

        let error = local_transfer_name(std::ffi::OsString::from_vec(vec![0xff]))
            .expect_err("non-UTF-8 file names must be rejected");
        assert!(matches!(error, mobarust_ssh::SshError::Sftp(_)));
        assert_eq!(error.to_string(), "SFTP operation failed");
        assert_eq!(
            local_transfer_name(std::ffi::OsString::from("café.txt")).unwrap(),
            "café.txt"
        );
    }

    #[test]
    fn tunnel_hosts_are_bounded_and_control_free_before_opening_channels() {
        assert!(validate_tunnel_host("db.internal", "target host").is_ok());
        assert!(validate_tunnel_host("127.0.0.1", "bind host").is_ok());
        for invalid in ["", "   ", "db\ninternal", &"h".repeat(256)] {
            assert!(validate_tunnel_host(invalid, "target host").is_err());
        }
    }

    #[test]
    fn local_download_commit_replaces_only_after_a_complete_temporary_file_exists() {
        let (_cancel_sender, mut cancel) = oneshot::channel();
        let directory = tempdir().unwrap();
        let destination = directory.path().join("download.txt");
        let temporary = directory.path().join(".download.txt.mobarust.part");
        fs::write(&destination, b"old complete file").unwrap();
        fs::write(&temporary, b"new complete file").unwrap();

        commit_local_file(&temporary, &destination, true, &mut cancel).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"new complete file");
        assert!(!temporary.exists());
    }

    #[test]
    fn local_download_commit_refuses_existing_destination_without_overwrite() {
        let (_cancel_sender, mut cancel) = oneshot::channel();
        let directory = tempdir().unwrap();
        let destination = directory.path().join("download.txt");
        let temporary = directory.path().join(".download.txt.mobarust.part");
        fs::write(&destination, b"original").unwrap();
        fs::write(&temporary, b"replacement").unwrap();

        assert!(commit_local_file(&temporary, &destination, false, &mut cancel).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"original");
        assert_eq!(fs::read(&temporary).unwrap(), b"replacement");
    }

    #[test]
    fn local_download_commit_without_overwrite_promotes_complete_file() {
        let (_cancel_sender, mut cancel) = oneshot::channel();
        let directory = tempdir().unwrap();
        let destination = directory.path().join("download.txt");
        let temporary = directory.path().join(".download.txt.mobarust.part");
        fs::write(&temporary, b"complete file").unwrap();

        commit_local_file(&temporary, &destination, false, &mut cancel).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"complete file");
        assert!(!temporary.exists());
    }

    #[cfg(unix)]
    #[test]
    fn local_download_commit_refuses_symlink_destination_without_touching_target() {
        let (_cancel_sender, mut cancel) = oneshot::channel();
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let target = directory.path().join("target.txt");
        let destination = directory.path().join("download.txt");
        let temporary = directory.path().join(".download.txt.mobarust.part");
        fs::write(&target, b"target remains unchanged").unwrap();
        symlink(&target, &destination).unwrap();
        fs::write(&temporary, b"replacement").unwrap();

        assert!(commit_local_file(&temporary, &destination, true, &mut cancel).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"target remains unchanged");
        assert_eq!(fs::read(&temporary).unwrap(), b"replacement");
    }

    fn queue_test_session(
        manager: &SshManager,
        id: &str,
    ) -> tokio::sync::mpsc::Receiver<super::SshCommand> {
        let (sender, commands) = tokio::sync::mpsc::channel(super::COMMAND_CAPACITY);
        manager.sessions.lock().unwrap().insert(
            id.into(),
            super::SessionState {
                sender,
                size: watch::channel((80, 24)).0,
                close: watch::channel(false).0,
                finished: watch::channel(false).1,
                attached: false,
                pending_output: Vec::new(),
                output_decoder: mobarust_core::Utf8OutputDecoder::default(),
            },
        );
        commands
    }

    #[tokio::test]
    async fn remote_text_open_preserves_explicit_encoding_and_path_validation() {
        use super::SshCommand;
        use mobarust_ssh::RemoteTextEncoding;

        let manager = SshManager::default();
        let mut commands = queue_test_session(&manager, "editor");
        tokio::time::timeout(Duration::from_secs(2), async {
            for selected in [RemoteTextEncoding::Utf8, RemoteTextEncoding::Windows1252] {
                let (result, ()) = tokio::join!(
                    manager.open_remote_text_file("editor", "./café.conf".into(), selected),
                    async {
                        match commands.recv().await.unwrap() {
                            SshCommand::OpenTextFile { path, encoding, reply } => {
                                assert_eq!(path, "./café.conf");
                                assert_eq!(encoding, selected);
                                reply.send(Err("controlled read refusal".into())).unwrap();
                            }
                            _ => panic!("unexpected editor command"),
                        }
                    }
                );
                assert!(matches!(result, Err(SshManagerError::InvalidRequest(message)) if message == "controlled read refusal"));
            }
            assert!(manager.open_remote_text_file("editor", "bad\0path".into(), RemoteTextEncoding::Windows1252).await.is_err());
            assert!(commands.try_recv().is_err(), "invalid paths must not queue a read");
        }).await.expect("editor command deadline");
    }

    fn authentication_test_context(
        manager: &SshManager,
    ) -> (
        super::AuthPromptContext,
        tokio::sync::mpsc::UnboundedReceiver<serde_json::Value>,
    ) {
        let (events, receiver) = tokio::sync::mpsc::unbounded_channel();
        let context = super::AuthPromptContext {
            manager: manager.clone(),
            events: tauri::ipc::Channel::new(move |body| {
                let _ = events.send(
                    body.deserialize()
                        .expect("authentication event must serialize"),
                );
                Ok(())
            }),
            host: "127.0.0.1".into(),
            port: 22,
            username: "fixture".into(),
            terminal_id: None,
        };
        (context, receiver)
    }

    fn authentication_test_challenge() -> mobarust_ssh::KeyboardInteractiveChallenge {
        mobarust_ssh::KeyboardInteractiveChallenge {
            name: "Fixture".into(),
            instructions: String::new(),
            prompts: vec!["Password".into(), "OTP".into()],
        }
    }

    #[tokio::test]
    async fn authentication_answers_are_one_shot_bounded_and_expire_with_the_waiter() {
        let manager = SshManager::default();
        let (context, mut events) = authentication_test_context(&manager);
        let attempt =
            tokio::spawn(async move { context.ask(authentication_test_challenge()).await });
        let challenge = events.recv().await.unwrap();
        assert_eq!(challenge["host"], "127.0.0.1");
        assert_eq!(challenge["username"], "fixture");
        let id = challenge["requestId"].as_str().unwrap();
        assert!(
            manager
                .answer_authentication(id, Some(vec!["one".into()]))
                .is_err()
        );
        assert!(
            manager
                .answer_authentication(
                    id,
                    Some(vec![
                        "s".repeat(mobarust_ssh::MAX_KEYBOARD_INTERACTIVE_RESPONSE_BYTES + 1),
                        "otp".into()
                    ])
                )
                .is_err()
        );
        manager
            .answer_authentication(id, Some(vec!["password".into(), "otp".into()]))
            .unwrap();
        let answers = attempt.await.unwrap().unwrap();
        assert_eq!(
            answers
                .iter()
                .map(mobarust_ssh::Secret::len)
                .collect::<Vec<_>>(),
            vec![8, 3]
        );
        assert_eq!(format!("{answers:?}"), "[<redacted>, <redacted>]");
        assert_eq!(
            events.recv().await.unwrap(),
            serde_json::json!({"event":"closed", "requestId":id})
        );
        assert!(
            manager
                .answer_authentication(id, Some(vec!["late".into(), "late".into()]))
                .is_err()
        );
        assert!(manager.authentication.lock().unwrap().is_empty());

        let (context, mut events) = authentication_test_context(&manager);
        let attempt =
            tokio::spawn(async move { context.ask(authentication_test_challenge()).await });
        let expired = events.recv().await.unwrap();
        let expired_id = expired["requestId"].as_str().unwrap();
        assert_ne!(expired_id, id);
        attempt.abort();
        assert!(attempt.await.unwrap_err().is_cancelled());
        assert_eq!(events.recv().await.unwrap()["event"], "closed");
        assert!(
            manager
                .answer_authentication(expired_id, Some(vec!["late".into(), "late".into()]))
                .is_err()
        );
        assert!(manager.authentication.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn concurrent_authentication_is_bounded_and_releases_only_the_retired_waiter() {
        tokio::time::timeout(Duration::from_secs(5), async {
            let manager = SshManager::default();
            let mut waiters = Vec::new();
            for index in 0..32 {
                let (mut context, mut events) = authentication_test_context(&manager);
                context.port = 10_000 + index;
                let attempt =
                    tokio::spawn(async move { context.ask(authentication_test_challenge()).await });
                let challenge = events.recv().await.unwrap();
                assert_eq!(challenge["port"], 10_000 + index);
                let id = challenge["requestId"].as_str().unwrap().to_owned();
                waiters.push((attempt, events, id));
            }
            assert_eq!(manager.authentication.lock().unwrap().len(), 32);

            let (overflow, mut events) = authentication_test_context(&manager);
            assert!(matches!(
                overflow.ask(authentication_test_challenge()).await,
                Err(mobarust_ssh::SshError::AuthenticationCancelled)
            ));
            assert!(
                matches!(
                    events.try_recv(),
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty)
                ),
                "overflow must not emit a challenge or disturb an owned waiter"
            );

            let (retired, mut events, id) = waiters.remove(15);
            retired.abort();
            assert!(retired.await.unwrap_err().is_cancelled());
            assert_eq!(
                events.recv().await.unwrap(),
                serde_json::json!({"event":"closed", "requestId":id})
            );
            assert_eq!(manager.authentication.lock().unwrap().len(), 31);
            assert!(manager.answer_authentication(&id, None).is_err());
            for (_, events, _) in &mut waiters {
                assert!(
                    matches!(
                        events.try_recv(),
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
                    ),
                    "another channel must not receive the retired request's closure"
                );
            }

            let (replacement, mut events) = authentication_test_context(&manager);
            let attempt =
                tokio::spawn(async move { replacement.ask(authentication_test_challenge()).await });
            let challenge = events.recv().await.unwrap();
            waiters.push((
                attempt,
                events,
                challenge["requestId"].as_str().unwrap().to_owned(),
            ));
            assert_eq!(manager.authentication.lock().unwrap().len(), 32);
            for (index, (attempt, mut events, id)) in waiters.into_iter().enumerate() {
                // Distinct lengths identify routing without exposing Secret contents.
                let password = "p".repeat(index + 1);
                let otp = "o".repeat(index + 33);
                manager
                    .answer_authentication(&id, Some(vec![password.clone(), otp.clone()]))
                    .unwrap();
                let answers = attempt.await.unwrap().unwrap();
                assert_eq!(answers.len(), 2);
                assert_eq!(answers[0].len(), password.len());
                assert_eq!(answers[1].len(), otp.len());
                assert_eq!(
                    events.recv().await.unwrap(),
                    serde_json::json!({"event":"closed", "requestId":id})
                );
                assert!(manager.answer_authentication(&id, None).is_err());
            }
            assert!(manager.authentication.lock().unwrap().is_empty());
        })
        .await
        .expect("bounded concurrent authentication cleanup");
    }

    #[tokio::test]
    async fn authentication_cancel_stops_reconnect_and_close_or_shutdown_refuses_answers() {
        for action in ["cancel", "close", "shutdown"] {
            let manager = SshManager::default();
            let _commands = queue_test_session(&manager, "terminal");
            let close = manager.sessions.lock().unwrap()["terminal"]
                .close
                .subscribe();
            let (mut context, mut events) = authentication_test_context(&manager);
            context.terminal_id = Some("terminal".into());
            let attempt =
                tokio::spawn(async move { context.ask(authentication_test_challenge()).await });
            let challenge = events.recv().await.unwrap();
            let id = challenge["requestId"].as_str().unwrap();
            if action == "cancel" {
                manager.answer_authentication(id, None).unwrap();
                assert!(matches!(
                    attempt.await.unwrap(),
                    Err(mobarust_ssh::SshError::AuthenticationCancelled)
                ));
                assert!(*close.borrow(), "Cancel prevents further reconnect prompts");
            } else {
                if action == "close" {
                    manager.close("terminal").await.unwrap();
                    assert!(
                        manager
                            .answer_authentication(id, Some(vec!["late".into(), "late".into()]))
                            .is_err()
                    );
                    attempt.abort();
                    assert!(attempt.await.unwrap_err().is_cancelled());
                } else {
                    manager.shutdown().await;
                    assert!(matches!(
                        attempt.await.unwrap(),
                        Err(mobarust_ssh::SshError::AuthenticationCancelled)
                    ));
                }
            }
            assert_eq!(events.recv().await.unwrap()["event"], "closed");
            assert!(manager.authentication.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn command_sender_refuses_close_before_worker_removal() {
        let manager = SshManager::default();
        let _commands = queue_test_session(&manager, "closing");
        assert!(manager.sender("closing").is_ok());
        manager
            .sessions
            .lock()
            .unwrap()
            .get("closing")
            .unwrap()
            .close
            .send_replace(true);
        assert!(matches!(
            manager.sender("closing"),
            Err(SshManagerError::Closed)
        ));
    }

    #[tokio::test]
    async fn full_command_queue_refuses_actions_without_waiting_or_registering_controls() {
        use super::{COMMAND_CAPACITY, SshCommand, SshRemoteForwardRequest};
        use mobarust_ssh::RemoteTextEncoding;
        use std::future::{Future, poll_fn};
        use std::task::Poll;

        async fn busy<T>(request: impl Future<Output = Result<T, SshManagerError>>) {
            let result = tokio::time::timeout(Duration::from_secs(1), request)
                .await
                .expect("full queues must refuse actions without waiting");
            assert!(
                matches!(result, Err(SshManagerError::InvalidRequest(ref reason))
                if reason == "SSH command queue is full; wait for queued actions to finish, then retry explicitly")
            );
        }

        let manager = SshManager::default();
        let mut commands = queue_test_session(&manager, "full");
        let size = manager.sessions.lock().unwrap()["full"].size.subscribe();
        for _ in 0..COMMAND_CAPACITY {
            manager.write("full", "accepted".into()).await.unwrap();
        }
        busy(manager.list_directory("full", "/".into())).await;
        busy(manager.open_remote_text_file("full", "/file".into(), RemoteTextEncoding::Utf8)).await;
        busy(manager.collect_remote_monitor("full")).await;
        busy(manager.save_remote_text_file(
            "full",
            "/file".into(),
            "revision".into(),
            "content".into(),
            RemoteTextEncoding::Utf8,
        ))
        .await;
        busy(manager.save_remote_text_file_as(
            "full",
            "/file".into(),
            "content".into(),
            RemoteTextEncoding::Utf8,
            false,
        ))
        .await;
        busy(manager.rename_remote("full", "/old".into(), "/new".into())).await;
        busy(manager.delete_remote("full", "/file".into())).await;
        busy(manager.create_remote_directory("full", "/directory".into())).await;
        busy(manager.set_remote_permissions("full", "/file".into(), 0o600)).await;
        busy(manager.start_remote_forward(
            "full".into(),
            SshRemoteForwardRequest {
                bind_host: "127.0.0.1".into(),
                bind_port: 0,
                target_host: "127.0.0.1".into(),
                target_port: 22,
            },
        ))
        .await;
        assert!(manager.transfers.lock().unwrap().is_empty());
        assert!(manager.tunnels.lock().unwrap().is_empty());
        assert!(manager.remote_forwards.lock().unwrap().is_empty());
        assert_eq!(commands.len(), COMMAND_CAPACITY);

        // Terminal input keeps backpressure rather than dropping keystrokes.
        let mut write = Box::pin(manager.write("full", "pending-input".into()));
        poll_fn(|cx| {
            assert!(write.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        manager.resize("full", 120, 40).await.unwrap();
        assert_eq!(*size.borrow(), (120, 40));
        assert!(
            matches!(commands.recv().await, Some(SshCommand::Write(bytes)) if bytes == b"accepted")
        );
        write.await.unwrap();
        for index in 0..COMMAND_CAPACITY {
            match commands.recv().await.unwrap() {
                SshCommand::Write(bytes) => assert_eq!(
                    bytes,
                    if index + 1 == COMMAND_CAPACITY {
                        b"pending-input".as_slice()
                    } else {
                        b"accepted".as_slice()
                    }
                ),
                _ => panic!("refused actions must never be queued for later execution"),
            }
        }
        let (result, ()) = tokio::join!(manager.list_directory("full", "/retry".into()), async {
            match commands.recv().await.unwrap() {
                SshCommand::ListDirectory { path, reply } => {
                    assert_eq!(path, "/retry");
                    reply.send(Ok(Vec::new())).unwrap();
                }
                _ => panic!("explicit retry should enqueue only the new action"),
            }
        });
        assert!(result.unwrap().is_empty());
        commands.close();
        assert!(matches!(
            manager.list_directory("full", "/".into()).await,
            Err(SshManagerError::Closed)
        ));
        // Reservations themselves count toward capacity, and dropping an
        // unused permit frees it without creating a command or control.
        let fresh = manager.reopen_command_queue("full").unwrap();
        let sender = manager.sender("full").unwrap();
        let mut permits = (0..COMMAND_CAPACITY)
            .map(|_| SshManager::reserve_command(&sender).unwrap())
            .collect::<Vec<_>>();
        busy(manager.list_directory("full", "/".into())).await;
        assert!(fresh.is_empty());
        drop(permits.pop());
        assert!(SshManager::reserve_command(&sender).is_ok());
        drop(permits);
        drop(fresh);
        assert!(matches!(
            SshManager::reserve_command(&sender),
            Err(SshManagerError::Closed)
        ));
    }

    #[tokio::test]
    async fn terminal_resize_survives_retired_command_queue() {
        let manager = SshManager::default();
        let mut commands = queue_test_session(&manager, "resizing");
        let mut size = manager.sessions.lock().unwrap()["resizing"]
            .size
            .subscribe();
        manager.resize("resizing", 100, 30).await.unwrap();
        assert_eq!(*size.borrow_and_update(), (100, 30));
        assert!(
            commands.try_recv().is_err(),
            "geometry does not fill the action queue"
        );
        commands.close();
        assert!(
            manager
                .write("resizing", "do-not-replay".into())
                .await
                .is_err()
        );
        manager
            .resize("resizing", 132, 41)
            .await
            .expect("current terminal size must survive reconnect backoff");
        assert_eq!(*size.borrow_and_update(), (132, 41));
        for cols in 1..=1000 {
            manager.resize("resizing", cols, 50).await.unwrap();
        }
        assert_eq!(*size.borrow_and_update(), (1000, 50));
        let mut fresh = manager.reopen_command_queue("resizing").unwrap();
        manager.resize("resizing", 0, 0).await.unwrap();
        assert_eq!(*size.borrow_and_update(), (1, 1));
        assert!(
            fresh.try_recv().is_err(),
            "no old terminal actions are replayed"
        );
        manager.sessions.lock().unwrap()["resizing"]
            .close
            .send_replace(true);
        assert!(manager.resize("resizing", 80, 24).await.is_err());
        assert_eq!(
            *size.borrow(),
            (1, 1),
            "Close refuses late geometry changes"
        );
        manager.sessions.lock().unwrap()["resizing"]
            .close
            .send_replace(false);
        manager.shutdown.send_replace(true);
        assert!(manager.resize("resizing", 80, 24).await.is_err());
        manager.shutdown.send_replace(false);
        drop(size);
        assert!(
            manager.resize("resizing", 80, 24).await.is_err(),
            "a terminated worker cannot accept resize"
        );
        assert!(manager.resize("missing", 80, 24).await.is_err());
    }

    #[tokio::test]
    async fn retired_queue_settles_old_permits_and_does_not_replay_input() {
        use super::{QUEUED_COMMAND_CANCELLED, SshCommand, SshFileOperation};
        use std::future::{Future, poll_fn};
        use std::task::Poll;

        let manager = SshManager::default();
        let mut commands = queue_test_session(&manager, "same-terminal");
        let old = manager.sender("same-terminal").unwrap();
        assert!(manager.reopen_command_queue("same-terminal").is_err());
        assert!(
            old.try_send(SshCommand::Write(b"do-not-replay\n".to_vec()))
                .is_ok()
        );
        let permit = old.clone().reserve_owned().await.unwrap();
        let (reply, response) = oneshot::channel();
        tokio::time::timeout(Duration::from_secs(2), async {
            let mut retire = Box::pin(manager.retire_command_queue(
                "same-terminal",
                &mut commands,
                |_| panic!("no transfer was queued"),
                |_| panic!("no tunnel was queued"),
            ));
            poll_fn(|cx| {
                assert!(retire.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            assert!(old.is_closed());
            assert!(manager.sender("same-terminal").is_err());
            // A permit already obtained before loss can still enqueue. The drain
            // must settle it, not stop when try_recv temporarily sees an empty queue.
            permit.send(SshCommand::FileOperation {
                operation: SshFileOperation::Delete {
                    path: "do-not-delete".into(),
                },
                reply,
            });
            assert_eq!(retire.await, 2);
            assert_eq!(
                response.await.unwrap().unwrap_err(),
                QUEUED_COMMAND_CANCELLED
            );
            let mut fresh = manager.reopen_command_queue("same-terminal").unwrap();
            assert!(
                manager
                    .ensure_current_sender("same-terminal", &old)
                    .is_err()
            );
            assert!(
                old.send(SshCommand::Write(b"late-old-input".to_vec()))
                    .await
                    .is_err()
            );
            assert!(fresh.try_recv().is_err());
            manager
                .write("same-terminal", "fresh-input".into())
                .await
                .unwrap();
            match fresh.recv().await.unwrap() {
                SshCommand::Write(bytes) => assert_eq!(bytes, b"fresh-input"),
                _ => panic!("replacement queue received the wrong action"),
            }
            manager
                .sessions
                .lock()
                .unwrap()
                .get("same-terminal")
                .unwrap()
                .close
                .send_replace(true);
            assert!(manager.reopen_command_queue("same-terminal").is_err());
        })
        .await
        .expect("queue retirement deadline");
    }

    #[tokio::test]
    async fn saturated_session_rejects_work_settles_replies_and_keeps_input_available() {
        use super::{
            SESSION_OPERATION_BUSY, SESSION_OPERATION_LIMIT, SshCommand, SshFileOperation,
            TransferControl, TransferDirection, TransferJob,
        };
        use mobarust_core::TransferState;
        use mobarust_ssh::RemoteTextEncoding;

        async fn busy<T>(response: oneshot::Receiver<Result<T, String>>) {
            let result = tokio::time::timeout(Duration::from_secs(1), response)
                .await
                .expect("busy action must settle immediately")
                .expect("busy action must retain its explicit error");
            assert!(matches!(result, Err(error) if error == SESSION_OPERATION_BUSY));
        }

        let manager = SshManager::default();
        let _commands = queue_test_session(&manager, "saturated");
        let mut size = manager.sessions.lock().unwrap()["saturated"]
            .size
            .subscribe();
        let mut workers = tokio::task::JoinSet::new();
        for _ in 0..SESSION_OPERATION_LIMIT {
            workers.spawn(std::future::pending::<()>());
        }
        let refuse = |command, workers: &mut tokio::task::JoinSet<()>| {
            assert!(
                manager
                    .admit_session_command(
                        command,
                        workers,
                        |_| panic!("file action must not emit a transfer event"),
                        |_| panic!("file action must not emit a tunnel event"),
                    )
                    .is_none()
            );
            assert_eq!(workers.len(), SESSION_OPERATION_LIMIT);
        };
        let (reply, response) = oneshot::channel();
        refuse(
            SshCommand::ListDirectory {
                path: "/".into(),
                reply,
            },
            &mut workers,
        );
        busy(response).await;
        let (reply, response) = oneshot::channel();
        refuse(
            SshCommand::OpenTextFile {
                path: "/fixture".into(),
                encoding: RemoteTextEncoding::Utf8,
                reply,
            },
            &mut workers,
        );
        busy(response).await;
        let (reply, response) = oneshot::channel();
        refuse(
            SshCommand::SaveTextFile {
                path: "/fixture".into(),
                expected_revision: "revision".into(),
                content: "never-written".into(),
                encoding: RemoteTextEncoding::Utf8,
                reply,
            },
            &mut workers,
        );
        busy(response).await;
        let (reply, response) = oneshot::channel();
        refuse(
            SshCommand::SaveTextFileAs {
                path: "/fixture".into(),
                content: "never-written".into(),
                encoding: RemoteTextEncoding::Utf8,
                overwrite: true,
                reply,
            },
            &mut workers,
        );
        busy(response).await;
        let (reply, response) = oneshot::channel();
        refuse(
            SshCommand::FileOperation {
                operation: SshFileOperation::Delete {
                    path: "/fixture".into(),
                },
                reply,
            },
            &mut workers,
        );
        busy(response).await;
        let (reply, response) = oneshot::channel();
        refuse(SshCommand::CollectMonitor { reply }, &mut workers);
        busy(response).await;

        let (cancel, cancellation) = oneshot::channel();
        manager.transfers.lock().unwrap().insert(
            "excess-transfer".into(),
            TransferControl {
                terminal_id: "saturated".into(),
                cancel,
            },
        );
        let mut events = Vec::new();
        assert!(
            manager
                .admit_session_command(
                    SshCommand::StartTransfer {
                        job: TransferJob {
                            transfer_id: "excess-transfer".into(),
                            terminal_id: "saturated".into(),
                            direction: TransferDirection::Upload,
                            protocol: TransferProtocol::Sftp,
                            remote_path: "never-uploaded".into(),
                            local_path: "never-opened".into(),
                            overwrite: false,
                            recursive: false,
                            source: "never-opened".into(),
                            destination: "never-uploaded".into(),
                            created_at: Instant::now(),
                        },
                        cancel: cancellation,
                    },
                    &mut workers,
                    |event| events.push(event),
                    |_| panic!("no tunnel event")
                )
                .is_none()
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].state, TransferState::Failed);
        assert_eq!(events[0].bytes_transferred, 0);
        assert_eq!(events[0].error.as_deref(), Some(SESSION_OPERATION_BUSY));
        assert!(manager.transfers.lock().unwrap().is_empty());
        assert_eq!(workers.len(), SESSION_OPERATION_LIMIT);

        let input = b"explicit-terminal-input\n".to_vec();
        assert!(
            matches!(manager.admit_session_command(SshCommand::Write(input.clone()),
            &mut workers, |_| panic!("no transfer"), |_| panic!("no tunnel")),
            Some(SshCommand::Write(bytes)) if bytes == input)
        );
        manager.resize("saturated", 140, 45).await.unwrap();
        assert_eq!(*size.borrow_and_update(), (140, 45));

        // Even a completed, unjoined entry must free capacity before admission.
        workers.abort_all();
        while workers.join_next().await.is_some() {}
        for _ in 1..SESSION_OPERATION_LIMIT {
            workers.spawn(std::future::pending::<()>());
        }
        let finished = workers.spawn(async {});
        tokio::time::timeout(Duration::from_secs(1), async {
            while !finished.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(workers.len(), SESSION_OPERATION_LIMIT);
        let (reply, response) = oneshot::channel();
        let command = manager.admit_session_command(
            SshCommand::ListDirectory {
                path: "/retry".into(),
                reply,
            },
            &mut workers,
            |_| panic!("no transfer"),
            |_| panic!("no tunnel"),
        );
        assert_eq!(workers.len(), SESSION_OPERATION_LIMIT - 1);
        let Some(SshCommand::ListDirectory { path, reply }) = command else {
            panic!("a completed worker must allow an explicit retry");
        };
        assert_eq!(path, "/retry");
        reply.send(Ok(Vec::new())).unwrap();
        assert!(response.await.unwrap().unwrap().is_empty());
        workers.abort_all();
        while workers.join_next().await.is_some() {}
    }

    #[tokio::test]
    async fn retired_queue_reports_cancelled_jobs_and_releases_loopback_listeners() {
        assert_queued_jobs_cleanup(false).await;
    }

    #[tokio::test]
    async fn saturated_session_refuses_tunnels_and_releases_loopback_listeners() {
        assert_queued_jobs_cleanup(true).await;
    }

    async fn assert_queued_jobs_cleanup(saturated: bool) {
        use super::{
            DynamicForwardJob, LocalForwardJob, QUEUED_COMMAND_CANCELLED, RemoteForwardJob,
            SshCommand, TransferControl, TransferDirection, TransferJob, TunnelControl,
            TunnelState,
        };
        use mobarust_core::TransferState;
        use tokio::net::TcpListener;

        let manager = SshManager::default();
        let mut commands = queue_test_session(&manager, "closing");
        let sender = manager.sender("closing").unwrap();
        let (cancel, cancellation) = oneshot::channel();
        manager.transfers.lock().unwrap().insert(
            "queued-file".into(),
            TransferControl {
                terminal_id: "closing".into(),
                cancel,
            },
        );
        let job = TransferJob {
            transfer_id: "queued-file".into(),
            terminal_id: "closing".into(),
            direction: TransferDirection::Upload,
            protocol: TransferProtocol::Sftp,
            remote_path: "never-uploaded".into(),
            local_path: "never-opened".into(),
            overwrite: false,
            recursive: false,
            source: "never-opened".into(),
            destination: "never-uploaded".into(),
            created_at: Instant::now(),
        };
        assert!(
            sender
                .try_send(SshCommand::StartTransfer {
                    job,
                    cancel: cancellation
                })
                .is_ok()
        );
        let mut ports = Vec::new();
        for (id, dynamic) in [("queued-local", false), ("queued-socks", true)] {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let port = listener.local_addr().unwrap().port();
            ports.push(port);
            let (cancel, cancellation) = watch::channel(false);
            manager.tunnels.lock().unwrap().insert(
                id.into(),
                TunnelControl {
                    terminal_id: "closing".into(),
                    cancel,
                },
            );
            let command = if dynamic {
                SshCommand::StartDynamicForward {
                    job: DynamicForwardJob {
                        tunnel_id: id.into(),
                        terminal_id: "closing".into(),
                        bind_host: "127.0.0.1".into(),
                        bind_port: port,
                        listener,
                        cancel: cancellation,
                    },
                }
            } else {
                SshCommand::StartLocalForward {
                    job: LocalForwardJob {
                        tunnel_id: id.into(),
                        terminal_id: "closing".into(),
                        bind_host: "127.0.0.1".into(),
                        bind_port: port,
                        target_host: "127.0.0.1".into(),
                        target_port: 1,
                        listener,
                        cancel: cancellation,
                    },
                }
            };
            assert!(sender.try_send(command).is_ok());
        }
        let (cancel, cancellation) = watch::channel(false);
        manager.tunnels.lock().unwrap().insert(
            "queued-remote".into(),
            TunnelControl {
                terminal_id: "closing".into(),
                cancel,
            },
        );
        manager
            .remote_forwards
            .lock()
            .unwrap()
            .insert("closing".into(), "queued-remote".into());
        let (reply, remote_response) = oneshot::channel();
        assert!(
            sender
                .try_send(SshCommand::StartRemoteForward {
                    job: RemoteForwardJob {
                        tunnel_id: "queued-remote".into(),
                        terminal_id: "closing".into(),
                        bind_host: "127.0.0.1".into(),
                        bind_port: 0,
                        target_host: "127.0.0.1".into(),
                        target_port: 1,
                        cancel: cancellation,
                    },
                    reply,
                })
                .is_ok()
        );
        let (reply, editor_response) = oneshot::channel();
        assert!(
            sender
                .try_send(SshCommand::SaveTextFileAs {
                    path: "never-saved".into(),
                    content: "never-sent-content".into(),
                    encoding: mobarust_ssh::RemoteTextEncoding::Utf8,
                    overwrite: true,
                    reply,
                })
                .is_ok()
        );
        let mut transfers = Vec::new();
        let mut tunnels = Vec::new();
        let count = if saturated {
            let mut workers = tokio::task::JoinSet::new();
            for _ in 0..super::SESSION_OPERATION_LIMIT {
                workers.spawn(std::future::pending::<()>());
            }
            let mut count = 0;
            while let Ok(command) = commands.try_recv() {
                assert!(
                    manager
                        .admit_session_command(
                            command,
                            &mut workers,
                            |event| transfers.push(event),
                            |event| tunnels.push(event)
                        )
                        .is_none(),
                    "tunnels must share session worker admission"
                );
                assert_eq!(workers.len(), super::SESSION_OPERATION_LIMIT);
                count += 1;
            }
            workers.abort_all();
            while workers.join_next().await.is_some() {}
            count
        } else {
            tokio::time::timeout(
                Duration::from_secs(2),
                manager.retire_command_queue(
                    "closing",
                    &mut commands,
                    |event| transfers.push(event),
                    |event| tunnels.push(event),
                ),
            )
            .await
            .expect("queued jobs cleanup deadline")
        };
        let reason = if saturated {
            super::SESSION_OPERATION_BUSY
        } else {
            QUEUED_COMMAND_CANCELLED
        };
        assert_eq!(count, 5);
        assert_eq!(transfers.len(), 1);
        assert_eq!(transfers[0].transfer_id, "queued-file");
        assert_eq!(
            transfers[0].state,
            if saturated {
                TransferState::Failed
            } else {
                TransferState::Cancelled
            }
        );
        assert_eq!(transfers[0].bytes_transferred, 0);
        assert_eq!(transfers[0].error.as_deref(), Some(reason));
        assert_eq!(tunnels.len(), 3);
        assert!(tunnels.iter().all(|event| (if saturated {
            matches!(event.state, TunnelState::Failed)
        } else {
            matches!(event.state, TunnelState::Stopped)
        }) && event.error.as_deref() == Some(reason)
            && event.bytes_forwarded == 0
            && event.connections == 0));
        assert_eq!(remote_response.await.unwrap().unwrap_err(), reason);
        assert_eq!(editor_response.await.unwrap().unwrap_err(), reason);
        assert!(manager.transfers.lock().unwrap().is_empty());
        assert!(manager.tunnels.lock().unwrap().is_empty());
        assert!(manager.remote_forwards.lock().unwrap().is_empty());
        for port in ports {
            let listener = TcpListener::bind(("127.0.0.1", port))
                .await
                .expect("discarded queued listener was released");
            drop(listener);
        }
    }

    #[tokio::test]
    async fn app_shutdown_signals_all_sessions_and_waits_for_each_cleanup() {
        use super::{SessionState, TransferControl};
        use mobarust_core::Utf8OutputDecoder;
        use std::future::{Future, poll_fn};
        use std::task::Poll;
        use tokio::sync::mpsc;
        use tokio::task::JoinSet;

        let manager = SshManager::default();
        let mut workers = JoinSet::new();
        let mut observed = Vec::new();
        let mut release = Vec::new();
        for id in ["first", "second"] {
            let (sender, _commands) = mpsc::channel(1);
            let (close, mut closing) = watch::channel(false);
            let (finished, finished_receiver) = watch::channel(false);
            let (cancel, mut cancelled) = oneshot::channel();
            manager.transfers.lock().unwrap().insert(
                format!("{id}-transfer"),
                TransferControl {
                    terminal_id: id.into(),
                    cancel,
                },
            );
            manager.sessions.lock().unwrap().insert(
                id.into(),
                SessionState {
                    sender,
                    size: watch::channel((80, 24)).0,
                    close,
                    finished: finished_receiver,
                    attached: false,
                    pending_output: Vec::new(),
                    output_decoder: Utf8OutputDecoder::default(),
                },
            );
            let (close_observed, wait_for_close) = oneshot::channel();
            let (allow_finish, wait_for_finish) = oneshot::channel();
            observed.push(wait_for_close);
            release.push(allow_finish);
            workers.spawn(async move {
                closing.changed().await.unwrap();
                assert!(*closing.borrow());
                assert!(cancelled.try_recv().is_ok());
                close_observed.send(()).unwrap();
                wait_for_finish.await.unwrap();
                finished.send_replace(true);
            });
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            let mut shutdown = Box::pin(manager.shutdown());
            tokio::select! {
                _ = &mut shutdown => panic!("Quit skipped active session cleanup"),
                _ = async { for receiver in observed { receiver.await.unwrap(); } } => {}
            }
            assert!(*manager.shutdown.borrow());
            assert!(matches!(
                manager.sender("first"),
                Err(SshManagerError::Closed)
            ));
            release.remove(0).send(()).unwrap();
            poll_fn(|cx| {
                assert!(shutdown.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            release.remove(0).send(()).unwrap();
            shutdown.await;
            while let Some(result) = workers.join_next().await {
                result.unwrap();
            }
            // Completed sessions and an already-started shutdown are idempotent.
            manager.shutdown().await;
        })
        .await
        .expect("all-session cleanup deadline");
    }

    #[tokio::test]
    async fn empty_app_shutdown_still_rejects_new_ssh_work() {
        let manager = SshManager::default();
        manager.shutdown().await;
        manager.shutdown().await;
        assert!(*manager.shutdown.borrow());
        assert!(matches!(
            manager.sender("new-session"),
            Err(SshManagerError::Closed)
        ));
    }

    #[tokio::test]
    async fn session_transfer_drain_cancels_only_its_jobs_and_waits_for_cleanup() {
        use super::TransferControl;
        use std::future::{Future, poll_fn};
        use std::task::Poll;
        use tokio::task::JoinSet;

        let manager = SshManager::default();
        let directory = tempdir().unwrap();
        let original = directory.path().join("original");
        let part = directory.path().join("part");
        fs::write(&original, b"original bytes").unwrap();
        fs::write(&part, b"unfinished upload").unwrap();
        let (cancel, cancellation) = oneshot::channel();
        let (other_cancel, mut other_cancellation) = oneshot::channel();
        for (id, terminal_id, cancel) in [
            ("closing-job", "closing-session", cancel),
            ("other-job", "other-session", other_cancel),
        ] {
            manager.transfers.lock().unwrap().insert(
                id.into(),
                TransferControl {
                    terminal_id: terminal_id.into(),
                    cancel,
                },
            );
        }
        let (cleanup_started, cleanup_observed) = oneshot::channel();
        let (finish_cleanup, cleanup_finished) = oneshot::channel();
        let part_for_worker = part.clone();
        let mut workers = JoinSet::new();
        workers.spawn(async move {
            cancellation.await.unwrap();
            fs::remove_file(part_for_worker).unwrap();
            cleanup_started.send(()).unwrap();
            cleanup_finished.await.unwrap();
        });
        let mut drain =
            Box::pin(manager.finish_session_operations("closing-session", &mut workers));
        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::select! {
                _ = &mut drain => panic!("session closed before worker cleanup finished"),
                _ = cleanup_observed => {}
            }
            assert!(!part.exists());
            assert_eq!(fs::read(&original).unwrap(), b"original bytes");
            assert!(matches!(
                other_cancellation.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ));
            assert!(manager.transfers.lock().unwrap().contains_key("other-job"));
            poll_fn(|cx| {
                assert!(drain.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            finish_cleanup.send(()).unwrap();
            drain.await;
        })
        .await
        .expect("cooperative transfer drain deadline");
        assert!(workers.is_empty());
    }

    #[tokio::test]
    async fn recursive_empty_upload_scan_honors_cancel_and_closed_control() {
        let directory = tempdir().unwrap();
        for explicit in [false, true] {
            let (sender, mut cancel) = oneshot::channel();
            if explicit {
                sender.send(()).unwrap();
            } else {
                drop(sender);
            }
            assert!(matches!(
                collect_local_files(directory.path(), "/fixture", &mut cancel).await,
                Err(mobarust_ssh::SshError::Cancelled)
            ));
        }
    }

    #[tokio::test]
    async fn cancelled_local_commit_preserves_destination_and_cleans_complete_part() {
        for overwrite in [false, true] {
            for send_cancel in [false, true] {
                let directory = tempdir().unwrap();
                let destination = directory.path().join("download.txt");
                let temporary = directory.path().join(".download.txt.mobarust.part");
                if overwrite {
                    fs::write(&destination, b"original").unwrap();
                }
                fs::write(&temporary, b"complete replacement").unwrap();
                let (sender, mut cancel) = oneshot::channel();
                if send_cancel {
                    sender.send(()).unwrap();
                } else {
                    drop(sender);
                }
                assert!(matches!(
                    commit_local_file(&temporary, &destination, overwrite, &mut cancel),
                    Err(mobarust_ssh::SshError::Cancelled)
                ));
                if overwrite {
                    assert_eq!(fs::read(&destination).unwrap(), b"original");
                } else {
                    assert!(!destination.exists());
                }
                remove_partial_download_path(&temporary).await.unwrap();
                assert!(!temporary.exists());
            }
        }
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
    async fn reconnect_policy_stops_after_uncertain_startup_delivery() {
        use mobarust_ssh::SshError;
        for kind in ["timeout", "closed", "overflow", "transport"] {
            for failure_at in [1, 2] {
                let (_close_sender, mut close) = watch::channel(false);
                let mut attempted = Vec::new();
                let result = reconnect_with_backoff(
                    &mut close,
                    "shell channel closed".to_owned(),
                    3,
                    |attempt, _error| attempted.push(attempt),
                    |attempt| async move {
                        let error = if attempt < failure_at {
                            SshError::ConnectionRefused
                        } else {
                            match kind {
                                "timeout" => SshError::StartupInputTimeout,
                                "closed" => SshError::StartupInputFailed(Box::new(
                                    SshError::ChannelRequestClosed {
                                        request: "startup input",
                                    },
                                )),
                                "overflow" => SshError::StartupInputFailed(Box::new(
                                    SshError::ShellSetupOutputTooLarge,
                                )),
                                _ => SshError::StartupInputFailed(Box::new(SshError::Channel(
                                    russh::Error::Disconnect,
                                ))),
                            }
                        };
                        Err::<(), SshManagerError>(error.into())
                    },
                    |_| Duration::ZERO,
                )
                .await;
                assert_eq!(
                    attempted,
                    (1..=failure_at).collect::<Vec<_>>(),
                    "uncertain startup input must not be replayed"
                );
                assert!(
                    matches!(result, ReconnectOutcome::Failed { attempts, last_error }
                    if attempts == failure_at && last_error.contains("Check the remote session and startup settings before reconnecting."))
                );
            }
        }
    }

    #[tokio::test]
    async fn reconnect_policy_reports_bounded_failure_and_last_error() {
        let (_close_sender, mut close) = watch::channel(false);
        let mut attempts = Vec::new();
        let result = reconnect_with_backoff(
            &mut close,
            "shell channel closed".to_owned(),
            3,
            |attempt, error| attempts.push((attempt, error.to_owned())),
            |attempt| async move {
                Err::<(), SshManagerError>(SshManagerError::InvalidRequest(format!(
                    "fixture failure {attempt}"
                )))
            },
            |_| Duration::ZERO,
        )
        .await;

        assert_eq!(
            attempts,
            vec![
                (1, "shell channel closed".to_owned()),
                (2, "invalid SSH request: fixture failure 1".to_owned()),
                (3, "invalid SSH request: fixture failure 2".to_owned()),
            ]
        );
        assert!(matches!(
            result,
            ReconnectOutcome::Failed {
                attempts: 3,
                last_error
            } if last_error == "invalid SSH request: fixture failure 3"
        ));
    }

    #[test]
    fn repeated_short_lived_shells_exhaust_reconnect_budget() {
        let mut used = 0;
        for _ in 0..3 {
            used = next_shell_reconnect_count(used, Duration::from_secs(1), 3).unwrap();
        }
        assert_eq!(
            next_shell_reconnect_count(used, Duration::from_secs(1), 3),
            None
        );
        assert_eq!(
            next_shell_reconnect_count(used, Duration::from_secs(30), 3),
            Some(1)
        );
        assert_eq!(next_shell_reconnect_count(0, Duration::ZERO, 0), None);
        assert_eq!(next_shell_reconnect_count(3, Duration::ZERO, 10), Some(4));
    }

    #[test]
    fn ssh_connection_policy_defaults_and_rejects_out_of_range_values() {
        let mut request: SshConnectRequest = serde_json::from_value(serde_json::json!({
            "host": "127.0.0.1", "port": 22, "username": "fixture",
            "auth": { "method": "agent" }
        }))
        .unwrap();
        assert_eq!(request.reconnect_attempts, 3);
        assert_eq!(request.connect_timeout_ms, 12_000);
        assert!(validate_ssh_connection_policy(&request).is_ok());

        let configured: SshConnectRequest = serde_json::from_value(serde_json::json!({
            "host": "127.0.0.1", "port": 22, "username": "fixture",
            "auth": { "method": "agent" },
            "reconnectAttempts": 0, "connectTimeoutMs": 250
        }))
        .unwrap();
        assert_eq!(configured.reconnect_attempts, 0);
        assert_eq!(configured.connect_timeout_ms, 250);
        assert!(validate_ssh_connection_policy(&configured).is_ok());

        request.reconnect_attempts = 10;
        request.connect_timeout_ms = 100;
        assert!(validate_ssh_connection_policy(&request).is_ok());
        request.reconnect_attempts = 11;
        assert!(validate_ssh_connection_policy(&request).is_err());
        request.reconnect_attempts = 0;
        request.connect_timeout_ms = 60_001;
        assert!(validate_ssh_connection_policy(&request).is_err());
        request.connect_timeout_ms = 100;
        request.jump_hosts = (0..=MAX_SSH_JUMP_HOSTS)
            .map(|_| {
                serde_json::from_value(serde_json::json!({
                    "host": "127.0.0.1", "port": 22, "username": "fixture",
                    "auth": { "method": "agent" }
                }))
                .unwrap()
            })
            .collect();
        assert!(validate_ssh_connection_policy(&request).is_err());
        request.jump_hosts.pop();
        assert!(validate_ssh_connection_policy(&request).is_ok());
    }

    #[tokio::test]
    async fn reconnect_policy_returns_on_first_success_after_a_failure() {
        let (_close_sender, mut close) = watch::channel(false);
        let result = reconnect_with_backoff(
            &mut close,
            "shell channel closed".to_owned(),
            3,
            |_attempt, _error| {},
            |attempt| async move {
                if attempt == 2 {
                    Ok::<_, SshManagerError>("fixture reconnected")
                } else {
                    Err(SshManagerError::InvalidRequest(format!(
                        "fixture failure {attempt}"
                    )))
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
                3,
                move |_attempt, _error| {
                    if let Some(sender) = started_sender.take() {
                        let _ = sender.send(());
                    }
                },
                |_attempt| async {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    Ok::<_, SshManagerError>(())
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
