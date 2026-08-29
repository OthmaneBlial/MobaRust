//! Rust-owned SSH transport. The crate deliberately keeps credential material
//! out of serde models and out of the frontend-facing data types.

use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mobarust_core::{ConnectionEvent, ConnectionLifecycle, ConnectionState};
use russh::client;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{Channel, ChannelMsg, ChannelReadHalf, ChannelWriteHalf, Disconnect};
use serde::Serialize;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use zeroize::Zeroize;

#[derive(Debug, Clone)]
pub enum HostKeyPolicy {
    /// Match against the OpenSSH known_hosts format. Unknown keys are rejected.
    KnownHosts(PathBuf),
    /// Match one operator-confirmed SHA-256 fingerprint.
    PinnedFingerprint(String),
}

impl Default for HostKeyPolicy {
    fn default() -> Self {
        let path = std::env::home_dir()
            .map(|home| home.join(".ssh").join("known_hosts"))
            .unwrap_or_else(|| PathBuf::from(".ssh/known_hosts"));
        Self::KnownHosts(path)
    }
}

#[derive(Debug, Error)]
pub enum SshError {
    #[error("SSH host and username are required")]
    InvalidOptions,
    #[error("SSH host key rejected; observed fingerprint: {fingerprint}")]
    HostKeyRejected { fingerprint: String },
    #[error("SSH authentication was rejected")]
    AuthenticationRejected,
    #[error("SSH agent authentication failed: {0}")]
    Agent(String),
    #[error("SSH connection timed out")]
    Timeout,
    #[error("SSH private key could not be loaded: {0}")]
    PrivateKey(#[source] russh::keys::Error),
    #[error("SSH known_hosts check failed: {0}")]
    KnownHosts(#[source] russh::keys::Error),
    #[error("SSH transport failed: {0}")]
    Transport(#[source] russh::Error),
    #[error("SSH handshake failed: {0}")]
    Handshake(String),
    #[error("SSH channel failed: {0}")]
    Channel(#[source] russh::Error),
    #[error("SFTP operation failed: {0}")]
    Sftp(String),
    #[error("SSH credential material is unavailable")]
    MissingCredentials,
}

/// A password is intentionally not serializable or cloneable. The value is
/// only borrowed across the native authentication call and is zeroized on drop.
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted>")
    }
}

pub enum SshCredentials {
    Password {
        username: String,
        password: Secret,
    },
    PrivateKey {
        username: String,
        path: PathBuf,
        passphrase: Option<Secret>,
    },
    Agent {
        username: String,
    },
}

impl fmt::Debug for SshCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SshCredentials")
            .field("username", &self.username())
            .field("method", &self.method_name())
            .finish()
    }
}

impl SshCredentials {
    pub fn password(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self::Password {
            username: username.into(),
            password: Secret::new(password),
        }
    }

    pub fn private_key(
        username: impl Into<String>,
        path: impl Into<PathBuf>,
        passphrase: Option<impl Into<String>>,
    ) -> Self {
        Self::PrivateKey {
            username: username.into(),
            path: path.into(),
            passphrase: passphrase.map(|value| Secret::new(value)),
        }
    }

    pub fn agent(username: impl Into<String>) -> Self {
        Self::Agent {
            username: username.into(),
        }
    }

    fn username(&self) -> &str {
        match self {
            Self::Password { username, .. }
            | Self::PrivateKey { username, .. }
            | Self::Agent { username } => username,
        }
    }

    fn method_name(&self) -> &'static str {
        match self {
            Self::Password { .. } => "password",
            Self::PrivateKey { .. } => "private-key",
            Self::Agent { .. } => "agent",
        }
    }
}

pub struct SshConnectOptions {
    pub host: String,
    pub port: u16,
    pub host_key_policy: HostKeyPolicy,
    pub timeout: Duration,
    pub credentials: SshCredentials,
}

impl fmt::Debug for SshConnectOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SshConnectOptions")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("host_key_policy", &self.host_key_policy)
            .field("timeout", &self.timeout)
            .field("credentials", &self.credentials)
            .finish()
    }
}

