//! Rust-owned SSH transport. The crate deliberately keeps credential material
//! out of serde models and out of the frontend-facing data types.

use std::collections::VecDeque;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mobarust_core::{ConnectionEvent, ConnectionLifecycle, ConnectionState};
use russh::client;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{Channel, ChannelMsg, ChannelReadHalf, ChannelWriteHalf, Disconnect};
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot};
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
    #[error("remote monitoring command failed with exit status {0}")]
    RemoteMonitorCommandFailed(u32),
    #[error("remote monitoring output exceeded its safety limit")]
    RemoteMonitorOutputTooLarge,
    #[error("remote monitoring is not supported by this host")]
    RemoteMonitorUnsupported,
    #[error("SFTP operation failed: {0}")]
    Sftp(String),
    #[error("remote file changed since it was opened")]
    RemoteConflict,
    #[error("remote text file exceeds the 4 MiB editor limit")]
    RemoteFileTooLarge,
    #[error("remote file is not valid UTF-8 text")]
    RemoteFileNotUtf8,
    #[error("SCP operation failed: {0}")]
    Scp(String),
    #[error("local file operation failed: {0}")]
    LocalIo(#[source] std::io::Error),
    #[error("SFTP transfer cancelled")]
    Cancelled,
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

/// A complete SSH connection description for one hop in a jump chain. The
/// credentials remain native and are consumed during authentication.
pub type SshJumpOptions = SshConnectOptions;

/// An SSH host-key observation that contains no authentication material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshHostKeyInspection {
    pub host: String,
    pub port: u16,
    pub fingerprint: String,
}

/// Options for a one-shot host-key inspection. This deliberately has no
/// username, credential, known_hosts path, or SSH-agent setting.
pub struct SshFingerprintOptions {
    pub host: String,
    pub port: u16,
    pub timeout: Duration,
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
    inspection_only: bool,
    observed_fingerprint: Arc<Mutex<Option<String>>>,
    forwarded_channels: mpsc::UnboundedSender<SshForwardedChannel>,
}

struct ConnectionParts {
    observed_fingerprint: Arc<Mutex<Option<String>>>,
    handler: ClientHandler,
    config: Arc<client::Config>,
    forwarded_channels: mpsc::UnboundedReceiver<SshForwardedChannel>,
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

        if self.inspection_only {
            return Ok(true);
        }

        match &self.policy {
            HostKeyPolicy::PinnedFingerprint(expected) => Ok(expected == &fingerprint),
            HostKeyPolicy::KnownHosts(path) => {
                russh::keys::check_known_hosts_path(&self.host, self.port, &public_key, path)
                    .map_err(anyhow::Error::from)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<client::Msg>,
        connected_address: &str,
        connected_port: u32,
        originator_address: &str,
        originator_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        let forwarded_channels = self.forwarded_channels.clone();
        let connected_address = connected_address.to_owned();
        let originator_address = originator_address.to_owned();
        async move {
            reply.accept().await;
            let _ = forwarded_channels.send(SshForwardedChannel {
                channel,
                connected_address,
                connected_port,
                originator_address,
                originator_port,
            });
            Ok(())
        }
    }
}

pub struct SshConnection {
    handle: Arc<client::Handle<ClientHandler>>,
    lifecycle: Mutex<ConnectionLifecycle>,
    parent: Option<Arc<SshConnection>>,
    forwarded_channels: AsyncMutex<mpsc::UnboundedReceiver<SshForwardedChannel>>,
}

/// Performs a one-shot SSH handshake to observe the server host key, then
/// closes the transport immediately. No authentication is attempted and no
/// user key, agent, or known_hosts file is consulted.
pub async fn inspect_host_key(
    options: SshFingerprintOptions,
) -> Result<SshHostKeyInspection, SshError> {
    validate_fingerprint_options(&options)?;
    let ConnectionParts {
        observed_fingerprint,
        handler,
        config,
        ..
    } = inspection_connection_parts(&options);
    let connect = tokio::time::timeout(
        options.timeout,
        client::connect(config, (options.host.as_str(), options.port), handler),
    )
    .await;
    let handle = map_connect_result(connect, Arc::clone(&observed_fingerprint))?;
    let fingerprint = observed_fingerprint
        .lock()
        .expect("SSH fingerprint observation poisoned")
        .clone()
        .ok_or_else(|| SshError::Handshake("SSH server did not present a host key".into()))?;

    // Dropping the handle also closes the transport, but attempt the protocol
    // disconnect first. A failed close must not hide the already observed key.
    let _ = tokio::time::timeout(
        options.timeout,
        handle.disconnect(Disconnect::ByApplication, "", "en"),
    )
    .await;

    Ok(SshHostKeyInspection {
        host: options.host,
        port: options.port,
        fingerprint,
    })
}

const REMOTE_MONITOR_OUTPUT_LIMIT: usize = 64 * 1024;
const REMOTE_MONITOR_TIMEOUT: Duration = Duration::from_secs(6);

/// A best-effort, one-shot snapshot collected with a fixed command. Missing
/// fields are represented as `None` because remote systems are not assumed to
/// be GNU/Linux and no agent is installed on the host.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteMonitorSnapshot {
    pub hostname: Option<String>,
    pub kernel: Option<String>,
    pub uptime_seconds: Option<u64>,
    pub load_average: Option<[f64; 3]>,
    pub memory_total_bytes: Option<u64>,
    pub memory_available_bytes: Option<u64>,
    pub root_disk_used_percent: Option<u8>,
    pub process_count: Option<u64>,
    pub supported_metrics: Vec<String>,
}

struct RemoteCommandOutput {
    stdout: Vec<u8>,
    #[allow(dead_code)]
    stderr: Vec<u8>,
    exit_status: Option<u32>,
}

