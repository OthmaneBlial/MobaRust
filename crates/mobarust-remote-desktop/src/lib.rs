//! Safe, versioned contracts for isolated RDP/VNC helper processes.
//!
//! This crate deliberately does not implement either remote-desktop protocol.
//! It constrains the boundary that a future FreeRDP/libvncclient helper must
//! satisfy: bounded frames, typed input, explicit lifecycle, and no secret
//! material in process arguments or diagnostic formatting.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::time::timeout;
use zeroize::Zeroizing;

pub const WIRE_VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;
pub const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;
pub const MAX_CREDENTIAL_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireEnvelope<T> {
    pub version: u16,
    pub payload: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DesktopProtocol {
    Rdp,
    Vnc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplaySize {
    pub width: u16,
    pub height: u16,
}

impl DisplaySize {
    pub fn validate(self) -> Result<(), HelperProtocolError> {
        if !(320..=16_384).contains(&self.width) || !(200..=16_384).contains(&self.height) {
            return Err(HelperProtocolError::InvalidDisplaySize {
                width: self.width,
                height: self.height,
            });
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperLaunchConfig {
    pub program: PathBuf,
    pub protocol: DesktopProtocol,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub domain: Option<String>,
    pub display: DisplaySize,
    pub color_depth: u16,
    pub audio_enabled: bool,
    /// Opaque vault identifier. The secret value is never part of this type.
    pub credential_ref: String,
}

impl fmt::Debug for HelperLaunchConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HelperLaunchConfig")
            .field("program", &self.program)
            .field("protocol", &self.protocol)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("domain", &self.domain)
            .field("display", &self.display)
            .field("color_depth", &self.color_depth)
            .field("audio_enabled", &self.audio_enabled)
            .field("credential_ref", &"<opaque-reference>")
            .finish()
    }
}

/// A password delivered only over the native helper pipe.
///
/// This type is deliberately separate from [`HelperCommand`]. It must not be
/// serialized with ordinary control messages or placed in process arguments.
/// The helper consumes it once to build the protocol client's native config.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperCredential {
    password: Zeroizing<String>,
}

impl HelperCredential {
    pub fn new(password: impl Into<String>) -> Self {
        Self {
            password: Zeroizing::new(password.into()),
        }
    }

    pub fn password(&self) -> &str {
        self.password.as_str()
    }

    pub fn validate(&self) -> Result<(), HelperProtocolError> {
        if self.password.len() > MAX_CREDENTIAL_BYTES {
            return Err(HelperProtocolError::CredentialTooLarge {
                bytes: self.password.len(),
            });
        }
        Ok(())
    }
}

impl fmt::Debug for HelperCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HelperCredential(<redacted>)")
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CredentialEnvelope {
    version: u16,
    credential: HelperCredential,
}

impl HelperLaunchConfig {
    pub fn validate(&self) -> Result<(), HelperProtocolError> {
        if self.program.as_os_str().is_empty() {
            return Err(HelperProtocolError::EmptyProgram);
        }
        if self.host.trim().is_empty() {
            return Err(HelperProtocolError::EmptyHost);
        }
        if self.port == 0 {
            return Err(HelperProtocolError::InvalidPort);
        }
        if self.username.trim().is_empty() {
            return Err(HelperProtocolError::EmptyUsername);
        }
        if self.credential_ref.trim().is_empty() && self.protocol == DesktopProtocol::Rdp {
            return Err(HelperProtocolError::EmptyCredentialReference);
        }
        if self.color_depth == 0 {
            return Err(HelperProtocolError::InvalidColorDepth);
        }
        self.display.validate()
    }

    /// Returns only non-secret process arguments. Credentials are handed over
    /// through a separate native channel after the helper has started.
    pub fn process_arguments(&self) -> Vec<String> {
        let mut arguments = vec![
            "--mobarust-protocol".into(),
            format!("{:?}", self.protocol).to_lowercase(),
            "--host".into(),
            self.host.clone(),
            "--port".into(),
            self.port.to_string(),
            "--username".into(),
            self.username.clone(),
            "--width".into(),
            self.display.width.to_string(),
            "--height".into(),
            self.display.height.to_string(),
            "--color-depth".into(),
            self.color_depth.to_string(),
        ];
        if let Some(domain) = self.domain.as_deref().filter(|value| !value.is_empty()) {
            arguments.extend(["--domain".into(), domain.into()]);
        }
        if self.audio_enabled {
            arguments.push("--audio".into());
        }
        arguments
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", content = "payload", rename_all = "camelCase")]
pub enum HelperCommand {
    Start {
        protocol: DesktopProtocol,
        display: DisplaySize,
    },
    Stop,
    Resize {
        display: DisplaySize,
    },
    Key {
        scancode: u32,
        pressed: bool,
    },
    Pointer {
        x: u16,
        y: u16,
        buttons: u8,
    },
    Clipboard {
        text: Zeroizing<String>,
    },
}

impl fmt::Debug for HelperCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Start { protocol, display } => formatter
                .debug_struct("Start")
                .field("protocol", protocol)
                .field("display", display)
                .finish(),
            Self::Stop => formatter.write_str("Stop"),
            Self::Resize { display } => formatter
                .debug_struct("Resize")
                .field("display", display)
                .finish(),
            Self::Key { scancode, pressed } => formatter
                .debug_struct("Key")
                .field("scancode", scancode)
                .field("pressed", pressed)
                .finish(),
            Self::Pointer { x, y, buttons } => formatter
                .debug_struct("Pointer")
                .field("x", x)
                .field("y", y)
                .field("buttons", buttons)
                .finish(),
            Self::Clipboard { text } => formatter
                .debug_struct("Clipboard")
                .field("bytes", &text.len())
                .finish(),
        }
    }
}

impl HelperCommand {
    pub fn validate(&self) -> Result<(), HelperProtocolError> {
        match self {
            Self::Start { display, .. } | Self::Resize { display } => display.validate(),
            Self::Clipboard { text } if text.len() > MAX_CLIPBOARD_BYTES => {
                Err(HelperProtocolError::ClipboardTooLarge { bytes: text.len() })
            }
            _ => Ok(()),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", content = "payload", rename_all = "camelCase")]
pub enum HelperEvent {
    Hello {
        version: u16,
    },
    State {
        state: HelperState,
    },
    Framebuffer {
        width: u16,
        height: u16,
        pixels: Vec<u8>,
    },
    Clipboard {
        text: Zeroizing<String>,
    },
    Diagnostic {
        level: DiagnosticLevel,
        message: String,
    },
}

impl fmt::Debug for HelperEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hello { version } => formatter
                .debug_struct("Hello")
                .field("version", version)
                .finish(),
            Self::State { state } => formatter
                .debug_struct("State")
                .field("state", state)
                .finish(),
            Self::Framebuffer {
                width,
                height,
                pixels,
            } => formatter
                .debug_struct("Framebuffer")
                .field("width", width)
                .field("height", height)
                .field("bytes", &pixels.len())
                .finish(),
            Self::Clipboard { text } => formatter
                .debug_struct("Clipboard")
                .field("bytes", &text.len())
                .finish(),
            Self::Diagnostic { level, message } => formatter
                .debug_struct("Diagnostic")
                .field("level", level)
                .field("message_bytes", &message.len())
                .finish(),
        }
    }
}

impl HelperEvent {
    pub fn validate(&self) -> Result<(), HelperProtocolError> {
        match self {
            Self::Hello { version } if *version != WIRE_VERSION => {
                Err(HelperProtocolError::UnsupportedVersion(*version))
            }
            Self::Framebuffer {
                width,
                height,
                pixels,
            } => {
                DisplaySize {
                    width: *width,
                    height: *height,
                }
                .validate()?;
                let expected = usize::from(*width)
                    .saturating_mul(usize::from(*height))
                    .saturating_mul(4);
                if pixels.len() != expected {
                    return Err(HelperProtocolError::InvalidFramebuffer {
                        expected,
                        actual: pixels.len(),
                    });
                }
                if pixels.len() > MAX_FRAME_BYTES {
                    return Err(HelperProtocolError::FrameTooLarge {
                        bytes: pixels.len(),
                    });
                }
                Ok(())
            }
            Self::Clipboard { text } if text.len() > MAX_CLIPBOARD_BYTES => {
                Err(HelperProtocolError::ClipboardTooLarge { bytes: text.len() })
            }
            Self::Diagnostic { message, .. } if message.len() > MAX_DIAGNOSTIC_BYTES => {
                Err(HelperProtocolError::DiagnosticTooLarge {
                    bytes: message.len(),
                })
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HelperState {
    Created,
    Starting,
    Ready,
    Active,
    Reconnecting,
    Stopping,
    Stopped,
    Crashed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperLifecycleEvent {
    StartRequested,
    Ready,
    Activate,
    ConnectionLost,
    BeginReconnect,
    StopRequested,
    Stopped,
    Crashed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperLifecycle {
    state: HelperState,
    revision: u64,
}

impl Default for HelperLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl HelperLifecycle {
    pub const fn new() -> Self {
        Self {
            state: HelperState::Created,
            revision: 0,
        }
    }

    pub const fn state(&self) -> HelperState {
        self.state
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub fn apply(
        &mut self,
        event: HelperLifecycleEvent,
    ) -> Result<HelperState, HelperProtocolError> {
        let next = match (self.state, event) {
            (HelperState::Created, HelperLifecycleEvent::StartRequested) => HelperState::Starting,
            (HelperState::Starting, HelperLifecycleEvent::Ready) => HelperState::Ready,
            (HelperState::Ready, HelperLifecycleEvent::Activate) => HelperState::Active,
            (HelperState::Active, HelperLifecycleEvent::ConnectionLost) => {
                HelperState::Reconnecting
            }
            (HelperState::Reconnecting, HelperLifecycleEvent::BeginReconnect) => {
                HelperState::Starting
            }
            (
                HelperState::Starting
                | HelperState::Ready
                | HelperState::Active
                | HelperState::Reconnecting,
                HelperLifecycleEvent::StopRequested,
            ) => HelperState::Stopping,
            (HelperState::Stopping, HelperLifecycleEvent::Stopped) => HelperState::Stopped,
            (
                HelperState::Starting
                | HelperState::Ready
                | HelperState::Active
                | HelperState::Reconnecting,
                HelperLifecycleEvent::Crashed,
            ) => HelperState::Crashed,
            (
                HelperState::Starting
                | HelperState::Ready
                | HelperState::Active
                | HelperState::Reconnecting,
                HelperLifecycleEvent::Failed,
            ) => HelperState::Failed,
            _ => {
                return Err(HelperProtocolError::InvalidTransition {
                    state: self.state,
                    event,
                });
            }
        };
        self.state = next;
        self.revision = self.revision.saturating_add(1);
        Ok(next)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum HelperProtocolError {
    #[error("helper program path is empty")]
    EmptyProgram,
    #[error("helper host is empty")]
    EmptyHost,
    #[error("helper port must be non-zero")]
    InvalidPort,
    #[error("helper username is empty")]
    EmptyUsername,
    #[error("helper credential reference is empty")]
    EmptyCredentialReference,
    #[error("helper color depth must be non-zero")]
    InvalidColorDepth,
    #[error("invalid display size {width}x{height}")]
    InvalidDisplaySize { width: u16, height: u16 },
    #[error("clipboard payload is too large: {bytes} bytes")]
    ClipboardTooLarge { bytes: usize },
    #[error("diagnostic payload is too large: {bytes} bytes")]
    DiagnosticTooLarge { bytes: usize },
    #[error("credential payload is too large: {bytes} bytes")]
    CredentialTooLarge { bytes: usize },
    #[error("invalid framebuffer payload: expected {expected} bytes, received {actual}")]
    InvalidFramebuffer { expected: usize, actual: usize },
    #[error("frame is too large: {bytes} bytes")]
    FrameTooLarge { bytes: usize },
    #[error("frame is truncated: expected {expected} bytes, received {actual}")]
    TruncatedFrame { expected: usize, actual: usize },
    #[error("frame contains invalid JSON: {0}")]
    InvalidJson(String),
    #[error("unsupported helper wire version {0}")]
    UnsupportedVersion(u16),
    #[error("invalid lifecycle transition from {state:?} with {event:?}")]
    InvalidTransition {
        state: HelperState,
        event: HelperLifecycleEvent,
    },
    #[error("helper I/O failed: {0}")]
    Io(String),
}

/// Encodes one length-prefixed JSON frame for a helper's native pipe.
pub fn encode_frame<T: Serialize>(message: &T) -> Result<Vec<u8>, HelperProtocolError> {
    let mut frame = encode_frame_zeroizing(message)?;
    Ok(std::mem::take(&mut *frame))
}

fn encode_frame_zeroizing<T: Serialize>(
    message: &T,
) -> Result<Zeroizing<Vec<u8>>, HelperProtocolError> {
    let body = Zeroizing::new(
        serde_json::to_vec(message)
            .map_err(|error| HelperProtocolError::InvalidJson(error.to_string()))?,
    );
    if body.len() > MAX_FRAME_BYTES {
        return Err(HelperProtocolError::FrameTooLarge { bytes: body.len() });
    }
    let length = u32::try_from(body.len())
        .map_err(|_| HelperProtocolError::FrameTooLarge { bytes: body.len() })?;
    let mut frame = Zeroizing::new(Vec::with_capacity(4 + body.len()));
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

/// Decodes exactly one length-prefixed JSON frame. Stream buffering and child
/// process I/O remain outside this crate so the boundary can be tested without
/// spawning a process or touching the host system.
pub fn decode_frame<T: DeserializeOwned>(frame: &[u8]) -> Result<T, HelperProtocolError> {
    if frame.len() < 4 {
        return Err(HelperProtocolError::TruncatedFrame {
            expected: 4,
            actual: frame.len(),
        });
    }
    let length = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
    if length > MAX_FRAME_BYTES {
        return Err(HelperProtocolError::FrameTooLarge { bytes: length });
    }
    let expected = 4 + length;
    if frame.len() != expected {
        return Err(HelperProtocolError::TruncatedFrame {
            expected,
            actual: frame.len(),
        });
    }
    serde_json::from_slice(&frame[4..])
        .map_err(|error| HelperProtocolError::InvalidJson(error.to_string()))
}

pub fn encode_command_frame(
    command: &HelperCommand,
) -> Result<Zeroizing<Vec<u8>>, HelperProtocolError> {
    command.validate()?;
    encode_frame_zeroizing(&WireEnvelope {
        version: WIRE_VERSION,
        payload: command,
    })
}

pub fn decode_command_frame(frame: &[u8]) -> Result<HelperCommand, HelperProtocolError> {
    let envelope: WireEnvelope<HelperCommand> = decode_frame(frame)?;
    if envelope.version != WIRE_VERSION {
        return Err(HelperProtocolError::UnsupportedVersion(envelope.version));
    }
    envelope.payload.validate()?;
    Ok(envelope.payload)
}

/// Encodes the one native credential handoff frame. The returned buffer is
/// zeroizing and should be written directly to the helper pipe, then dropped.
pub fn encode_credential_frame(
    credential: &HelperCredential,
) -> Result<Zeroizing<Vec<u8>>, HelperProtocolError> {
    credential.validate()?;

    encode_frame_zeroizing(&CredentialEnvelope {
        version: WIRE_VERSION,
        credential: credential.clone(),
    })
}

pub fn decode_credential_frame(frame: &[u8]) -> Result<HelperCredential, HelperProtocolError> {
    let envelope: CredentialEnvelope = decode_frame(frame)?;
    if envelope.version != WIRE_VERSION {
        return Err(HelperProtocolError::UnsupportedVersion(envelope.version));
    }
    envelope.credential.validate()?;
    Ok(envelope.credential)
}

/// Reads exactly one length-prefixed frame from a native pipe. The complete
/// frame is zeroized when the returned value is dropped, which matters because
/// a credential frame contains a serialized password.
pub async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<Option<Zeroizing<Vec<u8>>>, HelperProtocolError> {
    let mut length_bytes = [0u8; 4];
    match reader.read_exact(&mut length_bytes).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(HelperProtocolError::Io(error.to_string())),
    }

    let body_length = u32::from_be_bytes(length_bytes) as usize;
    if body_length > MAX_FRAME_BYTES {
        return Err(HelperProtocolError::FrameTooLarge { bytes: body_length });
    }

    let mut frame = Zeroizing::new(vec![0u8; body_length.saturating_add(4)]);
    frame[..4].copy_from_slice(&length_bytes);
    reader
        .read_exact(&mut frame[4..])
        .await
        .map_err(|error| HelperProtocolError::Io(error.to_string()))?;
    Ok(Some(frame))
}

/// Writes an event using a zeroizing temporary buffer. This should be used for
/// all helper output because clipboard events may contain sensitive text.
pub async fn write_event_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    event: &HelperEvent,
) -> Result<(), HelperProtocolError> {
    let frame = encode_event_frame(event)?;
    writer
        .write_all(&frame[..])
        .await
        .map_err(|error| HelperProtocolError::Io(error.to_string()))?;
    writer
        .flush()
        .await
        .map_err(|error| HelperProtocolError::Io(error.to_string()))
}

/// Writes the credential handoff and guarantees that the serialized frame is
/// zeroized after the pipe write completes.
pub async fn write_credential_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    credential: &HelperCredential,
) -> Result<(), HelperProtocolError> {
    let frame = encode_credential_frame(credential)?;
    writer
        .write_all(&frame[..])
        .await
        .map_err(|error| HelperProtocolError::Io(error.to_string()))?;
    writer
        .flush()
        .await
        .map_err(|error| HelperProtocolError::Io(error.to_string()))
}

pub fn encode_event_frame(event: &HelperEvent) -> Result<Zeroizing<Vec<u8>>, HelperProtocolError> {
    event.validate()?;
    encode_frame_zeroizing(&WireEnvelope {
        version: WIRE_VERSION,
        payload: event,
    })
}

pub fn decode_event_frame(frame: &[u8]) -> Result<HelperEvent, HelperProtocolError> {
    let envelope: WireEnvelope<HelperEvent> = decode_frame(frame)?;
    if envelope.version != WIRE_VERSION {
        return Err(HelperProtocolError::UnsupportedVersion(envelope.version));
    }
    envelope.payload.validate()?;
    Ok(envelope.payload)
}

#[derive(Debug, Error)]
pub enum HelperProcessError {
    #[error(transparent)]
    Protocol(#[from] HelperProtocolError),
    #[error("could not start remote desktop helper: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("remote desktop helper did not stop within the grace period")]
    GracePeriodExpired,
    #[error("could not stop remote desktop helper: {0}")]
    Terminate(#[source] std::io::Error),
    #[error("could not wait for remote desktop helper: {0}")]
    Wait(#[source] std::io::Error),
    #[error("remote desktop helper stdin is unavailable")]
    StdinUnavailable,
    #[error("remote desktop helper stdout is unavailable")]
    StdoutUnavailable,
}

/// Owns one isolated helper process. The supervisor intentionally exposes only
/// native pipes; callers are responsible for framing and for keeping secrets
/// on a separate, explicitly controlled credential channel.
pub struct HelperSupervisor {
    child: Child,
}

impl HelperSupervisor {
    pub fn spawn(config: &HelperLaunchConfig) -> Result<Self, HelperProcessError> {
        config.validate()?;
        let mut command = helper_command(config);
        let child = command.spawn().map_err(HelperProcessError::Spawn)?;
        Ok(Self { child })
    }

    pub fn stdin(&mut self) -> Option<&mut ChildStdin> {
        self.child.stdin.as_mut()
    }

    pub fn stdout(&mut self) -> Option<&mut ChildStdout> {
        self.child.stdout.as_mut()
    }

    /// Transfers the native stdin pipe to a dedicated writer task. The child
    /// process remains owned by this supervisor, so `stop` can still perform
    /// bounded graceful shutdown and forced reaping.
    pub fn take_stdin(&mut self) -> Result<ChildStdin, HelperProcessError> {
        self.child
            .stdin
            .take()
            .ok_or(HelperProcessError::StdinUnavailable)
    }

    /// Transfers the native stdout pipe to a dedicated reader task. Keeping
    /// the pipe separate from the child handle lets input and framebuffer
    /// events flow concurrently without a mutex held across a blocking read.
    pub fn take_stdout(&mut self) -> Result<ChildStdout, HelperProcessError> {
        self.child
            .stdout
            .take()
            .ok_or(HelperProcessError::StdoutUnavailable)
    }

    pub fn try_exit_status(&mut self) -> Result<Option<ExitStatus>, HelperProcessError> {
        self.child.try_wait().map_err(HelperProcessError::Wait)
    }

    /// Sends a typed control message without exposing the native pipe to
    /// callers. Serialized clipboard text is held in a zeroizing frame.
    pub async fn send_command(
        &mut self,
        command: &HelperCommand,
    ) -> Result<(), HelperProcessError> {
        let stdin = self.stdin().ok_or(HelperProcessError::StdinUnavailable)?;
        let frame = encode_command_frame(command)?;
        stdin.write_all(&frame[..]).await.map_err(|error| {
            HelperProcessError::Protocol(HelperProtocolError::Io(error.to_string()))
        })?;
        stdin.flush().await.map_err(|error| {
            HelperProcessError::Protocol(HelperProtocolError::Io(error.to_string()))
        })
    }

    /// Sends a password through the dedicated zeroizing native channel. The
    /// password is never part of a `HelperCommand` or process argument.
    pub async fn send_credentials(
        &mut self,
        credential: &HelperCredential,
    ) -> Result<(), HelperProcessError> {
        let stdin = self.stdin().ok_or(HelperProcessError::StdinUnavailable)?;
        write_credential_frame(stdin, credential)
            .await
            .map_err(HelperProcessError::Protocol)
    }

    /// Reads one typed event from the helper. The incoming serialized frame is
    /// zeroized immediately after decoding.
    pub async fn read_event(&mut self) -> Result<Option<HelperEvent>, HelperProcessError> {
        let stdout = self.stdout().ok_or(HelperProcessError::StdoutUnavailable)?;
        let Some(frame) = read_frame(stdout)
            .await
            .map_err(HelperProcessError::Protocol)?
        else {
            return Ok(None);
        };
        decode_event_frame(&frame)
            .map(Some)
            .map_err(HelperProcessError::Protocol)
    }

    /// Gives the helper a bounded chance to exit cleanly, then kills it and
    /// waits for reaping. No process is left running after this returns Ok.
    pub async fn stop(&mut self, grace_period: Duration) -> Result<ExitStatus, HelperProcessError> {
        if let Some(status) = self.child.try_wait().map_err(HelperProcessError::Wait)? {
            return Ok(status);
        }
        if let Ok(result) = timeout(grace_period, self.child.wait()).await {
            return result.map_err(HelperProcessError::Wait);
        }
        self.child
            .start_kill()
            .map_err(HelperProcessError::Terminate)?;
        self.child.wait().await.map_err(HelperProcessError::Wait)
    }
}

fn helper_command(config: &HelperLaunchConfig) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(&config.program);
    command
        .args(config.process_arguments())
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch_config() -> HelperLaunchConfig {
        HelperLaunchConfig {
            program: PathBuf::from("/opt/mobarust/bin/mobarust-rdp-helper"),
            protocol: DesktopProtocol::Rdp,
            host: "fixture.example".into(),
            port: 3389,
            username: "operator".into(),
            domain: Some("LAB".into()),
            display: DisplaySize {
                width: 1280,
                height: 800,
            },
            color_depth: 32,
            audio_enabled: false,
            credential_ref: "credential:test-only".into(),
        }
    }

    #[test]
    fn launch_arguments_never_contain_credential_material_or_reference() {
        let config = launch_config();
        let arguments = config.process_arguments();
        assert!(
            !arguments
                .iter()
                .any(|argument| argument.contains("credential:test-only"))
        );
        assert!(
            !arguments
                .iter()
                .any(|argument| argument.contains("password"))
        );
        assert!(arguments.contains(&"--host".into()));
        assert!(arguments.contains(&"--domain".into()));
    }

    #[test]
    fn debug_output_redacts_credential_reference_and_clipboard_content() {
        let config = launch_config();
        let credential = HelperCredential::new("password-secret");
        let command = HelperCommand::Clipboard {
            text: "clipboard-secret".to_owned().into(),
        };
        let event = HelperEvent::Clipboard {
            text: "clipboard-secret".to_owned().into(),
        };
        assert!(!format!("{config:?}").contains("credential:test-only"));
        assert!(!format!("{command:?}").contains("clipboard-secret"));
        assert!(!format!("{event:?}").contains("clipboard-secret"));
        assert!(!format!("{credential:?}").contains("password-secret"));
        assert!(format!("{command:?}").contains("bytes"));
    }

    #[test]
    fn credential_frames_round_trip_without_becoming_control_commands() {
        let credential = HelperCredential::new("fixture-only-password");
        let frame = encode_credential_frame(&credential).unwrap();
        let decoded = decode_credential_frame(&frame).unwrap();
        assert_eq!(decoded.password(), "fixture-only-password");
        assert!(decode_command_frame(&frame).is_err());
        assert!(!format!("{credential:?}").contains("fixture-only-password"));
    }

    #[tokio::test]
    async fn native_pipe_helpers_round_trip_a_zeroizing_credential_frame() {
        let credential = HelperCredential::new("fixture-pipe-password");
        let (mut writer, mut reader) = tokio::io::duplex(4096);
        let write_task =
            tokio::spawn(async move { write_credential_frame(&mut writer, &credential).await });

        let frame = read_frame(&mut reader).await.unwrap().unwrap();
        let decoded = decode_credential_frame(&frame).unwrap();
        write_task.await.unwrap().unwrap();
        assert_eq!(decoded.password(), "fixture-pipe-password");
    }

    #[test]
    fn frames_round_trip_and_reject_truncation() {
        let command = HelperCommand::Resize {
            display: DisplaySize {
                width: 1440,
                height: 900,
            },
        };
        let frame = encode_command_frame(&command).unwrap();
        let decoded = decode_command_frame(&frame).unwrap();
        assert_eq!(decoded, command);
        assert!(matches!(
            decode_command_frame(&frame[..frame.len() - 1]),
            Err(HelperProtocolError::TruncatedFrame { .. })
        ));
    }

    #[test]
    fn frames_reject_unknown_wire_versions() {
        let frame = encode_frame(&WireEnvelope {
            version: WIRE_VERSION + 1,
            payload: HelperCommand::Stop,
        })
        .unwrap();
        assert!(matches!(
            decode_command_frame(&frame),
            Err(HelperProtocolError::UnsupportedVersion(version)) if version == WIRE_VERSION + 1
        ));
    }

    #[test]
    fn validation_bounds_clipboard_and_display_payloads() {
        let oversized = HelperCommand::Clipboard {
            text: "x".repeat(MAX_CLIPBOARD_BYTES + 1).into(),
        };
        assert!(matches!(
            oversized.validate(),
            Err(HelperProtocolError::ClipboardTooLarge { .. })
        ));
        let invalid = DisplaySize {
            width: 10,
            height: 10,
        };
        assert!(matches!(
            invalid.validate(),
            Err(HelperProtocolError::InvalidDisplaySize { .. })
        ));
        let invalid_frame = HelperEvent::Framebuffer {
            width: 320,
            height: 200,
            pixels: vec![0; 4],
        };
        assert!(matches!(
            invalid_frame.validate(),
            Err(HelperProtocolError::InvalidFramebuffer { .. })
        ));
    }

    #[test]
    fn lifecycle_requires_ready_before_active_and_distinguishes_crash() {
        let mut lifecycle = HelperLifecycle::new();
        lifecycle
            .apply(HelperLifecycleEvent::StartRequested)
            .unwrap();
        lifecycle.apply(HelperLifecycleEvent::Ready).unwrap();
        lifecycle.apply(HelperLifecycleEvent::Activate).unwrap();
        lifecycle.apply(HelperLifecycleEvent::Crashed).unwrap();
        assert_eq!(lifecycle.state(), HelperState::Crashed);
        assert!(matches!(
            lifecycle.apply(HelperLifecycleEvent::Ready),
            Err(HelperProtocolError::InvalidTransition { .. })
        ));
    }
}