struct ClientHandler {
    host: String,
    port: u16,
    policy: HostKeyPolicy,
    observed_fingerprint: Arc<Mutex<Option<String>>>,
}

impl client::Handler for ClientHandler {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let public_key = server_public_key.public_key();
        let fingerprint = public_key.fingerprint(HashAlg::Sha256).to_string();
        *self
            .observed_fingerprint
            .lock()
            .expect("SSH fingerprint observation poisoned") = Some(fingerprint.clone());

        match &self.policy {
            HostKeyPolicy::PinnedFingerprint(expected) => Ok(expected == &fingerprint),
            HostKeyPolicy::KnownHosts(path) => {
                russh::keys::check_known_hosts_path(&self.host, self.port, &public_key, path)
                    .map_err(anyhow::Error::from)
            }
        }
    }
}

pub struct SshConnection {
    handle: client::Handle<ClientHandler>,
    lifecycle: ConnectionLifecycle,
}

impl SshConnection {
    pub async fn connect(options: SshConnectOptions) -> Result<Self, SshError> {
        if options.host.trim().is_empty()
            || options.port == 0
            || options.credentials.username().trim().is_empty()
        {
            return Err(SshError::InvalidOptions);
        }

        let observed_fingerprint = Arc::new(Mutex::new(None));
        let handler = ClientHandler {
            host: options.host.clone(),
            port: options.port,
            policy: options.host_key_policy,
            observed_fingerprint: Arc::clone(&observed_fingerprint),
        };
        let config = Arc::new(client::Config {
            inactivity_timeout: Some(options.timeout),
            ..Default::default()
        });

        let connect = tokio::time::timeout(
            options.timeout,
            client::connect(config, (options.host.as_str(), options.port), handler),
        )
        .await;
        let mut handle = match connect {
            Err(_) => return Err(SshError::Timeout),
            Ok(Ok(handle)) => handle,
            Ok(Err(error)) => {
                if let Some(fingerprint) = observed_fingerprint
                    .lock()
                    .expect("SSH fingerprint observation poisoned")
                    .clone()
                {
                    return Err(SshError::HostKeyRejected { fingerprint });
                }
                return Err(SshError::Handshake(error.to_string()));
            }
        };

        let authentication = tokio::time::timeout(options.timeout, async {
            match options.credentials {
                SshCredentials::Password { username, password } => handle
                    .authenticate_password(username, password.as_str())
                    .await
                    .map_err(SshError::Transport),
                SshCredentials::PrivateKey {
                    username,
                    path,
                    passphrase,
                } => {
                    let key = russh::keys::load_secret_key(
                        &path,
                        passphrase.as_ref().map(Secret::as_str),
                    )
                    .map_err(SshError::PrivateKey)?;
                    let hash = handle
                        .best_supported_rsa_hash()
                        .await
                        .map_err(SshError::Transport)?
                        .flatten();
                    handle
                        .authenticate_publickey(
                            username,
                            PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                        )
                        .await
                        .map_err(SshError::Transport)
                }
                SshCredentials::Agent { username } => {
                    authenticate_with_agent(&mut handle, username).await
                }
            }
        })
        .await
        .map_err(|_| SshError::Timeout)??;

        if !authentication.success() {
            return Err(SshError::AuthenticationRejected);
        }

        let mut lifecycle = ConnectionLifecycle::new();
        lifecycle
            .apply(ConnectionEvent::BeginConnect)
            .expect("SSH connection lifecycle must begin from Created");
        lifecycle
            .apply(ConnectionEvent::BeginAuthentication)
            .expect("SSH connection lifecycle must authenticate after connecting");
        lifecycle
            .apply(ConnectionEvent::AuthenticationSucceeded)
            .expect("SSH connection lifecycle must connect after authentication");

        Ok(Self { handle, lifecycle })
    }

    pub fn state(&self) -> ConnectionState {
        self.lifecycle.state()
    }