// This string is intentionally constant. User input never reaches a remote
// shell through the monitoring API. Every individual metric is optional so a
// BSD host, a minimal appliance, or a host without /proc can degrade cleanly.
const REMOTE_MONITOR_COMMAND: &[u8] = br#"sh -c 'printf "__MOBARUST__hostname=%s\n" "$(hostname 2>/dev/null || true)"; printf "__MOBARUST__kernel=%s\n" "$(uname -sr 2>/dev/null || true)"; if [ -r /proc/uptime ]; then read uptime_seconds uptime_rest < /proc/uptime; printf "__MOBARUST__uptime_seconds=%s\n" "$uptime_seconds"; fi; if [ -r /proc/loadavg ]; then read load_one load_two load_three load_rest < /proc/loadavg; printf "__MOBARUST__load=%s,%s,%s\n" "$load_one" "$load_two" "$load_three"; elif command -v sysctl >/dev/null 2>&1; then load_line=$(sysctl -n vm.loadavg 2>/dev/null || true); set -- $load_line; printf "__MOBARUST__load=%s,%s,%s\n" "$(printf %s "$1" | tr -d "{}")" "$2" "$3"; fi; if [ -r /proc/meminfo ]; then mem_total=$(grep ^MemTotal: /proc/meminfo 2>/dev/null || true); set -- $mem_total; printf "__MOBARUST__mem_total_kib=%s\n" "$2"; mem_available=$(grep ^MemAvailable: /proc/meminfo 2>/dev/null || true); set -- $mem_available; printf "__MOBARUST__mem_available_kib=%s\n" "$2"; elif command -v sysctl >/dev/null 2>&1; then printf "__MOBARUST__mem_total_bytes=%s\n" "$(sysctl -n hw.memsize 2>/dev/null || true)"; fi; disk_line=$(df -P / 2>/dev/null | tail -n 1 || true); set -- $disk_line; printf "__MOBARUST__disk_root_used_percent=%s\n" "$(printf %s "$5" | tr -d "%")"; if command -v ps >/dev/null 2>&1; then printf "__MOBARUST__process_count=%s\n" "$(ps -e 2>/dev/null | tail -n +2 | wc -l | tr -d " ")"; fi'"#;

/// A server-initiated channel created by an SSH remote `-R` forward.
///
/// The channel is delivered only after the server-side open request has been
/// accepted. The caller owns the stream and must apply its own bounded worker
/// and cancellation policy.
pub struct SshForwardedChannel {
    channel: Channel<client::Msg>,
    pub connected_address: String,
    pub connected_port: u32,
    pub originator_address: String,
    pub originator_port: u32,
}

