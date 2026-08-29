use mobarust_ssh::{
    HostKeyPolicy, SshConnectOptions, SshConnection, SshCredentials, SshError, SshOutput,
};
use mobarust_vault::{CredentialId, PlatformVault, VaultError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use uuid::Uuid;

const COMMAND_CAPACITY: usize = 64;
const OUTPUT_BUFFER_BYTES: usize = 32 * 1024;

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

#[derive(Clone, Default)]
pub struct SshManager {
    sessions: Arc<Mutex<HashMap<String, mpsc::Sender<SshCommand>>>>,
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
        let shell = connection.open_shell(request.cols, request.rows).await?;
        let (mut reader, writer) = shell.split();
        let terminal_id = Uuid::new_v4().to_string();
        let (sender, receiver) = mpsc::channel(COMMAND_CAPACITY);
        self.sessions
            .lock()
            .map_err(|_| SshManagerError::Closed)?
            .insert(terminal_id.clone(), sender);

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

    fn sender(&self, terminal_id: &str) -> Result<mpsc::Sender<SshCommand>, SshManagerError> {
        self.sessions
            .lock()
            .map_err(|_| SshManagerError::Closed)?
            .get(terminal_id)
            .cloned()
            .ok_or_else(|| SshManagerError::MissingSession(terminal_id.to_owned()))
    }

    fn remove(&self, terminal_id: &str) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.remove(terminal_id);
        }
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
    connection: SshConnection,
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
                        emit_output(&app, &terminal_id, &bytes);
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

fn emit_output(app: &AppHandle, terminal_id: &str, bytes: &[u8]) {
    for chunk in bytes.chunks(OUTPUT_BUFFER_BYTES) {
        let _ = app.emit(
            "ssh://output",
            SshOutputEvent {
                terminal_id: terminal_id.to_owned(),
                data: String::from_utf8_lossy(chunk).into_owned(),
            },
        );
    }
}

fn default_terminal_cols() -> u32 {
    120
}

fn default_terminal_rows() -> u32 {
    32
}