    pub async fn open_shell(&self, cols: u32, rows: u32) -> Result<SshShell, SshError> {
        tokio::time::timeout(Duration::from_secs(12), async {
            let channel = self
                .handle
                .channel_open_session()
                .await
                .map_err(SshError::Channel)?;
            channel
                .request_pty(false, "xterm-256color", cols.max(1), rows.max(1), 0, 0, &[])
                .await
                .map_err(SshError::Channel)?;
            channel
                .request_shell(true)
                .await
                .map_err(SshError::Channel)?;
            Ok(SshShell { channel })
        })
        .await
        .map_err(|_| SshError::Timeout)?
    }

    pub async fn disconnect(mut self) -> Result<(), SshError> {
        self.lifecycle
            .apply(ConnectionEvent::DisconnectRequested)
            .expect("connected SSH session must disconnect through the lifecycle");
        let result = self
            .handle
            .disconnect(Disconnect::ByApplication, "", "en")
            .await
            .map_err(SshError::Transport);
        if result.is_ok() {
            self.lifecycle
                .apply(ConnectionEvent::Disconnected)
                .expect("disconnecting SSH session must finish as disconnected");
        }
        result
    }

    pub async fn open_sftp(&self) -> Result<SftpConnection, SshError> {
        let session = tokio::time::timeout(Duration::from_secs(12), async {
            let channel = self
                .handle
                .channel_open_session()
                .await
                .map_err(SshError::Channel)?;
            channel
                .request_subsystem(true, "sftp")
                .await
                .map_err(SshError::Channel)?;
            russh_sftp::client::SftpSession::new(channel.into_stream())
                .await
                .map_err(|error| SshError::Sftp(error.to_string()))
        })
        .await
        .map_err(|_| SshError::Timeout)??;
        session.set_timeout(12);
        Ok(SftpConnection { session })
    }
}

#[cfg(unix)]
async fn authenticate_with_agent(
    handle: &mut client::Handle<ClientHandler>,
    username: String,
) -> Result<client::AuthResult, SshError> {
    use russh::keys::agent::{AgentIdentity, client::AgentClient};

    let mut agent = AgentClient::connect_env()
        .await
        .map_err(|error| SshError::Agent(error.to_string()))?;
    let identities = agent
        .request_identities()
        .await
        .map_err(|error| SshError::Agent(error.to_string()))?;

    for identity in identities {
        let authentication = match identity {
            AgentIdentity::PublicKey { key, .. } => handle
                .authenticate_publickey_with(username.clone(), key, None, &mut agent)
                .await
                .map_err(|error| SshError::Agent(error.to_string()))?,
            AgentIdentity::Certificate { certificate, .. } => handle
                .authenticate_certificate_with(username.clone(), certificate, None, &mut agent)
                .await
                .map_err(|error| SshError::Agent(error.to_string()))?,
        };
        if authentication.success() {
            return Ok(authentication);
        }
    }

    Err(SshError::AuthenticationRejected)
}

#[cfg(windows)]
async fn authenticate_with_agent(
    handle: &mut client::Handle<ClientHandler>,
    username: String,
) -> Result<client::AuthResult, SshError> {
    use russh::keys::agent::{AgentIdentity, client::AgentClient};

    let mut agent = AgentClient::connect_pageant()
        .await
        .map_err(|error| SshError::Agent(error.to_string()))?;
    let identities = agent
        .request_identities()
        .await
        .map_err(|error| SshError::Agent(error.to_string()))?;

    for identity in identities {
        let authentication = match identity {
            AgentIdentity::PublicKey { key, .. } => handle
                .authenticate_publickey_with(username.clone(), key, None, &mut agent)
                .await
                .map_err(|error| SshError::Agent(error.to_string()))?,
            AgentIdentity::Certificate { certificate, .. } => handle
                .authenticate_certificate_with(username.clone(), certificate, None, &mut agent)
                .await
                .map_err(|error| SshError::Agent(error.to_string()))?,
        };
        if authentication.success() {
            return Ok(authentication);
        }
    }

    Err(SshError::AuthenticationRejected)
}

pub struct SshShell {
    channel: Channel<client::Msg>,
}