impl SshForwardedChannel {
    pub fn into_stream(self) -> russh::ChannelStream<client::Msg> {
        self.channel.into_stream()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Socks5Request {
    pub target_host: String,
    pub target_port: u16,
}

#[derive(Debug, Error)]
pub enum Socks5Error {
    #[error("SOCKS5 I/O failed: {0}")]
    Io(#[source] std::io::Error),
    #[error("unsupported SOCKS version {0}")]
    UnsupportedVersion(u8),
    #[error("SOCKS5 client offered no unauthenticated method")]
    NoUnauthenticatedMethod,
    #[error("SOCKS5 command is not CONNECT: {0}")]
    UnsupportedCommand(u8),
    #[error("SOCKS5 address type is unsupported: {0}")]
    UnsupportedAddressType(u8),
    #[error("SOCKS5 domain name is empty")]
    EmptyDomain,
    #[error("SOCKS5 target port is zero")]
    InvalidPort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Socks5ReplyCode {
    Succeeded = 0,
    GeneralFailure = 1,
    ConnectionNotAllowed = 2,
    NetworkUnreachable = 3,
    HostUnreachable = 4,
    ConnectionRefused = 5,
    TtlExpired = 6,
    CommandNotSupported = 7,
    AddressTypeNotSupported = 8,
}

/// Performs the bounded unauthenticated SOCKS5 CONNECT handshake. SSH
/// authentication protects the upstream connection; local proxy clients are
/// deliberately not given a second arbitrary authentication surface here.
pub async fn negotiate_socks5<S>(stream: &mut S) -> Result<Socks5Request, Socks5Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let version = stream.read_u8().await.map_err(Socks5Error::Io)?;
    if version != 5 {
        return Err(Socks5Error::UnsupportedVersion(version));
    }
    let method_count = stream.read_u8().await.map_err(Socks5Error::Io)? as usize;
    if method_count > 32 {
        stream
            .write_all(&[5, 0xff])
            .await
            .map_err(Socks5Error::Io)?;
        return Err(Socks5Error::NoUnauthenticatedMethod);
    }
    let mut methods = vec![0_u8; method_count];
    stream
        .read_exact(&mut methods)
        .await
        .map_err(Socks5Error::Io)?;
    if !methods.contains(&0) {
        stream
            .write_all(&[5, 0xff])
            .await
            .map_err(Socks5Error::Io)?;
        return Err(Socks5Error::NoUnauthenticatedMethod);
    }
    stream.write_all(&[5, 0]).await.map_err(Socks5Error::Io)?;

    let version = stream.read_u8().await.map_err(Socks5Error::Io)?;
    if version != 5 {
        return Err(Socks5Error::UnsupportedVersion(version));
    }
    let command = stream.read_u8().await.map_err(Socks5Error::Io)?;
    if command != 1 {
        let _ = send_socks5_reply(stream, Socks5ReplyCode::CommandNotSupported).await;
        return Err(Socks5Error::UnsupportedCommand(command));
    }
    let _reserved = stream.read_u8().await.map_err(Socks5Error::Io)?;
    let address_type = stream.read_u8().await.map_err(Socks5Error::Io)?;
    let target_host = match address_type {
        1 => {
            let mut bytes = [0_u8; 4];
            stream
                .read_exact(&mut bytes)
                .await
                .map_err(Socks5Error::Io)?;
            std::net::Ipv4Addr::from(bytes).to_string()
        }
        3 => {
            let length = stream.read_u8().await.map_err(Socks5Error::Io)? as usize;
            let mut bytes = vec![0_u8; length];
            stream
                .read_exact(&mut bytes)
                .await
                .map_err(Socks5Error::Io)?;
            let host = String::from_utf8_lossy(&bytes).trim().to_owned();
            if host.is_empty() {
                return Err(Socks5Error::EmptyDomain);
            }
            host
        }
        4 => {
            let mut bytes = [0_u8; 16];
            stream
                .read_exact(&mut bytes)
                .await
                .map_err(Socks5Error::Io)?;
            std::net::Ipv6Addr::from(bytes).to_string()
        }
        other => {
            let _ = send_socks5_reply(stream, Socks5ReplyCode::AddressTypeNotSupported).await;
            return Err(Socks5Error::UnsupportedAddressType(other));
        }
    };
    let target_port = stream.read_u16().await.map_err(Socks5Error::Io)?;
    if target_port == 0 {
        let _ = send_socks5_reply(stream, Socks5ReplyCode::GeneralFailure).await;
        return Err(Socks5Error::InvalidPort);
    }
    Ok(Socks5Request {
        target_host,
        target_port,
    })
}

pub async fn send_socks5_reply<S>(stream: &mut S, reply: Socks5ReplyCode) -> Result<(), Socks5Error>
where
    S: AsyncWrite + Unpin,
{
    stream
        .write_all(&[5, reply as u8, 0, 1, 0, 0, 0, 0, 0, 0])
        .await
        .map_err(Socks5Error::Io)
}

impl SshConnection {
    pub async fn connect(options: SshConnectOptions) -> Result<Self, SshError> {
        validate_options(&options)?;
        let ConnectionParts {
            observed_fingerprint,
            handler,
            config,
            forwarded_channels,
        } = connection_parts(&options);
        let connect = tokio::time::timeout(
            options.timeout,
            client::connect(config, (options.host.as_str(), options.port), handler),
        )
        .await;
        let handle = map_connect_result(connect, observed_fingerprint)?;
        Self::finish_authenticated(options, handle, None, forwarded_channels).await
    }

    /// Connects through one or more already described jump hosts. Every hop
    /// is a real SSH connection; the target handshake travels through the
    /// previous hop's `direct-tcpip` channel.
    pub async fn connect_with_jump_chain(
        options: SshConnectOptions,
        mut jumps: Vec<SshJumpOptions>,
    ) -> Result<Self, SshError> {
        validate_options(&options)?;
        let Some(first) = jumps.first() else {
            return Self::connect(options).await;
        };
        validate_options(first)?;
        let first = jumps.remove(0);
        let mut upstream = Arc::new(Self::connect(first).await?);
        for jump in jumps {
            upstream = Arc::new(Self::connect_via_upstream(jump, upstream).await?);
        }
        Self::connect_via_upstream(options, upstream).await
    }

    async fn connect_via_upstream(
        options: SshConnectOptions,
        upstream: Arc<SshConnection>,
    ) -> Result<Self, SshError> {
        validate_options(&options)?;
        let stream = upstream
            .open_direct_tcpip(options.host.clone(), u32::from(options.port))
            .await?;
        let ConnectionParts {
            observed_fingerprint,
            handler,
            config,
            forwarded_channels,
        } = connection_parts(&options);
        let connect = tokio::time::timeout(
            options.timeout,
            client::connect_stream(config, stream, handler),
        )
        .await;
        let handle = map_connect_result(connect, observed_fingerprint)?;
        Self::finish_authenticated(options, handle, Some(upstream), forwarded_channels).await
    }

    async fn finish_authenticated(
        options: SshConnectOptions,
        mut handle: client::Handle<ClientHandler>,
        parent: Option<Arc<SshConnection>>,
        forwarded_channels: mpsc::UnboundedReceiver<SshForwardedChannel>,
    ) -> Result<Self, SshError> {
        let authentication = tokio::time::timeout(
            options.timeout,
            authenticate(&mut handle, options.credentials),
        )
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

        Ok(Self {
            handle: Arc::new(handle),
            lifecycle: Mutex::new(lifecycle),
            parent,
            forwarded_channels: AsyncMutex::new(forwarded_channels),
        })
    }

    pub fn state(&self) -> ConnectionState {
        self.lifecycle
            .lock()
            .expect("SSH lifecycle lock poisoned")
            .state()
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

    /// Collects a bounded, one-shot system snapshot through a fixed remote
    /// command. This is deliberately separate from an arbitrary remote exec
    /// API: the frontend can request monitoring, but cannot provide shell text.
    pub async fn remote_monitor_snapshot(&self) -> Result<RemoteMonitorSnapshot, SshError> {
        let output = self
            .exec_bounded(REMOTE_MONITOR_COMMAND, REMOTE_MONITOR_OUTPUT_LIMIT)
            .await?;
        if let Some(status) = output.exit_status
            && status != 0
        {
            return Err(SshError::RemoteMonitorCommandFailed(status));
        }
        parse_remote_monitor_snapshot(&output.stdout)
    }

    async fn exec_bounded(
        &self,
        command: &[u8],
        output_limit: usize,
    ) -> Result<RemoteCommandOutput, SshError> {
        tokio::time::timeout(REMOTE_MONITOR_TIMEOUT, async {
            let channel = self
                .handle
                .channel_open_session()
                .await
                .map_err(SshError::Channel)?;
            channel
                .exec(true, command.to_vec())
                .await
                .map_err(SshError::Channel)?;
            let mut channel = channel;
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let mut exit_status = None;
            while let Some(message) = channel.wait().await {
                match message {
                    ChannelMsg::Data { data } => {
                        if stdout
                            .len()
                            .saturating_add(stderr.len())
                            .saturating_add(data.len())
                            > output_limit
                        {
                            return Err(SshError::RemoteMonitorOutputTooLarge);
                        }
                        stdout.extend_from_slice(&data);
                    }
                    ChannelMsg::ExtendedData { data, .. } => {
                        if stdout
                            .len()
                            .saturating_add(stderr.len())
                            .saturating_add(data.len())
                            > output_limit
                        {
                            return Err(SshError::RemoteMonitorOutputTooLarge);
                        }
                        stderr.extend_from_slice(&data);
                    }
                    ChannelMsg::ExitStatus {
                        exit_status: status,
                    } => exit_status = Some(status),
                    ChannelMsg::Eof | ChannelMsg::Close => break,
                    _ => {}
                }
            }
            Ok(RemoteCommandOutput {
                stdout,
                stderr,
                exit_status,
            })
        })
        .await
        .map_err(|_| SshError::Timeout)?
    }

    pub async fn disconnect(&self) -> Result<(), SshError> {
        let result = self.disconnect_one().await;
        if result.is_ok() {
            let mut parent = self.parent.clone();
            while let Some(connection) = parent {
                parent = connection.parent.clone();
                let _ = connection.disconnect_one().await;
            }
        }
        result
    }

    async fn disconnect_one(&self) -> Result<(), SshError> {
        self.lifecycle
            .lock()
            .expect("SSH lifecycle lock poisoned")
            .apply(ConnectionEvent::DisconnectRequested)
            .expect("connected SSH session must disconnect through the lifecycle");
        let result = self
            .handle
            .disconnect(Disconnect::ByApplication, "", "en")
            .await
            .map_err(SshError::Transport);
        if result.is_ok() {
            self.lifecycle
                .lock()
                .expect("SSH lifecycle lock poisoned")
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

    pub async fn scp_upload<R>(
        &self,
        remote_path: impl Into<String>,
        size: u64,
        source: R,
    ) -> Result<u64, SshError>
    where
        R: AsyncRead + Unpin,
    {
        let (_cancel_sender, mut cancel) = oneshot::channel();
        self.scp_upload_with_cancel(remote_path, size, source, &mut cancel, |_| {})
            .await
    }

    pub async fn scp_upload_with_cancel<R, F>(
        &self,
        remote_path: impl Into<String>,
        size: u64,
        mut source: R,
        cancel: &mut oneshot::Receiver<()>,
        mut on_progress: F,
    ) -> Result<u64, SshError>
    where
        R: AsyncRead + Unpin,
        F: FnMut(u64),
    {
        let remote_path = remote_path.into();
        let file_name = scp_file_name(&remote_path)?;
        let command = format!("scp -O -t {}", shell_quote(&remote_path)?);
        let mut channel = tokio::select! {
            _ = &mut *cancel => return Err(SshError::Cancelled),
            result = self.open_scp_channel(command.into_bytes()) => result?,
        };
        channel.read_ack_with_cancel(cancel).await?;
        channel
            .write_bytes_with_cancel(format!("C0644 {size} {file_name}\n").into_bytes(), cancel)
            .await?;
        channel.read_ack_with_cancel(cancel).await?;

        let mut buffer = vec![0_u8; 64 * 1024];
        let mut copied = 0_u64;
        while copied < size {
            let remaining = size - copied;
            let read_limit = remaining.min(buffer.len() as u64) as usize;
            let read = tokio::select! {
                _ = &mut *cancel => return Err(SshError::Cancelled),
                result = source.read(&mut buffer[..read_limit]) => result.map_err(SshError::LocalIo)?,
            };
            if read == 0 {
                return Err(SshError::Scp(format!(
                    "source ended before declared size ({copied}/{size} bytes)"
                )));
            }
            channel
                .write_bytes_with_cancel(buffer[..read].to_vec(), cancel)
                .await?;
            copied += read as u64;
            on_progress(copied);
            if cancel.try_recv().is_ok() {
                return Err(SshError::Cancelled);
            }
        }
        channel.write_bytes_with_cancel(vec![0], cancel).await?;
        channel.read_ack_with_cancel(cancel).await?;
        channel.close().await?;
        Ok(copied)
    }

    pub async fn scp_download<W>(
        &self,
        remote_path: impl Into<String>,
        destination: W,
    ) -> Result<u64, SshError>
    where
        W: AsyncWrite + Unpin,
    {
        let (_cancel_sender, mut cancel) = oneshot::channel();
        self.scp_download_with_cancel(remote_path, destination, &mut cancel, |_, _| {})
            .await
    }

    pub async fn scp_download_with_cancel<W, F>(
        &self,
        remote_path: impl Into<String>,
        mut destination: W,
        cancel: &mut oneshot::Receiver<()>,
        mut on_progress: F,
    ) -> Result<u64, SshError>
    where
        W: AsyncWrite + Unpin,
        F: FnMut(u64, u64),
    {
        let remote_path = remote_path.into();
        let command = format!("scp -O -f {}", shell_quote(&remote_path)?);
        let mut channel = tokio::select! {
            _ = &mut *cancel => return Err(SshError::Cancelled),
            result = self.open_scp_channel(command.into_bytes()) => result?,
        };
        channel.write_bytes_with_cancel(vec![0], cancel).await?;
        let metadata = channel.read_line_with_cancel(cancel).await?;
        let size = parse_scp_metadata(&metadata)?;
        channel.write_bytes_with_cancel(vec![0], cancel).await?;
        let mut remaining = size;
        let mut copied = 0_u64;
        let mut buffer = vec![0_u8; 64 * 1024];
        while remaining > 0 {
            let read = tokio::select! {
                _ = &mut *cancel => return Err(SshError::Cancelled),
                result = channel.read_bytes(&mut buffer, remaining) => result?,
            };
            if read == 0 {
                return Err(SshError::Scp("source ended before declared size".into()));
            }
            tokio::select! {
                _ = &mut *cancel => return Err(SshError::Cancelled),
                result = destination.write_all(&buffer[..read]) => result.map_err(SshError::LocalIo)?,
            }
            copied += read as u64;
            remaining -= read as u64;
            on_progress(copied, size);
            if cancel.try_recv().is_ok() {
                return Err(SshError::Cancelled);
            }
        }
        tokio::select! {
            _ = &mut *cancel => return Err(SshError::Cancelled),
            result = destination.flush() => result.map_err(SshError::LocalIo)?,
        }
        channel.read_ack_with_cancel(cancel).await?;
        channel.write_bytes_with_cancel(vec![0], cancel).await?;
        channel.close().await?;
        Ok(copied)
    }

    async fn open_scp_channel(&self, command: Vec<u8>) -> Result<ScpChannel, SshError> {
        let channel = tokio::time::timeout(Duration::from_secs(12), async {
            let channel = self
                .handle
                .channel_open_session()
                .await
                .map_err(SshError::Channel)?;
            channel
                .exec(true, command)
                .await
                .map_err(SshError::Channel)?;
            Ok::<_, SshError>(channel)
        })
        .await
        .map_err(|_| SshError::Timeout)??;
        Ok(ScpChannel {
            channel,
            buffer: VecDeque::new(),
            closed: false,
        })
    }

    /// Opens one SSH direct-tcpip channel for local port forwarding. The
    /// caller owns the local listener and the lifecycle of the returned
    /// bidirectional stream.
    pub async fn open_direct_tcpip(
        &self,
        target_host: impl Into<String>,
        target_port: u32,
    ) -> Result<russh::ChannelStream<client::Msg>, SshError> {
        if target_port == 0 {
            return Err(SshError::InvalidOptions);
        }
        let target_host = target_host.into();
        if target_host.trim().is_empty() {
            return Err(SshError::InvalidOptions);
        }
        let channel = tokio::time::timeout(
            Duration::from_secs(12),
            self.handle
                .channel_open_direct_tcpip(target_host, target_port, "127.0.0.1", 0),
        )
        .await
        .map_err(|_| SshError::Timeout)?
        .map_err(SshError::Channel)?;
        Ok(channel.into_stream())
    }

    /// Requests the SSH server to listen on a remote endpoint. A returned
    /// forwarded channel becomes available through `next_forwarded_channel`.
    pub async fn request_remote_forward(
        &self,
        address: impl Into<String>,
        port: u32,
    ) -> Result<u16, SshError> {
        let address = address.into();
        if address.trim().is_empty() || address.contains('\0') {
            return Err(SshError::InvalidOptions);
        }
        let port = tokio::time::timeout(
            Duration::from_secs(12),
            self.handle.tcpip_forward(address, port),
        )
        .await
        .map_err(|_| SshError::Timeout)?
        .map_err(SshError::Channel)?;
        u16::try_from(port).map_err(|_| SshError::InvalidOptions)
    }

    pub async fn cancel_remote_forward(
        &self,
        address: impl Into<String>,
        port: u32,
    ) -> Result<(), SshError> {
        let address = address.into();
        if address.trim().is_empty() || address.contains('\0') || port == 0 {
            return Err(SshError::InvalidOptions);
        }
        tokio::time::timeout(
            Duration::from_secs(12),
            self.handle.cancel_tcpip_forward(address, port),
        )
        .await
        .map_err(|_| SshError::Timeout)?
        .map_err(SshError::Channel)
    }

    pub async fn next_forwarded_channel(&self) -> Option<SshForwardedChannel> {
        self.forwarded_channels.lock().await.recv().await
    }
}

struct ScpChannel {
    channel: Channel<client::Msg>,
    buffer: VecDeque<u8>,
    closed: bool,
}

impl ScpChannel {
    async fn fill(&mut self) -> Result<(), SshError> {
        if self.closed {
            return Ok(());
        }
        match self.channel.wait().await {
            Some(ChannelMsg::Data { data }) => self.buffer.extend(data),
            Some(ChannelMsg::ExtendedData { data, .. }) => {
                return Err(SshError::Scp(String::from_utf8_lossy(&data).trim().into()));
            }
            Some(ChannelMsg::Eof | ChannelMsg::Close) | None => self.closed = true,
            Some(ChannelMsg::ExitStatus { exit_status }) if exit_status != 0 => {
                return Err(SshError::Scp(format!(
                    "remote scp exited with status {exit_status}"
                )));
            }
            Some(_) => {}
        }
        Ok(())
    }

    async fn read_byte(&mut self) -> Result<u8, SshError> {
        loop {
            if let Some(byte) = self.buffer.pop_front() {
                return Ok(byte);
            }
            self.fill().await?;
            if self.closed && self.buffer.is_empty() {
                return Err(SshError::Scp(
                    "remote scp closed before the response".into(),
                ));
            }
        }
    }

    async fn read_line_with_cancel(
        &mut self,
        cancel: &mut oneshot::Receiver<()>,
    ) -> Result<Vec<u8>, SshError> {
        const MAX_LINE_BYTES: usize = 4096;
        let mut line = Vec::new();
        loop {
            let byte = tokio::select! {
                _ = &mut *cancel => return Err(SshError::Cancelled),
                result = self.read_byte() => result?,
            };
            if byte == b'\n' {
                return Ok(line);
            }
            line.push(byte);
            if line.len() >= MAX_LINE_BYTES {
                return Err(SshError::Scp("remote scp control line is too long".into()));
            }
        }
    }

    async fn read_ack_with_cancel(
        &mut self,
        cancel: &mut oneshot::Receiver<()>,
    ) -> Result<(), SshError> {
        loop {
            let byte = tokio::select! {
                _ = &mut *cancel => return Err(SshError::Cancelled),
                result = self.read_byte() => result?,
            };
            match byte {
                0 => return Ok(()),
                1 => {
                    let _warning = self.read_line_with_cancel(cancel).await?;
                }
                2 => {
                    let error = String::from_utf8_lossy(&self.read_line_with_cancel(cancel).await?)
                        .into_owned();
                    return Err(SshError::Scp(error));
                }
                code => return Err(SshError::Scp(format!("invalid scp acknowledgement {code}"))),
            }
        }
    }

    async fn read_bytes(
        &mut self,
        destination: &mut [u8],
        maximum: u64,
    ) -> Result<usize, SshError> {
        while self.buffer.is_empty() {
            self.fill().await?;
            if self.closed {
                return Ok(0);
            }
        }
        let count = destination
            .len()
            .min(maximum as usize)
            .min(self.buffer.len());
        for byte in &mut destination[..count] {
            *byte = self
                .buffer
                .pop_front()
                .expect("count comes from buffer length");
        }
        Ok(count)
    }

    async fn write_bytes_with_cancel(
        &self,
        bytes: Vec<u8>,
        cancel: &mut oneshot::Receiver<()>,
    ) -> Result<(), SshError> {
        tokio::select! {
            _ = &mut *cancel => Err(SshError::Cancelled),
            result = self.channel.data_bytes(bytes) => result.map_err(SshError::Channel),
        }
    }

    async fn close(&self) -> Result<(), SshError> {
        self.channel.eof().await.map_err(SshError::Channel)
    }
}

fn scp_file_name(path: &str) -> Result<String, SshError> {
    let name = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| SshError::Scp("remote path has no file name".into()))?;
    if name.contains('\0') || name.contains('\n') {
        return Err(SshError::Scp(
            "remote file name contains a control character".into(),
        ));
    }
    Ok(name.to_owned())
}

fn shell_quote(path: &str) -> Result<String, SshError> {
    if path.trim().is_empty() || path.contains('\0') || path.contains('\n') || path.contains('\r') {
        return Err(SshError::Scp(
            "remote path contains an invalid control character".into(),
        ));
    }
    Ok(format!("'{}'", path.replace('\'', "'\\''")))
}

fn parse_scp_metadata(line: &[u8]) -> Result<u64, SshError> {
    let line = String::from_utf8_lossy(line);
    let mut fields = line.splitn(3, ' ');
    let mode = fields.next().unwrap_or_default();
    let size = fields.next().unwrap_or_default();
    let name = fields.next().unwrap_or_default();
    if !mode.starts_with('C') || name.is_empty() {
        return Err(SshError::Scp(format!("invalid scp metadata: {line}")));
    }
    size.parse::<u64>()
        .map_err(|_| SshError::Scp("invalid scp file size".into()))
}

fn validate_options(options: &SshConnectOptions) -> Result<(), SshError> {
    if options.host.trim().is_empty()
        || options.host.contains('\0')
        || options.port == 0
        || options.credentials.username().trim().is_empty()
    {
        return Err(SshError::InvalidOptions);
    }
    Ok(())
}

fn validate_fingerprint_options(options: &SshFingerprintOptions) -> Result<(), SshError> {
    if options.host.trim().is_empty()
        || options.host.chars().any(char::is_control)
        || options.port == 0
        || options.timeout.is_zero()
        || options.timeout > Duration::from_secs(60)
    {
        return Err(SshError::InvalidOptions);
    }
    Ok(())
}

fn connection_parts(options: &SshConnectOptions) -> ConnectionParts {
    connection_parts_for(
        &options.host,
        options.port,
        options.host_key_policy.clone(),
        false,
        options.timeout,
    )
}

fn inspection_connection_parts(options: &SshFingerprintOptions) -> ConnectionParts {
    connection_parts_for(
        &options.host,
        options.port,
        // The policy is unused in observation mode. Keeping a concrete value
        // here avoids adding an accept-any variant to the public connection
        // policy, where it could be misused for an authenticated session.
        HostKeyPolicy::PinnedFingerprint(String::new()),
        true,
        options.timeout,
    )
}

fn connection_parts_for(
    host: &str,
    port: u16,
    policy: HostKeyPolicy,
    inspection_only: bool,
    timeout: Duration,
) -> ConnectionParts {
    let observed_fingerprint = Arc::new(Mutex::new(None));
    let (forwarded_sender, forwarded_receiver) = mpsc::unbounded_channel();
    let handler = ClientHandler {
        host: host.to_owned(),
        port,
        policy,
        inspection_only,
        observed_fingerprint: Arc::clone(&observed_fingerprint),
        forwarded_channels: forwarded_sender,
    };
    let config = Arc::new(client::Config {
        inactivity_timeout: Some(timeout),
        ..Default::default()
    });
    ConnectionParts {
        observed_fingerprint,
        handler,
        config,
        forwarded_channels: forwarded_receiver,
    }
}

fn map_connect_result(
    connect: Result<
        Result<client::Handle<ClientHandler>, anyhow::Error>,
        tokio::time::error::Elapsed,
    >,
    observed_fingerprint: Arc<Mutex<Option<String>>>,
) -> Result<client::Handle<ClientHandler>, SshError> {
    match connect {
        Err(_) => Err(SshError::Timeout),
        Ok(Ok(handle)) => Ok(handle),
        Ok(Err(error)) => {
            if let Some(fingerprint) = observed_fingerprint
                .lock()
                .expect("SSH fingerprint observation poisoned")
                .clone()
            {
                return Err(SshError::HostKeyRejected { fingerprint });
            }
            Err(SshError::Handshake(error.to_string()))
        }
    }
}

async fn authenticate(
    handle: &mut client::Handle<ClientHandler>,
    credentials: SshCredentials,
) -> Result<client::AuthResult, SshError> {
    match credentials {
        SshCredentials::Password { username, password } => handle
            .authenticate_password(username, password.as_str())
            .await
            .map_err(SshError::Transport),
        SshCredentials::PrivateKey {
            username,
            path,
            passphrase,
        } => {
            let key = russh::keys::load_secret_key(&path, passphrase.as_ref().map(Secret::as_str))
                .map_err(SshError::PrivateKey)?;
            let hash = handle
                .best_supported_rsa_hash()
                .await
                .map_err(SshError::Transport)?
                .flatten();
            handle
                .authenticate_publickey(username, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                .await
                .map_err(SshError::Transport)
        }
        SshCredentials::Agent { username } => authenticate_with_agent(handle, username).await,
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

pub const MAX_REMOTE_EDITOR_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTextDocument {
    pub path: String,
    pub content: String,
    pub revision: String,
    pub size: u64,
    pub modified_unix_seconds: Option<u64>,
    pub permissions: Option<u32>,
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

    pub async fn file_info(&self, path: impl Into<String>) -> Result<(u64, bool), SshError> {
        let metadata = self
            .session
            .metadata(path)
            .await
            .map_err(|error| SshError::Sftp(error.to_string()))?;
        Ok((metadata.len(), metadata.is_dir()))
    }

    /// Read a bounded UTF-8 document together with a content revision. The
    /// bound keeps the editor path from becoming an accidental large-file
    /// transfer mechanism.
    pub async fn read_text_document(
        &self,
        path: impl Into<String>,
    ) -> Result<RemoteTextDocument, SshError> {
        let path = path.into();
        let metadata = self
            .session
            .metadata(&path)
            .await
            .map_err(|error| SshError::Sftp(error.to_string()))?;
        if metadata.is_dir() {
            return Err(SshError::Sftp("remote path is a directory".into()));
        }
        if metadata.len() > MAX_REMOTE_EDITOR_BYTES as u64 {
            return Err(SshError::RemoteFileTooLarge);
        }
        let mut file = self
            .session
            .open(&path)
            .await
            .map_err(|error| SshError::Sftp(error.to_string()))?;
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        let mut limited = (&mut file).take((MAX_REMOTE_EDITOR_BYTES + 1) as u64);
        let result = limited
            .read_to_end(&mut bytes)
            .await
            .map_err(|error| SshError::Sftp(error.to_string()));
        let close_result = file
            .close()
            .await
            .map_err(|error| SshError::Sftp(error.to_string()));
        result?;
        close_result?;
        if bytes.len() > MAX_REMOTE_EDITOR_BYTES {
            return Err(SshError::RemoteFileTooLarge);
        }
        let content = String::from_utf8(bytes).map_err(|_| SshError::RemoteFileNotUtf8)?;
        Ok(RemoteTextDocument {
            path,
            revision: text_revision(content.as_bytes()),
            size: content.len() as u64,
            modified_unix_seconds: metadata.mtime.map(u64::from),
            permissions: metadata.permissions,
            content,
        })
    }

    /// Replace a document through a remote temporary file after rechecking
    /// the content revision. The original mode is reapplied before a
    /// rollback-safe promotion. SFTP servers without POSIX rename semantics
    /// cannot guarantee a zero-gap replacement, so that limitation is kept
    /// explicit instead of being hidden behind an atomicity claim.
    pub async fn save_text_document(
        &self,
        path: impl Into<String>,
        expected_revision: &str,
        content: &str,
    ) -> Result<RemoteTextDocument, SshError> {
        if content.len() > MAX_REMOTE_EDITOR_BYTES {
            return Err(SshError::RemoteFileTooLarge);
        }
        let path = path.into();
        let current = self.read_text_document(path.clone()).await?;
        if current.revision != expected_revision {
            return Err(SshError::RemoteConflict);
        }
        let temporary = format!(
            "{path}.mobarust-edit-{}-{}",
            std::process::id(),
            next_editor_temp_id()
        );
        let mut file = self
            .session
            .create(&temporary)
            .await
            .map_err(|error| SshError::Sftp(error.to_string()))?;
        let write_result = file
            .write_all(content.as_bytes())
            .await
            .map_err(|error| SshError::Sftp(error.to_string()));
        let close_result = file
            .shutdown()
            .await
            .map_err(|error| SshError::Sftp(error.to_string()));
        if let Err(error) = write_result {
            let _ = self.session.remove_file(&temporary).await;
            return Err(error);
        }
        if let Err(error) = close_result {
            let _ = self.session.remove_file(&temporary).await;
            return Err(error);
        }
        if let Some(permissions) = current.permissions {
            let mut metadata = russh_sftp::client::fs::Metadata::empty();
            metadata.permissions = Some(permissions);
            let file = match self.session.open(&temporary).await {
                Ok(file) => file,
                Err(error) => {
                    let _ = self.session.remove_file(&temporary).await;
                    return Err(SshError::Sftp(error.to_string()));
                }
            };
            let set_result = file
                .set_metadata(metadata)
                .await
                .map_err(|error| SshError::Sftp(error.to_string()));
            let close_result = file
                .close()
                .await
                .map_err(|error| SshError::Sftp(error.to_string()));
            if let Err(error) = set_result {
                let _ = self.session.remove_file(&temporary).await;
                return Err(error);
            }
            if let Err(error) = close_result {
                let _ = self.session.remove_file(&temporary).await;
                return Err(error);
            }
        }

        // Some SFTP servers reject rename-over-existing. Move the original to
        // a unique rollback name first, then promote the complete temporary
        // file. If promotion fails, restore the original before returning.
        let backup = format!(
            "{path}.mobarust-edit-backup-{}-{}",
            std::process::id(),
            next_editor_temp_id()
        );
        if let Err(error) = self.session.rename(&path, &backup).await {
            let _ = self.session.remove_file(&temporary).await;
            return Err(SshError::Sftp(error.to_string()));
        }
        if let Err(error) = self.session.rename(&temporary, &path).await {
            let _ = self.session.rename(&backup, &path).await;
            let _ = self.session.remove_file(&temporary).await;
            return Err(SshError::Sftp(error.to_string()));
        }
        if let Err(error) = self.session.remove_file(&backup).await {
            return Err(SshError::Sftp(format!(
                "remote file saved but backup cleanup failed: {error}"
            )));
        }
        self.read_text_document(path).await
    }

    pub async fn try_exists(&self, path: impl Into<String>) -> Result<bool, SshError> {
        self.session
            .try_exists(path)
            .await
            .map_err(|error| SshError::Sftp(error.to_string()))
    }

    pub async fn create_dir(&self, path: impl Into<String>) -> Result<(), SshError> {
        self.session
            .create_dir(path)
            .await
            .map_err(|error| SshError::Sftp(error.to_string()))
    }

    pub async fn remove_dir(&self, path: impl Into<String>) -> Result<(), SshError> {
        self.session
            .remove_dir(path)
            .await
            .map_err(|error| SshError::Sftp(error.to_string()))
    }

    pub async fn rename(
        &self,
        old_path: impl Into<String>,
        new_path: impl Into<String>,
    ) -> Result<(), SshError> {
        self.session
            .rename(old_path, new_path)
            .await
            .map_err(|error| SshError::Sftp(error.to_string()))
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

    pub async fn download_to_with_cancel<W, F>(
        &self,
        remote_path: impl Into<String>,
        destination: &mut W,
        cancel: &mut oneshot::Receiver<()>,
        on_progress: F,
    ) -> Result<u64, SshError>
    where
        W: AsyncWrite + Unpin,
        F: FnMut(u64),
    {
        let mut file = self
            .session
            .open(remote_path)
            .await
            .map_err(|error| SshError::Sftp(error.to_string()))?;
        let copied = copy_with_cancel(&mut file, destination, cancel, on_progress).await?;
        file.close()
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

    pub async fn upload_from_with_cancel<R, F>(
        &self,
        source: &mut R,
        remote_path: impl Into<String>,
        cancel: &mut oneshot::Receiver<()>,
        on_progress: F,
    ) -> Result<u64, SshError>
    where
        R: AsyncRead + Unpin,
        F: FnMut(u64),
    {
        let mut file = self
            .session
            .create(remote_path)
            .await
            .map_err(|error| SshError::Sftp(error.to_string()))?;
        let copied = copy_with_cancel(source, &mut file, cancel, on_progress).await?;
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

fn text_revision(content: &[u8]) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(content);
    let mut revision = String::from("sha256:");
    for byte in digest {
        let _ = write!(revision, "{byte:02x}");
    }
    revision
}

fn next_editor_temp_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

async fn copy_with_cancel<R, W, F>(
    source: &mut R,
    destination: &mut W,
    cancel: &mut oneshot::Receiver<()>,
    mut on_progress: F,
) -> Result<u64, SshError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    F: FnMut(u64),
{
    const BUFFER_SIZE: usize = 64 * 1024;
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    let mut copied = 0_u64;

    loop {
        let read = tokio::select! {
            _ = &mut *cancel => return Err(SshError::Cancelled),
            result = source.read(&mut buffer) => result.map_err(SshError::LocalIo)?,
        };
        if read == 0 {
            break;
        }

        tokio::select! {
            _ = &mut *cancel => return Err(SshError::Cancelled),
            result = destination.write_all(&buffer[..read]) => result.map_err(SshError::LocalIo)?,
        }
        copied += read as u64;
        on_progress(copied);
        if cancel.try_recv().is_ok() {
            return Err(SshError::Cancelled);
        }
    }

    tokio::select! {
        _ = &mut *cancel => Err(SshError::Cancelled),
        result = destination.flush() => result.map(|_| copied).map_err(SshError::LocalIo),
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

fn parse_remote_monitor_snapshot(stdout: &[u8]) -> Result<RemoteMonitorSnapshot, SshError> {
    let text = String::from_utf8_lossy(stdout);
    let mut hostname = None;
    let mut kernel = None;
    let mut uptime_seconds = None;
    let mut load_average = None;
    let mut memory_total_bytes = None;
    let mut memory_available_bytes = None;
    let mut root_disk_used_percent = None;
    let mut process_count = None;

    for line in text.lines() {
        let Some((key, raw_value)) = line
            .strip_prefix("__MOBARUST__")
            .and_then(|line| line.split_once('='))
        else {
            continue;
        };
        let value = raw_value.trim();
        if value.is_empty() {
            continue;
        }
        match key {
            "hostname" => hostname = Some(value.to_owned()),
            "kernel" => kernel = Some(value.to_owned()),
            "uptime_seconds" => uptime_seconds = value.parse().ok(),
            "load" => {
                let values = value
                    .split(',')
                    .map(str::parse::<f64>)
                    .collect::<Result<Vec<_>, _>>()
                    .ok();
                if let Some(values) = values
                    && values.len() == 3
                    && values.iter().all(|number| number.is_finite())
                {
                    load_average = Some([values[0], values[1], values[2]]);
                }
            }
            "mem_total_kib" => {
                memory_total_bytes = value
                    .parse::<u64>()
                    .ok()
                    .and_then(|kib| kib.checked_mul(1024));
            }
            "mem_available_kib" => {
                memory_available_bytes = value
                    .parse::<u64>()
                    .ok()
                    .and_then(|kib| kib.checked_mul(1024));
            }
            "mem_total_bytes" => memory_total_bytes = value.parse().ok(),
            "disk_root_used_percent" => {
                root_disk_used_percent = value.parse::<u8>().ok().filter(|percent| *percent <= 100);
            }
            "process_count" => process_count = value.parse().ok(),
            _ => {}
        }
    }

    let mut supported_metrics = Vec::new();
    if hostname.is_some() {
        supported_metrics.push("hostname".to_owned());
    }
    if kernel.is_some() {
        supported_metrics.push("kernel".to_owned());
    }
    if uptime_seconds.is_some() {
        supported_metrics.push("uptime".to_owned());
    }
    if load_average.is_some() {
        supported_metrics.push("load".to_owned());
    }
    if memory_total_bytes.is_some() {
        supported_metrics.push("memory".to_owned());
    }
    if memory_available_bytes.is_some() {
        supported_metrics.push("memory-available".to_owned());
    }
    if root_disk_used_percent.is_some() {
        supported_metrics.push("disk".to_owned());
    }
    if process_count.is_some() {
        supported_metrics.push("processes".to_owned());
    }
    if supported_metrics.is_empty() {
        return Err(SshError::RemoteMonitorUnsupported);
    }

    Ok(RemoteMonitorSnapshot {
        hostname,
        kernel,
        uptime_seconds,
        load_average,
        memory_total_bytes,
        memory_available_bytes,
        root_disk_used_percent,
        process_count,
        supported_metrics,
    })
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

    #[test]
    fn socks5_domain_connect_handshake_is_bounded_and_typed() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (mut client, mut proxy) = tokio::io::duplex(4096);
            let proxy_task = tokio::spawn(async move { negotiate_socks5(&mut proxy).await });
            client.write_all(&[5, 1, 0]).await.unwrap();
            let mut method_reply = [0_u8; 2];
            client.read_exact(&mut method_reply).await.unwrap();
            assert_eq!(method_reply, [5, 0]);
            client.write_all(&[5, 1, 0, 3, 13]).await.unwrap();
            client.write_all(b"fixture.local").await.unwrap();
            client.write_all(&443_u16.to_be_bytes()).await.unwrap();
            let request = proxy_task.await.unwrap().unwrap();
            assert_eq!(
                request,
                Socks5Request {
                    target_host: "fixture.local".into(),
                    target_port: 443,
                }
            );
        });
    }

    #[test]
    fn socks5_rejects_authentication_instead_of_falling_back() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (mut client, mut proxy) = tokio::io::duplex(128);
            let proxy_task = tokio::spawn(async move { negotiate_socks5(&mut proxy).await });
            client.write_all(&[5, 1, 2]).await.unwrap();
            let mut method_reply = [0_u8; 2];
            client.read_exact(&mut method_reply).await.unwrap();
            assert_eq!(method_reply, [5, 0xff]);
            assert!(matches!(
                proxy_task.await.unwrap(),
                Err(Socks5Error::NoUnauthenticatedMethod)
            ));
        });
    }

    #[test]
    fn remote_text_revisions_are_stable_and_content_bound() {
        let first = text_revision(b"line one\n");
        let second = text_revision(b"line one\n");
        let changed = text_revision(b"line two\n");
        assert_eq!(first, second);
        assert_ne!(first, changed);
        assert!(first.starts_with("sha256:"));
    }

    #[test]
    fn remote_monitor_parser_keeps_optional_metrics_and_converts_memory() {
        let snapshot = parse_remote_monitor_snapshot(
            b"noise from a login banner\n__MOBARUST__hostname=fixture-sshd\n__MOBARUST__kernel=Linux 6.1\n__MOBARUST__uptime_seconds=42\n__MOBARUST__load=0.10,0.20,0.30\n__MOBARUST__mem_total_kib=2048\n__MOBARUST__mem_available_kib=1024\n__MOBARUST__disk_root_used_percent=37\n__MOBARUST__process_count=9\n",
        )
        .unwrap();

        assert_eq!(snapshot.hostname.as_deref(), Some("fixture-sshd"));
        assert_eq!(snapshot.uptime_seconds, Some(42));
        assert_eq!(snapshot.memory_total_bytes, Some(2 * 1024 * 1024));
        assert_eq!(snapshot.memory_available_bytes, Some(1024 * 1024));
        assert_eq!(snapshot.root_disk_used_percent, Some(37));
        assert_eq!(snapshot.process_count, Some(9));
        assert_eq!(snapshot.load_average, Some([0.10, 0.20, 0.30]));
        assert!(snapshot.supported_metrics.contains(&"memory".to_owned()));
    }

    #[test]
    fn remote_monitor_parser_rejects_an_empty_or_invalid_snapshot() {
        assert!(matches!(
            parse_remote_monitor_snapshot(b"login banner only\n"),
            Err(SshError::RemoteMonitorUnsupported)
        ));
        assert!(matches!(
            parse_remote_monitor_snapshot(b"__MOBARUST__load=not-a-number\n"),
            Err(SshError::RemoteMonitorUnsupported)
        ));
    }

    fn accepted_fingerprint(policy: &HostKeyPolicy, fingerprint: &str) -> Option<bool> {
        match policy {
            HostKeyPolicy::PinnedFingerprint(expected) => Some(expected == fingerprint),
            HostKeyPolicy::KnownHosts(_) => None,
        }
    }
}
