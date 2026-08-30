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
pub const MAX_HOST_BYTES: usize = 255;
pub const MAX_USERNAME_BYTES: usize = 256;
pub const MAX_DOMAIN_BYTES: usize = 255;
pub const MAX_GATEWAY_ENDPOINT_BYTES: usize = 512;
pub const MAX_CREDENTIAL_REFERENCE_BYTES: usize = 128;
pub const DEFAULT_REMOTE_DESKTOP_RECONNECT_ENABLED: bool = true;
pub const DEFAULT_REMOTE_DESKTOP_RECONNECT_ATTEMPTS: u8 = 3;
pub const MAX_REMOTE_DESKTOP_RECONNECT_ATTEMPTS: u8 = 10;
pub const HELPER_PIPE_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
/// Bit used by the typed key command to mark an RDP extended scan code.
///
/// The low seven bits contain the set-1 scan code. Keeping this convention in
/// the shared contract prevents the frontend from sending browser/evdev-style
/// values that the RDP engine would interpret as a different key.
pub const RDP_EXTENDED_SCANCODE_MASK: u32 = 0x100;

pub fn rdp_scancode_parts(scancode: u32) -> Option<(u8, bool)> {
    if scancode & !0x17f != 0 {
        return None;
    }
    let code = scancode & 0x7f;
    (code != 0).then_some((code as u8, scancode & RDP_EXTENDED_SCANCODE_MASK != 0))
}