/// Read-only half of an interactive SSH shell. It can run concurrently with
/// [`SshShellWriter`] so terminal input never blocks remote output.
pub struct SshShellReader {
    channel: ChannelReadHalf,
}

/// Write/control half of an interactive SSH shell.
pub struct SshShellWriter {
    channel: ChannelWriteHalf<client::Msg>,
}

/// Streaming SFTP surface. The methods copy between async readers/writers so
/// a multi-gigabyte file is never accumulated in application memory.
pub struct SftpConnection {
    session: russh_sftp::client::SftpSession,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteEntry {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub is_directory: bool,
    pub modified_unix_seconds: Option<u64>,
}

impl SftpConnection {
    pub async fn canonicalize(&self, path: impl Into<String>) -> Result<String, SshError> {
        self.session
            .canonicalize(path)
            .await
            .map_err(|error| SshError::Sftp(error.to_string()))
    }

    pub async fn read_dir(&self, path: impl Into<String>) -> Result<Vec<RemoteEntry>, SshError> {
        let entries = self
            .session
            .read_dir(path)
            .await
            .map_err(|error| SshError::Sftp(error.to_string()))?;
        Ok(entries
            .map(|entry| {
                let metadata = entry.metadata();
                let modified_unix_seconds = metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_secs());
                RemoteEntry {
                    name: entry.file_name(),
                    path: entry.path(),
                    size: metadata.len(),
                    is_directory: metadata.is_dir(),
                    modified_unix_seconds,
                }
            })
            .collect())
    }

    pub async fn download_to<R>(
        &self,
        remote_path: impl Into<String>,
        mut destination: R,
    ) -> Result<u64, SshError>
    where
        R: AsyncWrite + Unpin,
    {
        let mut file = self
            .session
            .open(remote_path)
            .await
            .map_err(|error| SshError::Sftp(error.to_string()))?;
        let copied = tokio::io::copy(&mut file, &mut destination)
            .await
            .map_err(|error| SshError::Sftp(error.to_string()))?;
        destination
            .flush()
            .await
            .map_err(|error| SshError::Sftp(error.to_string()))?;
        Ok(copied)
    }

    pub async fn upload_from<R>(
        &self,
        mut source: R,
        remote_path: impl Into<String>,
    ) -> Result<u64, SshError>
    where
        R: AsyncRead + Unpin,
    {
        let mut file = self
            .session
            .create(remote_path)
            .await
            .map_err(|error| SshError::Sftp(error.to_string()))?;
        let copied = tokio::io::copy(&mut source, &mut file)
            .await
            .map_err(|error| SshError::Sftp(error.to_string()))?;
        file.shutdown()
            .await
            .map_err(|error| SshError::Sftp(error.to_string()))?;
        Ok(copied)
    }

    pub async fn remove_file(&self, path: impl Into<String>) -> Result<(), SshError> {
        self.session
            .remove_file(path)
            .await
            .map_err(|error| SshError::Sftp(error.to_string()))
    }

    pub async fn close(&self) -> Result<(), SshError> {
        self.session
            .close()
            .await
            .map_err(|error| SshError::Sftp(error.to_string()))
    }
}

impl SshShell {
    pub fn split(self) -> (SshShellReader, SshShellWriter) {
        let (reader, writer) = self.channel.split();
        (
            SshShellReader { channel: reader },
            SshShellWriter { channel: writer },
        )
    }

    pub async fn write(&self, data: &[u8]) -> Result<(), SshError> {
        self.channel.data(data).await.map_err(SshError::Channel)
    }

    pub async fn resize(&self, cols: u32, rows: u32) -> Result<(), SshError> {
        self.channel
            .window_change(cols.max(1), rows.max(1), 0, 0)
            .await
            .map_err(SshError::Channel)
    }

    pub async fn next_output(&mut self) -> Option<Result<SshOutput, SshError>> {
        match self.channel.wait().await? {
            ChannelMsg::Data { data } => Some(Ok(SshOutput::Stdout(data.to_vec()))),
            ChannelMsg::ExtendedData { data, .. } => Some(Ok(SshOutput::Stderr(data.to_vec()))),
            ChannelMsg::ExitStatus { exit_status } => Some(Ok(SshOutput::ExitStatus(exit_status))),
            ChannelMsg::Eof | ChannelMsg::Close => None,
            _ => Some(Ok(SshOutput::Control)),
        }
    }

