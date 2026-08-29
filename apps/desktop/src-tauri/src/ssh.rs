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
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
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
    #[serde(default = "default_terminal_cols")]
    pub cols: u32,
    #[serde(default = "default_terminal_rows")]
    pub rows: u32,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "method", rename_all = "camelCase")]
pub enum SshAuthRequest {
    Agent,
    Password {
        credential_id: String,
    },
    PrivateKey {
        path: String,
        #[serde(default)]
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
    StartTransfer {
        job: TransferJob,
    },
    Close,
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
    transfer_slots: Arc<Semaphore>,
}

impl Default for SshManager {
    fn default() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            transfers: Arc::new(Mutex::new(HashMap::new())),
            transfer_slots: Arc::new(Semaphore::new(3)),
        }
    }
}

struct SessionState {
    sender: mpsc::Sender<SshCommand>,
    attached: bool,
    pending_output: Vec<String>,
}

struct TransferControl {
    terminal_id: String,
    cancel: oneshot::Sender<()>,
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
        let credentials = credentials_from_request(vault, &request)?;
        let host_key_policy = host_key_policy(&request)?;
        let host = request.host.clone();
        let connection = SshConnection::connect(SshConnectOptions {
            host: request.host,
            port: request.port,
            host_key_policy,
            timeout: Duration::from_secs(12),
            credentials,
        })
        .await?;
        let connection = Arc::new(connection);
        let shell = connection.open_shell(request.cols, request.rows).await?;
        let (mut reader, writer) = shell.split();
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
        tauri::async_runtime::spawn(async move {
            run_remote_session(
                app,
                manager,
                id_for_task,
                connection,
                &mut reader,
                writer,
                receiver,
            )
            .await;
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
        if path.trim().is_empty() {
            return Err(SshManagerError::InvalidRequest(
                "remote directory path cannot be empty".into(),
            ));
        }
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
    }

    fn finish_transfer(&self, transfer_id: &str) {
        if let Ok(mut transfers) = self.transfers.lock() {
            transfers.remove(transfer_id);
        }
    }

    fn emit_transfer(&self, app: &AppHandle, event: SshTransferEvent) {
        let _ = app.emit("sftp://transfer", event);
    }
}

fn credentials_from_request(
    vault: &PlatformVault,
    request: &SshConnectRequest,
) -> Result<SshCredentials, SshManagerError> {
    match &request.auth {
        SshAuthRequest::Agent => Ok(SshCredentials::agent(&request.username)),
        SshAuthRequest::Password { credential_id } => {
            let id = CredentialId::new(credential_id.clone())?;
            let secret = vault.get(&id)?;
            Ok(SshCredentials::password(&request.username, secret.as_str()))
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
                &request.username,
                expand_user_path(path),
                passphrase,
            ))
        }
    }
}

fn host_key_policy(request: &SshConnectRequest) -> Result<HostKeyPolicy, SshManagerError> {
    match (&request.known_hosts_path, &request.pinned_fingerprint) {
        (Some(_), Some(_)) => Err(SshManagerError::InvalidRequest(
            "choose known_hosts or a pinned fingerprint, not both".into(),
        )),
        (Some(path), None) => Ok(HostKeyPolicy::KnownHosts(expand_user_path(path))),
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
    app: AppHandle,
    manager: SshManager,
    terminal_id: String,
    connection: Arc<SshConnection>,
    reader: &mut mobarust_ssh::SshShellReader,
    writer: mobarust_ssh::SshShellWriter,
    mut commands: mpsc::Receiver<SshCommand>,
) {
    let mut should_report_error = None;
    let writer = writer;

    'session: loop {
        tokio::select! {
            output = reader.next_output() => {
                match output {
                    None => break 'session,
                    Some(Ok(SshOutput::Stdout(bytes) | SshOutput::Stderr(bytes))) => {
                        manager.emit_output(&app, &terminal_id, &bytes);
                    }
                    Some(Ok(SshOutput::ExitStatus(_))) | Some(Ok(SshOutput::Control)) => {}
                    Some(Err(error)) => {
                        should_report_error = Some(error.to_string());
                        break 'session;
                    }
                }
            }
            command = commands.recv() => {
                match command {
                    Some(SshCommand::Write(data)) => {
                        if let Err(error) = writer.write(&data).await {
                            should_report_error = Some(error.to_string());
                            break 'session;
                        }
                    }
                    Some(SshCommand::Resize { cols, rows }) => {
                        if let Err(error) = writer.resize(cols, rows).await {
                            should_report_error = Some(error.to_string());
                            break 'session;
                        }
                    }
                    Some(SshCommand::ListDirectory { path, reply }) => {
                        let result = async {
                            let sftp = connection
                                .open_sftp()
                                .await
                                .map_err(|error| error.to_string())?;
                            let entries = sftp
                                .read_dir(path)
                                .await
                                .map_err(|error| error.to_string())?;
                            let _ = sftp.close().await;
                            Ok(entries)
                        }
                        .await;
                        let _ = reply.send(result);
                    }
                    Some(SshCommand::StartTransfer { job }) => {
                        let transfer_manager = manager.clone();
                        let transfer_connection = Arc::clone(&connection);
                        let transfer_app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            run_transfer(transfer_app, transfer_manager, transfer_connection, job)
                                .await;
                        });
                    }
                    Some(SshCommand::Close) | None => {
                        let _ = writer.close().await;
                        break 'session;
                    }
                }
            }
        }
    }

    let _ = connection.disconnect().await;
    manager.remove(&terminal_id);
    let _ = app.emit(
        "ssh://closed",
        SshClosedEvent {
            terminal_id,
            reason: should_report_error.unwrap_or_else(|| "closed".into()),
        },
    );
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

fn default_terminal_rows() -> u32 {
    32
}