/// Validate the color depths supported by the pinned RDP candidate.
///
/// This is intentionally shared by the Tauri boundary, supervisor, and
/// helper parser so an unsupported depth cannot travel farther than the
/// earliest validation layer.
pub fn validate_rdp_color_depth(depth: u16) -> Result<(), HelperProtocolError> {
    if matches!(depth, 16 | 32) {
        Ok(())
    } else {
        Err(HelperProtocolError::UnsupportedRdpColorDepth { depth })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct DisplaySize {
    pub width: u16,
    pub height: u16,
}

/// Capabilities reported by the helper that is actually running.
///
/// The desktop UI may know the requested protocol, but only the native helper
/// knows which backend was compiled for the current platform. Reporting this
/// explicitly keeps unsupported operations visible instead of silently
/// treating a protocol feature flag as proof that a backend exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HelperCapabilities {
    pub protocol: DesktopProtocol,
    pub clipboard: bool,
    pub audio: bool,
    pub server_resize: bool,
    pub local_scaling: bool,
    pub gateway: bool,
    pub color_depths: Vec<u16>,
}

impl HelperCapabilities {
    pub fn rdp() -> Self {
        Self {
            protocol: DesktopProtocol::Rdp,
            clipboard: cfg!(windows),
            audio: false,
            server_resize: true,
            local_scaling: true,
            gateway: true,
            color_depths: vec![16, 32],
        }
    }

    pub fn vnc() -> Self {
        Self {
            protocol: DesktopProtocol::Vnc,
            clipboard: true,
            audio: false,
            server_resize: false,
            local_scaling: true,
            gateway: false,
            color_depths: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), HelperProtocolError> {
        if self.color_depths.len() > 8 {
            return Err(HelperProtocolError::CapabilitiesTooLarge {
                count: self.color_depths.len(),
            });
        }
        if let Some(&depth) = self.color_depths.iter().find(|depth| **depth == 0) {
            return Err(HelperProtocolError::InvalidCapabilityColorDepth { depth });
        }
        if self.protocol == DesktopProtocol::Rdp {
            for &depth in &self.color_depths {
                validate_rdp_color_depth(depth)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReconnectPolicy {
    pub enabled: bool,
    pub attempts: u8,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_REMOTE_DESKTOP_RECONNECT_ENABLED,
            attempts: DEFAULT_REMOTE_DESKTOP_RECONNECT_ATTEMPTS,
        }
    }
}

impl ReconnectPolicy {
    pub fn validate(self) -> Result<(), HelperProtocolError> {
        if self.attempts > MAX_REMOTE_DESKTOP_RECONNECT_ATTEMPTS {
            return Err(HelperProtocolError::InvalidReconnectAttempts {
                attempts: self.attempts,
            });
        }
        Ok(())
    }
}

fn default_remote_desktop_reconnect_enabled() -> bool {
    DEFAULT_REMOTE_DESKTOP_RECONNECT_ENABLED
}

fn default_remote_desktop_reconnect_attempts() -> u8 {
    DEFAULT_REMOTE_DESKTOP_RECONNECT_ATTEMPTS
}

impl DisplaySize {
    pub fn validate(self) -> Result<(), HelperProtocolError> {
        if !(320..=16_384).contains(&self.width) || !(200..=16_384).contains(&self.height) {
            return Err(HelperProtocolError::InvalidDisplaySize {
                width: self.width,
                height: self.height,
            });
        }
        let bytes = usize::from(self.width)
            .checked_mul(usize::from(self.height))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(HelperProtocolError::FrameTooLarge { bytes: usize::MAX })?;
        if bytes > MAX_FRAME_BYTES {
            return Err(HelperProtocolError::FrameTooLarge { bytes });
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HelperLaunchConfig {
    pub program: PathBuf,
    pub protocol: DesktopProtocol,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub domain: Option<String>,
    /// Explicit RDS gateway endpoint, including its port. Gateway credentials
    /// are still delivered through a separate native credential frame.
    #[serde(default)]
    pub gateway_endpoint: Option<String>,
    #[serde(default)]
    pub gateway_username: Option<String>,
    #[serde(default)]
    pub gateway_credential_ref: Option<String>,
    pub display: DisplaySize,
    pub color_depth: u16,
    pub audio_enabled: bool,
    /// Clipboard input is opt-in. RDP requires a reviewed native OS backend;
    /// VNC uses the helper's negotiated text channel when advertised.
    #[serde(default)]
    pub clipboard_enabled: bool,
    /// VNC encoding/refresh profile. Ignored by RDP helpers.
    pub vnc_quality: String,
    /// Opaque vault identifier. The secret value is never part of this type.
    pub credential_ref: String,
    #[serde(default = "default_remote_desktop_reconnect_enabled")]
    pub reconnect_enabled: bool,
    #[serde(default = "default_remote_desktop_reconnect_attempts")]
    pub reconnect_attempts: u8,
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
            .field("gateway_endpoint", &self.gateway_endpoint)
            .field("gateway_username", &self.gateway_username)
            .field("gateway_credential_ref", &"<opaque-reference>")
            .field("display", &self.display)
            .field("color_depth", &self.color_depth)
            .field("audio_enabled", &self.audio_enabled)
            .field("clipboard_enabled", &self.clipboard_enabled)
            .field("vnc_quality", &self.vnc_quality)
            .field("credential_ref", &"<opaque-reference>")
            .field("reconnect_enabled", &self.reconnect_enabled)
            .field("reconnect_attempts", &self.reconnect_attempts)
            .finish()
    }
}

/// Identifies which native protocol credential a helper frame carries.
///
/// Keeping the purpose on the frame prevents a gateway password from being
/// accidentally consumed as the target-session password when a connection
/// needs two independent authentication steps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HelperCredentialKind {
    #[default]
    Session,
    Gateway,
}

/// A password delivered only over the native helper pipe.
///
/// This type is deliberately separate from [`HelperCommand`]. It must not be
/// serialized with ordinary control messages or placed in process arguments.
/// The helper consumes it once to build the protocol client's native config.
#[derive(PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelperCredential {
    #[serde(default)]
    kind: HelperCredentialKind,
    password: Zeroizing<String>,
}

impl HelperCredential {
    pub fn new(password: impl Into<String>) -> Self {
        Self::with_kind(HelperCredentialKind::Session, password)
    }

    pub fn gateway(password: impl Into<String>) -> Self {
        Self::with_kind(HelperCredentialKind::Gateway, password)
    }

    fn with_kind(kind: HelperCredentialKind, password: impl Into<String>) -> Self {
        Self {
            kind,
            password: Zeroizing::new(password.into()),
        }
    }

    /// Adopt a zeroizing native buffer without cloning its plaintext.
    pub fn from_zeroizing(password: Zeroizing<String>) -> Self {
        Self::from_zeroizing_with_kind(HelperCredentialKind::Session, password)
    }

    /// Adopt a zeroizing native buffer for a specific native authentication
    /// role without cloning its plaintext.
    pub fn from_zeroizing_with_kind(
        kind: HelperCredentialKind,
        password: Zeroizing<String>,
    ) -> Self {
        Self { kind, password }
    }

    pub fn kind(&self) -> HelperCredentialKind {
        self.kind
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CredentialEnvelope {
    version: u16,
    credential: HelperCredential,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CredentialEnvelopeRef<'a> {
    version: u16,
    credential: &'a HelperCredential,
}

fn validate_metadata(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), HelperProtocolError> {
    if value.len() > max_bytes {
        return Err(HelperProtocolError::MetadataTooLarge {
            field,
            bytes: value.len(),
        });
    }
    if value.chars().any(char::is_control) {
        return Err(HelperProtocolError::MetadataContainsControl { field });
    }
    Ok(())
}

pub fn validate_gateway_endpoint(value: &str) -> Result<(), HelperProtocolError> {
    validate_metadata("gateway endpoint", value, MAX_GATEWAY_ENDPOINT_BYTES)?;
    let (host, port) = if let Some(rest) = value.strip_prefix('[') {
        let Some(close) = rest.find(']') else {
            return Err(HelperProtocolError::InvalidGatewayEndpoint);
        };
        let host = &rest[..close];
        let Some(port) = rest
            .get(close + 1..)
            .and_then(|value| value.strip_prefix(':'))
        else {
            return Err(HelperProtocolError::InvalidGatewayEndpoint);
        };
        (host, port)
    } else {
        let Some((host, port)) = value.rsplit_once(':') else {
            return Err(HelperProtocolError::InvalidGatewayEndpoint);
        };
        if host.contains(':') {
            return Err(HelperProtocolError::InvalidGatewayEndpoint);
        }
        (host, port)
    };
    if host.trim().is_empty() || port.parse::<u16>().ok().is_none_or(|port| port == 0) {
        return Err(HelperProtocolError::InvalidGatewayEndpoint);
    }
    Ok(())
}

impl HelperLaunchConfig {
    pub fn validate(&self) -> Result<(), HelperProtocolError> {
        if self.program.as_os_str().is_empty() {
            return Err(HelperProtocolError::EmptyProgram);
        }
        if self.host.trim().is_empty() {
            return Err(HelperProtocolError::EmptyHost);
        }
        validate_metadata("host", &self.host, MAX_HOST_BYTES)?;
        if self.port == 0 {
            return Err(HelperProtocolError::InvalidPort);
        }
        if self.username.trim().is_empty() && self.protocol == DesktopProtocol::Rdp {
            return Err(HelperProtocolError::EmptyUsername);
        }
        validate_metadata("username", &self.username, MAX_USERNAME_BYTES)?;
        if self.credential_ref.trim().is_empty() && self.protocol == DesktopProtocol::Rdp {
            return Err(HelperProtocolError::EmptyCredentialReference);
        }
        if !self.credential_ref.trim().is_empty() {
            validate_metadata(
                "credential reference",
                &self.credential_ref,
                MAX_CREDENTIAL_REFERENCE_BYTES,
            )?;
        }
        if let Some(domain) = self.domain.as_deref() {
            validate_metadata("domain", domain, MAX_DOMAIN_BYTES)?;
        }
        let gateway_fields_present = self.gateway_endpoint.is_some()
            || self.gateway_username.is_some()
            || self.gateway_credential_ref.is_some();
        if self.protocol != DesktopProtocol::Rdp && gateway_fields_present {
            return Err(HelperProtocolError::GatewayOnlyForRdp);
        }
        if gateway_fields_present {
            let endpoint = self
                .gateway_endpoint
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or(HelperProtocolError::MissingGatewayField)?;
            validate_gateway_endpoint(endpoint)?;
            let username = self
                .gateway_username
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or(HelperProtocolError::MissingGatewayField)?;
            validate_metadata("gateway username", username, MAX_USERNAME_BYTES)?;
            let credential_ref = self
                .gateway_credential_ref
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or(HelperProtocolError::MissingGatewayField)?;
            validate_metadata(
                "gateway credential reference",
                credential_ref,
                MAX_CREDENTIAL_REFERENCE_BYTES,
            )?;
        }
        if self.color_depth == 0 {
            return Err(HelperProtocolError::InvalidColorDepth);
        }
        if self.protocol == DesktopProtocol::Rdp {
            validate_rdp_color_depth(self.color_depth)?;
        }
        if self.protocol == DesktopProtocol::Vnc
            && !matches!(
                self.vnc_quality.as_str(),
                "balanced" | "low-latency" | "low-bandwidth"
            )
        {
            return Err(HelperProtocolError::InvalidVncQuality);
        }
        ReconnectPolicy {
            enabled: self.reconnect_enabled,
            attempts: self.reconnect_attempts,
        }
        .validate()?;
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
            "--width".into(),
            self.display.width.to_string(),
            "--height".into(),
            self.display.height.to_string(),
            if self.reconnect_enabled {
                "--reconnect-enabled".into()
            } else {
                "--reconnect-disabled".into()
            },
            "--reconnect-attempts".into(),
            self.reconnect_attempts.to_string(),
        ];
        if !self.username.trim().is_empty() {
            arguments.extend(["--username".into(), self.username.clone()]);
        }
        if self.protocol == DesktopProtocol::Rdp {
            arguments.extend(["--color-depth".into(), self.color_depth.to_string()]);
            if let Some(domain) = self.domain.as_deref().filter(|value| !value.is_empty()) {
                arguments.extend(["--domain".into(), domain.into()]);
            }
            if self.audio_enabled {
                arguments.push("--audio".into());
            }
            if self.clipboard_enabled {
                arguments.push("--clipboard-enabled".into());
            }
            if let Some(endpoint) = self.gateway_endpoint.as_deref() {
                arguments.extend(["--gateway-endpoint".into(), endpoint.into()]);
            }
            if let Some(username) = self.gateway_username.as_deref() {
                arguments.extend(["--gateway-username".into(), username.into()]);
            }
        } else {
            arguments.extend(["--quality".into(), self.vnc_quality.clone()]);
        }
        arguments
    }
}

#[derive(PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "command",
    content = "payload",
    rename_all = "camelCase",
    deny_unknown_fields
)]
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
    Wheel {
        x: u16,
        y: u16,
        delta: i16,
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
            Self::Wheel { x, y, delta } => formatter
                .debug_struct("Wheel")
                .field("x", x)
                .field("y", y)
                .field("delta", delta)
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
            Self::Wheel { delta, .. } if !(-256..=255).contains(delta) || *delta == 0 => {
                Err(HelperProtocolError::InvalidWheelDelta { delta: *delta })
            }
            _ => Ok(()),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "event",
    content = "payload",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum HelperEvent {
    Hello {
        version: u16,
    },
    Capabilities {
        capabilities: HelperCapabilities,
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
            Self::Capabilities { capabilities } => formatter
                .debug_struct("Capabilities")
                .field("protocol", &capabilities.protocol)
                .field("clipboard", &capabilities.clipboard)
                .field("audio", &capabilities.audio)
                .field("server_resize", &capabilities.server_resize)
                .field("local_scaling", &capabilities.local_scaling)
                .field("gateway", &capabilities.gateway)
                .field("color_depths", &capabilities.color_depths)
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
            Self::Capabilities { capabilities } => capabilities.validate(),
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
    #[error("helper {field} is too large")]
    MetadataTooLarge { field: &'static str, bytes: usize },
    #[error("helper {field} contains control characters")]
    MetadataContainsControl { field: &'static str },
    #[error("RDP gateway settings are supported only for RDP")]
    GatewayOnlyForRdp,
    #[error("RDP gateway settings require an endpoint, username, and credential reference")]
    MissingGatewayField,
    #[error("RDP gateway endpoint must be an explicit host:port target")]
    InvalidGatewayEndpoint,
    #[error("helper color depth must be non-zero")]
    InvalidColorDepth,
    #[error("helper RDP color depth {depth} is unsupported; use 16 or 32")]
    UnsupportedRdpColorDepth { depth: u16 },
    #[error("helper VNC quality is invalid")]
    InvalidVncQuality,
    #[error("helper capability list is too large: {count} entries")]
    CapabilitiesTooLarge { count: usize },
    #[error("helper capability color depth must be non-zero: {depth}")]
    InvalidCapabilityColorDepth { depth: u16 },
    #[error("helper reconnect attempts {attempts} exceed the safety limit")]
    InvalidReconnectAttempts { attempts: u8 },
    #[error("invalid display size {width}x{height}")]
    InvalidDisplaySize { width: u16, height: u16 },
    #[error("clipboard payload is too large: {bytes} bytes")]
    ClipboardTooLarge { bytes: usize },
    #[error("wheel delta is outside the supported range: {delta}")]
    InvalidWheelDelta { delta: i16 },
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
    #[error("helper pipe write timed out")]
    PipeWriteTimeout,
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

    encode_frame_zeroizing(&CredentialEnvelopeRef {
        version: WIRE_VERSION,
        credential,
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
    write_frame_with_timeout(writer, &frame).await
}

/// Writes the credential handoff and guarantees that the serialized frame is
/// zeroized after the pipe write completes.
pub async fn write_credential_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    credential: &HelperCredential,
) -> Result<(), HelperProtocolError> {
    let frame = encode_credential_frame(credential)?;
    write_frame_with_timeout(writer, &frame).await
}

/// Writes one already-validated native frame with a deadline. Helper pipes
/// are local IPC, but a crashed or wedged peer must not leave a protocol task
/// blocked forever while the parent is trying to stop it.
pub async fn write_frame_with_timeout<W: AsyncWrite + Unpin>(
    writer: &mut W,
    frame: &[u8],
) -> Result<(), HelperProtocolError> {
    write_frame_with_deadline(writer, frame, HELPER_PIPE_WRITE_TIMEOUT).await
}

async fn write_frame_with_deadline<W: AsyncWrite + Unpin>(
    writer: &mut W,
    frame: &[u8],
    deadline: Duration,
) -> Result<(), HelperProtocolError> {
    timeout(deadline, async {
        writer
            .write_all(frame)
            .await
            .map_err(|error| HelperProtocolError::Io(error.to_string()))?;
        writer
            .flush()
            .await
            .map_err(|error| HelperProtocolError::Io(error.to_string()))
    })
    .await
    .map_err(|_| HelperProtocolError::PipeWriteTimeout)?
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
        write_frame_with_timeout(stdin, &frame)
            .await
            .map_err(HelperProcessError::Protocol)
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
        // Helper diagnostics use the bounded, typed stdout protocol. Keeping
        // stderr disconnected prevents an unconsumed OS pipe from stalling a
        // helper and avoids retaining uncontrolled process output.
        .stderr(Stdio::null())
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
            gateway_endpoint: None,
            gateway_username: None,
            gateway_credential_ref: None,
            display: DisplaySize {
                width: 1280,
                height: 800,
            },
            color_depth: 32,
            audio_enabled: false,
            clipboard_enabled: false,
            vnc_quality: "balanced".into(),
            credential_ref: "credential:test-only".into(),
            reconnect_enabled: true,
            reconnect_attempts: 3,
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
    fn launch_arguments_include_each_optional_domain_value_once() {
        let arguments = launch_config().process_arguments();
        let domain_count = arguments
            .iter()
            .filter(|argument| *argument == "--domain")
            .count();
        assert_eq!(domain_count, 1);
        assert_eq!(
            arguments
                .iter()
                .filter(|argument| *argument == "LAB")
                .count(),
            1
        );
    }

    #[test]
    fn launch_arguments_include_only_the_explicit_rdp_clipboard_opt_in() {
        let mut config = launch_config();
        assert!(
            !config
                .process_arguments()
                .contains(&"--clipboard-enabled".into())
        );

        config.clipboard_enabled = true;
        let arguments = config.process_arguments();
        assert_eq!(
            arguments
                .iter()
                .filter(|argument| argument.as_str() == "--clipboard-enabled")
                .count(),
            1
        );
    }

    #[test]
    fn gateway_arguments_expose_only_non_secret_metadata() {
        let mut config = launch_config();
        config.gateway_endpoint = Some("gateway.example:443".into());
        config.gateway_username = Some("gateway-user".into());
        config.gateway_credential_ref = Some("gateway-secret-ref".into());

        config.validate().unwrap();
        let arguments = config.process_arguments();
        assert!(arguments.contains(&"--gateway-endpoint".into()));
        assert!(arguments.contains(&"gateway.example:443".into()));
        assert!(arguments.contains(&"--gateway-username".into()));
        assert!(arguments.contains(&"gateway-user".into()));
        assert!(
            !arguments
                .iter()
                .any(|argument| argument.contains("gateway-secret-ref"))
        );
        assert!(!format!("{config:?}").contains("gateway-secret-ref"));
    }

    #[test]
    fn gateway_settings_require_complete_rdp_metadata() {
        let mut config = launch_config();
        config.gateway_endpoint = Some("gateway.example:443".into());
        assert_eq!(
            config.validate(),
            Err(HelperProtocolError::MissingGatewayField)
        );

        config.gateway_username = Some("gateway-user".into());
        config.gateway_credential_ref = Some("gateway-secret-ref".into());
        config.protocol = DesktopProtocol::Vnc;
        assert_eq!(
            config.validate(),
            Err(HelperProtocolError::GatewayOnlyForRdp)
        );

        config.protocol = DesktopProtocol::Rdp;
        config.gateway_endpoint = Some("gateway.example".into());
        assert_eq!(
            config.validate(),
            Err(HelperProtocolError::InvalidGatewayEndpoint)
        );
    }

    #[test]
    fn vnc_no_auth_can_omit_username_and_credential_reference() {
        let mut config = launch_config();
        config.protocol = DesktopProtocol::Vnc;
        config.username.clear();
        config.credential_ref.clear();
        config.program = PathBuf::from("/opt/mobarust/bin/mobarust-vnc-helper");

        config.validate().unwrap();
        let arguments = config.process_arguments();
        assert!(!arguments.contains(&"--username".into()));
        assert!(!arguments.contains(&"credential:test-only".into()));
    }

    #[test]
    fn vnc_arguments_exclude_rdp_only_options() {
        let mut config = launch_config();
        config.protocol = DesktopProtocol::Vnc;
        config.domain = Some("SHOULD-NOT-CROSS-PROTOCOLS".into());
        config.audio_enabled = true;

        let arguments = config.process_arguments();
        assert!(!arguments.contains(&"--color-depth".into()));
        assert!(!arguments.contains(&"--domain".into()));
        assert!(!arguments.contains(&"--audio".into()));
        assert!(arguments.contains(&"--quality".into()));
        let quality_index = arguments
            .iter()
            .position(|argument| argument == "--quality")
            .unwrap();
        assert_eq!(arguments[quality_index + 1], "balanced");
    }

    #[test]
    fn vnc_quality_is_validated_before_process_start() {
        let mut config = launch_config();
        config.protocol = DesktopProtocol::Vnc;
        config.vnc_quality = "unbounded".into();
        assert_eq!(
            config.validate(),
            Err(HelperProtocolError::InvalidVncQuality)
        );
    }

    #[test]
    fn rdp_color_depth_is_validated_before_process_start() {
        let mut config = launch_config();
        config.color_depth = 24;
        assert_eq!(
            config.validate(),
            Err(HelperProtocolError::UnsupportedRdpColorDepth { depth: 24 })
        );

        for depth in [16, 32] {
            config.color_depth = depth;
            config.validate().unwrap();
        }
    }

    #[test]
    fn reconnect_policy_is_explicit_bounded_and_non_secret() {
        let config = launch_config();
        let arguments = config.process_arguments();
        assert!(arguments.contains(&"--reconnect-enabled".into()));
        assert!(arguments.contains(&"--reconnect-attempts".into()));
        let attempts_index = arguments
            .iter()
            .position(|argument| argument == "--reconnect-attempts")
            .unwrap();
        assert_eq!(arguments[attempts_index + 1], "3");

        let mut invalid = config;
        invalid.reconnect_attempts = MAX_REMOTE_DESKTOP_RECONNECT_ATTEMPTS + 1;
        assert!(matches!(
            invalid.validate(),
            Err(HelperProtocolError::InvalidReconnectAttempts { attempts: 11 })
        ));
    }

    #[test]
    fn rdp_extended_scancode_contract_preserves_base_code_and_marker() {
        assert_eq!(rdp_scancode_parts(0x148), Some((0x48, true)));
        assert_eq!(rdp_scancode_parts(0x4b), Some((0x4b, false)));
        assert_eq!(rdp_scancode_parts(0x1ff), None);
        assert_eq!(rdp_scancode_parts(0), None);
    }

    #[test]
    fn launch_config_rejects_oversized_or_control_metadata() {
        let mut config = launch_config();
        config.host = "h".repeat(MAX_HOST_BYTES + 1);
        assert!(matches!(
            config.validate(),
            Err(HelperProtocolError::MetadataTooLarge { field: "host", .. })
        ));

        let mut config = launch_config();
        config.username = "u".repeat(MAX_USERNAME_BYTES + 1);
        assert!(matches!(
            config.validate(),
            Err(HelperProtocolError::MetadataTooLarge {
                field: "username",
                ..
            })
        ));

        let mut config = launch_config();
        config.domain = Some(format!("LAB{}", '\n'));
        assert!(matches!(
            config.validate(),
            Err(HelperProtocolError::MetadataContainsControl { field: "domain" })
        ));

        let mut config = launch_config();
        config.credential_ref = "c".repeat(MAX_CREDENTIAL_REFERENCE_BYTES + 1);
        assert!(matches!(
            config.validate(),
            Err(HelperProtocolError::MetadataTooLarge {
                field: "credential reference",
                ..
            })
        ));
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
    fn helper_credential_adopts_a_zeroizing_buffer() {
        let credential =
            HelperCredential::from_zeroizing(zeroize::Zeroizing::new("fixture-password".into()));

        assert_eq!(credential.password(), "fixture-password");
        assert_eq!(format!("{credential:?}"), "HelperCredential(<redacted>)");
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

    #[test]
    fn credential_frames_preserve_gateway_purpose_without_exposing_secret() {
        let credential = HelperCredential::gateway("fixture-gateway-password");
        let frame = encode_credential_frame(&credential).unwrap();
        let decoded = decode_credential_frame(&frame).unwrap();
        assert_eq!(decoded.kind(), HelperCredentialKind::Gateway);
        assert_eq!(decoded.password(), "fixture-gateway-password");
        assert!(!format!("{decoded:?}").contains("fixture-gateway-password"));
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

    #[tokio::test]
    async fn backpressured_helper_pipe_write_hits_its_deadline() {
        let (mut writer, _reader) = tokio::io::duplex(1);
        let result = tokio::time::timeout(
            Duration::from_millis(100),
            write_frame_with_deadline(&mut writer, &[0; 4096], Duration::from_millis(5)),
        )
        .await
        .expect("the test must not wait beyond its outer deadline");

        assert!(matches!(result, Err(HelperProtocolError::PipeWriteTimeout)));
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
        let wheel = HelperCommand::Wheel {
            x: 12,
            y: 18,
            delta: 120,
        };
        let wheel_frame = encode_command_frame(&wheel).unwrap();
        assert_eq!(decode_command_frame(&wheel_frame).unwrap(), wheel);
        assert!(matches!(
            decode_command_frame(&frame[..frame.len() - 1]),
            Err(HelperProtocolError::TruncatedFrame { .. })
        ));
    }

    #[test]
    fn helper_capabilities_round_trip_and_describe_platform_limits() {
        let rdp = HelperCapabilities::rdp();
        assert_eq!(rdp.protocol, DesktopProtocol::Rdp);
        assert_eq!(rdp.clipboard, cfg!(windows));
        assert!(!rdp.audio);
        assert!(rdp.server_resize);
        assert!(rdp.gateway);
        assert_eq!(rdp.color_depths, vec![16, 32]);

        let vnc = HelperCapabilities::vnc();
        assert_eq!(vnc.protocol, DesktopProtocol::Vnc);
        assert!(vnc.clipboard);
        assert!(!vnc.server_resize);
        assert!(vnc.local_scaling);
        assert!(!vnc.gateway);

        let frame = encode_event_frame(&HelperEvent::Capabilities {
            capabilities: rdp.clone(),
        })
        .unwrap();
        assert_eq!(
            decode_event_frame(&frame).unwrap(),
            HelperEvent::Capabilities { capabilities: rdp }
        );
    }

    #[test]
    fn helper_capabilities_reject_invalid_depth_metadata() {
        let mut invalid_depth = HelperCapabilities::vnc();
        invalid_depth.color_depths = vec![0];
        assert!(matches!(
            invalid_depth.validate(),
            Err(HelperProtocolError::InvalidCapabilityColorDepth { depth: 0 })
        ));

        let mut too_many = HelperCapabilities::vnc();
        too_many.color_depths = (1..=9).collect();
        assert!(matches!(
            too_many.validate(),
            Err(HelperProtocolError::CapabilitiesTooLarge { count: 9 })
        ));

        let mut unsupported_rdp_depth = HelperCapabilities::rdp();
        unsupported_rdp_depth.color_depths = vec![16, 24, 32];
        assert!(matches!(
            unsupported_rdp_depth.validate(),
            Err(HelperProtocolError::UnsupportedRdpColorDepth { depth: 24 })
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
    fn frames_reject_unknown_fields_at_each_ipc_layer() {
        let command_envelope = encode_frame(&serde_json::json!({
            "version": WIRE_VERSION,
            "payload": { "command": "stop" },
            "unknown": true
        }))
        .unwrap();
        assert!(decode_command_frame(&command_envelope).is_err());

        let command_payload = encode_frame(&serde_json::json!({
            "version": WIRE_VERSION,
            "payload": {
                "command": "resize",
                "payload": {
                    "display": { "width": 1024, "height": 768 },
                    "unknown": true
                }
            }
        }))
        .unwrap();
        assert!(decode_command_frame(&command_payload).is_err());

        let credential_envelope = encode_frame(&serde_json::json!({
            "version": WIRE_VERSION,
            "credential": { "password": "fixture-password" },
            "unknown": true
        }))
        .unwrap();
        assert!(decode_credential_frame(&credential_envelope).is_err());

        let credential_payload = encode_frame(&serde_json::json!({
            "version": WIRE_VERSION,
            "credential": { "password": "fixture-password", "unknown": true }
        }))
        .unwrap();
        assert!(decode_credential_frame(&credential_payload).is_err());

        let event_envelope = encode_frame(&serde_json::json!({
            "version": WIRE_VERSION,
            "payload": { "event": "state", "payload": { "state": "ready" } },
            "unknown": true
        }))
        .unwrap();
        assert!(decode_event_frame(&event_envelope).is_err());

        let event_payload = encode_frame(&serde_json::json!({
            "version": WIRE_VERSION,
            "payload": {
                "event": "state",
                "payload": { "state": "ready", "unknown": true }
            }
        }))
        .unwrap();
        assert!(decode_event_frame(&event_payload).is_err());
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
        let oversized_display = DisplaySize {
            width: 4096,
            height: 2048,
        };
        assert!(matches!(
            oversized_display.validate(),
            Err(HelperProtocolError::FrameTooLarge { .. })
        ));
        assert!(matches!(
            HelperCommand::Wheel {
                x: 0,
                y: 0,
                delta: 0,
            }
            .validate(),
            Err(HelperProtocolError::InvalidWheelDelta { delta: 0 })
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