    pub async fn close(&self) -> Result<(), SshError> {
        self.channel.eof().await.map_err(SshError::Channel)
    }
}

impl SshShellReader {
    pub async fn next_output(&mut self) -> Option<Result<SshOutput, SshError>> {
        match self.channel.wait().await? {
            ChannelMsg::Data { data } => Some(Ok(SshOutput::Stdout(data.to_vec()))),
            ChannelMsg::ExtendedData { data, .. } => Some(Ok(SshOutput::Stderr(data.to_vec()))),
            ChannelMsg::ExitStatus { exit_status } => Some(Ok(SshOutput::ExitStatus(exit_status))),
            ChannelMsg::Eof | ChannelMsg::Close => None,
            _ => Some(Ok(SshOutput::Control)),
        }
    }
}

impl SshShellWriter {
    pub async fn write(&self, data: &[u8]) -> Result<(), SshError> {
        self.channel
            .data_bytes(data.to_vec())
            .await
            .map_err(SshError::Channel)
    }

    pub async fn resize(&self, cols: u32, rows: u32) -> Result<(), SshError> {
        self.channel
            .window_change(cols.max(1), rows.max(1), 0, 0)
            .await
            .map_err(SshError::Channel)
    }

    pub async fn close(&self) -> Result<(), SshError> {
        self.channel.eof().await.map_err(SshError::Channel)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshOutput {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    ExitStatus(u32),
    Control,
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::parse_public_key_base64;
    use std::fs;

    const TEST_KEY: &str = "AAAAC3NzaC1lZDI1NTE5AAAAILagOJFgwaMNhBWQINinKOXmqS4Gh5NgxgriXwdOoINJ";

    #[test]
    fn pinned_fingerprint_is_exact_and_not_a_tofu_accept() {
        let key = parse_public_key_base64(TEST_KEY).unwrap();
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        let accepted = HostKeyPolicy::PinnedFingerprint(fingerprint.clone());
        let rejected = HostKeyPolicy::PinnedFingerprint("SHA256:not-this-key".into());

        assert_eq!(accepted_fingerprint(&accepted, &fingerprint), Some(true));
        assert_eq!(accepted_fingerprint(&rejected, &fingerprint), Some(false));
    }

    #[test]
    fn known_hosts_policy_accepts_a_recorded_key_only() {
        let directory = tempfile::tempdir().unwrap();
        let known_hosts = directory.path().join("known_hosts");
        fs::write(
            &known_hosts,
            format!("[localhost]:2222 ssh-ed25519 {TEST_KEY}\n"),
        )
        .unwrap();
        let key = parse_public_key_base64(TEST_KEY).unwrap();

        let accepted =
            russh::keys::check_known_hosts_path("localhost", 2222, &key, &known_hosts).unwrap();
        let rejected =
            russh::keys::check_known_hosts_path("other-host", 2222, &key, &known_hosts).unwrap();
        assert!(accepted);
        assert!(!rejected);
    }

    #[test]
    fn credential_debug_is_redacted() {
        let credentials = SshCredentials::password("ops", "do-not-print-me");
        let debug = format!("{credentials:?}");
        assert!(debug.contains("ops"));
        assert!(!debug.contains("do-not-print-me"));
    }

    #[test]
    fn agent_credentials_have_no_secret_bearing_debug_fields() {
        let debug = format!("{:?}", SshCredentials::agent("ops"));
        assert!(debug.contains("agent"));
        assert!(debug.contains("ops"));
    }

    fn accepted_fingerprint(policy: &HostKeyPolicy, fingerprint: &str) -> Option<bool> {
        match policy {
            HostKeyPolicy::PinnedFingerprint(expected) => Some(expected == fingerprint),
            HostKeyPolicy::KnownHosts(_) => None,
        }
    }
}
