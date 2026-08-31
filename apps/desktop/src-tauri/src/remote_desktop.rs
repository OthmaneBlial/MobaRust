use mobarust_remote_desktop::{
    DEFAULT_REMOTE_DESKTOP_RECONNECT_ATTEMPTS, DEFAULT_REMOTE_DESKTOP_RECONNECT_ENABLED,
    DesktopProtocol, DisplaySize, HelperCapabilities, HelperCommand, HelperCredential, HelperEvent,
    HelperLaunchConfig, HelperProtocolError, HelperSupervisor, MAX_CREDENTIAL_REFERENCE_BYTES,
    MAX_DOMAIN_BYTES, MAX_GATEWAY_ENDPOINT_BYTES, MAX_HOST_BYTES,
    MAX_REMOTE_DESKTOP_RECONNECT_ATTEMPTS, MAX_USERNAME_BYTES, ReconnectPolicy,
    VNC_TRANSPORT_INSECURE_TCP, VNC_TRANSPORT_LOOPBACK_ONLY, decode_event_frame,
    encode_command_frame, read_frame, validate_gateway_endpoint, validate_rdp_color_depth,
    vnc_keysym_is_supported, write_frame_with_timeout,
};
use mobarust_vault::{CredentialId, CredentialLookup};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWrite;
use tokio::sync::{Mutex, mpsc};
use uuid::Uuid;

const HELPER_GRACE_PERIOD: Duration = Duration::from_secs(2);
const HELPER_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const COMMAND_CAPACITY: usize = 32;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteDesktopConnectRequest {
    pub protocol: DesktopProtocol,
    pub host: String,
    pub port: u16,
    pub username: String,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub gateway_endpoint: Option<String>,
    #[serde(default)]
    pub gateway_username: Option<String>,
    #[serde(default)]
    pub gateway_credential_id: Option<String>,
    #[serde(default)]
    pub credential_id: Option<String>,
    pub width: u16,
    pub height: u16,
    #[serde(default = "default_color_depth")]
    pub color_depth: u16,
    #[serde(default = "default_vnc_quality")]
    pub vnc_quality: String,
    #[serde(default)]
    pub allow_insecure_vnc: bool,
    #[serde(default)]
    pub audio_enabled: bool,
    #[serde(default)]
    pub clipboard_enabled: bool,
    #[serde(default = "default_reconnect_enabled")]
    pub reconnect_enabled: bool,
    #[serde(default = "default_reconnect_attempts")]
    pub reconnect_attempts: u8,
}

fn default_reconnect_enabled() -> bool {
    DEFAULT_REMOTE_DESKTOP_RECONNECT_ENABLED
}

fn default_reconnect_attempts() -> u8 {
    DEFAULT_REMOTE_DESKTOP_RECONNECT_ATTEMPTS
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDesktopConnectResponse {
    pub session_id: String,
    pub protocol: DesktopProtocol,
    pub host: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDesktopEvent {
    pub session_id: String,
    pub event: HelperEvent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HelperCapabilityRequirements {
    protocol: DesktopProtocol,
    clipboard: bool,
    audio: bool,
    gateway: bool,
    color_depth: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SessionCommandPolicy {
    protocol: DesktopProtocol,
    clipboard_enabled: bool,
}

impl From<HelperCapabilityRequirements> for SessionCommandPolicy {
    fn from(requirements: HelperCapabilityRequirements) -> Self {
        Self {
            protocol: requirements.protocol,
            clipboard_enabled: requirements.clipboard,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum HelperSessionPhase {
    #[default]
    Starting,
    Ready,
    Active,
    Reconnecting,
    Stopping,
    Terminal,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum HelperDataPhase {
    #[default]
    AwaitingCapabilities,
    AwaitingActive,
    Active,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct HelperEventProgress {
    hello_seen: bool,
    starting_seen: bool,
    ready_seen: bool,
    capabilities_seen: bool,
    data_phase: HelperDataPhase,
    reported_capabilities: Option<HelperCapabilities>,
}

fn required_helper_capabilities(
    request: &RemoteDesktopConnectRequest,
) -> HelperCapabilityRequirements {
    HelperCapabilityRequirements {
        protocol: request.protocol,
        clipboard: request.clipboard_enabled,
        audio: request.audio_enabled,
        gateway: request.gateway_endpoint.is_some(),
        color_depth: (request.protocol == DesktopProtocol::Rdp).then_some(request.color_depth),
    }
}

#[derive(Clone)]
struct ManagedSession {
    commands: mpsc::Sender<HelperCommand>,
    supervisor: Arc<Mutex<HelperSupervisor>>,
    stop_requested: Arc<AtomicBool>,
    command_policy: SessionCommandPolicy,
    capabilities: Arc<Mutex<Option<HelperCapabilities>>>,
    phase: Arc<Mutex<HelperSessionPhase>>,
}

#[derive(Clone, Default)]
pub struct RemoteDesktopManager {
    sessions: Arc<Mutex<HashMap<String, ManagedSession>>>,
}

impl RemoteDesktopManager {
    pub async fn start(
        &self,
        app: AppHandle,
        program: PathBuf,
        resolver: &dyn CredentialLookup,
        request: RemoteDesktopConnectRequest,
    ) -> Result<RemoteDesktopConnectResponse, String> {
        validate_request(&request)?;
        let capability_requirements = required_helper_capabilities(&request);
        let credential_id = request
            .credential_id
            .clone()
            .unwrap_or_default()
            .trim()
            .to_owned();
        let gateway_credential_id = request
            .gateway_credential_id
            .clone()
            .unwrap_or_default()
            .trim()
            .to_owned();
        let credential = if credential_id.is_empty() {
            if request.protocol == DesktopProtocol::Rdp {
                return Err("RDP requires a saved credential reference".into());
            }
            HelperCredential::new("")
        } else {
            let id = CredentialId::new(&credential_id).map_err(|error| error.to_string())?;
            let secret = resolver.get(&id).map_err(|error| error.to_string())?;
            HelperCredential::from_zeroizing(secret.into_zeroizing())
        };
        let gateway_credential = if gateway_credential_id.is_empty() {
            None
        } else {
            let id =
                CredentialId::new(&gateway_credential_id).map_err(|error| error.to_string())?;
            let secret = resolver.get(&id).map_err(|error| error.to_string())?;
            Some(HelperCredential::from_zeroizing_with_kind(
                mobarust_remote_desktop::HelperCredentialKind::Gateway,
                secret.into_zeroizing(),
            ))
        };
        let config = HelperLaunchConfig {
            program,
            protocol: request.protocol,
            host: request.host.clone(),
            port: request.port,
            username: request.username,
            domain: request.domain,
            gateway_endpoint: request.gateway_endpoint,
            gateway_username: request.gateway_username,
            gateway_credential_ref: (!gateway_credential_id.is_empty())
                .then_some(gateway_credential_id),
            display: DisplaySize {
                width: request.width,
                height: request.height,
            },
            color_depth: request.color_depth,
            audio_enabled: request.audio_enabled,
            clipboard_enabled: request.clipboard_enabled,
            vnc_quality: request.vnc_quality,
            allow_insecure_vnc: request.allow_insecure_vnc,
            credential_ref: credential_id,
            reconnect_enabled: request.reconnect_enabled,
            reconnect_attempts: request.reconnect_attempts,
        };
        let mut supervisor = HelperSupervisor::spawn(&config).map_err(|error| error.to_string())?;
        if let Err(error) = supervisor
            .send_command(&HelperCommand::Start {
                protocol: request.protocol,
                display: config.display,
            })
            .await
        {
            let _ = supervisor.stop(HELPER_GRACE_PERIOD).await;
            return Err(error.to_string());
        }
        if let Err(error) = supervisor.send_credentials(&credential).await {
            let _ = supervisor.stop(HELPER_GRACE_PERIOD).await;
            return Err(error.to_string());
        }
        if let Some(gateway_credential) = gateway_credential.as_ref()
            && let Err(error) = supervisor.send_credentials(gateway_credential).await
        {
            let _ = supervisor.stop(HELPER_GRACE_PERIOD).await;
            return Err(error.to_string());
        }

        let stdin = match supervisor.take_stdin() {
            Ok(stdin) => stdin,
            Err(error) => {
                let _ = supervisor.stop(HELPER_GRACE_PERIOD).await;
                return Err(error.to_string());
            }
        };
        let stdout = match supervisor.take_stdout() {
            Ok(stdout) => stdout,
            Err(error) => {
                let _ = supervisor.stop(HELPER_GRACE_PERIOD).await;
                return Err(error.to_string());
            }
        };

        let session_id = Uuid::new_v4().to_string();
        let (command_tx, command_rx) = mpsc::channel(COMMAND_CAPACITY);
        let supervisor = Arc::new(Mutex::new(supervisor));
        let stop_requested = Arc::new(AtomicBool::new(false));
        let capabilities = Arc::new(Mutex::new(None));
        let phase = Arc::new(Mutex::new(HelperSessionPhase::Starting));
        self.sessions.lock().await.insert(
            session_id.clone(),
            ManagedSession {
                commands: command_tx,
                supervisor: Arc::clone(&supervisor),
                stop_requested: Arc::clone(&stop_requested),
                command_policy: capability_requirements.into(),
                capabilities: Arc::clone(&capabilities),
                phase: Arc::clone(&phase),
            },
        );
        let sessions = Arc::clone(&self.sessions);
        let reader_session_id = session_id.clone();
        let reader_supervisor = Arc::clone(&supervisor);
        let writer_app = app.clone();
        let writer_session_id = session_id.clone();
        let writer_supervisor = Arc::clone(&supervisor);
        let writer_stop_requested = Arc::clone(&stop_requested);
        let reader_context = HelperReaderContext {
            sessions,
            stop_requested,
            supervisor: reader_supervisor,
            capability_requirements,
            capabilities,
            phase,
        };
        tokio::spawn(read_helper_events(
            app,
            reader_session_id,
            stdout,
            reader_context,
        ));
        tokio::spawn(write_helper_commands(
            writer_app,
            writer_session_id,
            stdin,
            command_rx,
            writer_supervisor,
            writer_stop_requested,
        ));

        Ok(RemoteDesktopConnectResponse {
            session_id,
            protocol: request.protocol,
            host: request.host,
        })
    }

    pub async fn send(&self, session_id: &str, command: HelperCommand) -> Result<(), String> {
        command.validate().map_err(|error| error.to_string())?;
        let session = self
            .sessions
            .lock()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| "remote desktop session was not found".to_owned())?;
        let capabilities = session.capabilities.lock().await;
        let phase = session.phase.lock().await;
        validate_command_for_session(
            session.command_policy,
            capabilities.as_ref(),
            *phase,
            &command,
        )?;
        drop(phase);
        drop(capabilities);
        session
            .commands
            .send(command)
            .await
            .map_err(|_| "remote desktop helper is no longer accepting input".to_owned())
    }

    pub async fn stop(&self, session_id: &str) -> Result<(), String> {
        let session = self
            .sessions
            .lock()
            .await
            .remove(session_id)
            .ok_or_else(|| "remote desktop session was not found".to_owned())?;
        session.stop_requested.store(true, Ordering::Release);
        let _ = session.commands.send(HelperCommand::Stop).await;
        session
            .supervisor
            .lock()
            .await
            .stop(HELPER_GRACE_PERIOD)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

pub fn helper_program(app: &AppHandle, protocol: DesktopProtocol) -> Result<PathBuf, String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|error| format!("application resources are unavailable: {error}"))?;
    let filename = match protocol {
        DesktopProtocol::Rdp => "mobarust-rdp-helper",
        DesktopProtocol::Vnc => "mobarust-vnc-helper",
    };
    let filename = if cfg!(windows) {
        format!("{filename}.exe")
    } else {
        filename.to_owned()
    };
    let program = resource_dir.join("helpers").join(filename);
    validate_helper_resource(&program, protocol)?;
    Ok(program)
}

fn validate_helper_resource(
    program: &std::path::Path,
    protocol: DesktopProtocol,
) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(program).map_err(|_| {
        format!(
            "{} helper is not installed in the application package",
            protocol_name(protocol)
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(format!(
            "{} helper resource is not a regular file",
            protocol_name(protocol)
        ));
    }
    Ok(())
}

pub(crate) fn validate_request(request: &RemoteDesktopConnectRequest) -> Result<(), String> {
    if request.host.trim().is_empty() || request.port == 0 {
        return Err("remote desktop host and port are invalid".into());
    }
    validate_metadata(
        &request.host,
        MAX_HOST_BYTES,
        "remote desktop host is invalid",
    )?;
    if request.protocol != DesktopProtocol::Vnc && request.allow_insecure_vnc {
        return Err("insecure VNC transport is supported only for VNC".into());
    }
    if request.protocol == DesktopProtocol::Vnc
        && !request.allow_insecure_vnc
        && !is_loopback_ip_literal(&request.host)
    {
        return Err(format!(
            "VNC transport must be {VNC_TRANSPORT_LOOPBACK_ONLY}; explicitly enable {VNC_TRANSPORT_INSECURE_TCP} for an unencrypted remote target"
        ));
    }
    if (request.protocol == DesktopProtocol::Rdp && request.username.trim().is_empty())
        || request.username != request.username.trim()
        || request.username.len() > MAX_USERNAME_BYTES
        || request.username.chars().any(char::is_control)
    {
        return Err("remote desktop username is invalid".into());
    }
    if let Some(domain) = request.domain.as_deref() {
        validate_metadata(domain, MAX_DOMAIN_BYTES, "remote desktop domain is invalid")?;
    }
    let gateway_fields_present = request.gateway_endpoint.is_some()
        || request.gateway_username.is_some()
        || request.gateway_credential_id.is_some();
    if request.protocol != DesktopProtocol::Rdp && gateway_fields_present {
        return Err("RDP gateway settings are supported only for RDP".into());
    }
    if gateway_fields_present {
        let endpoint = request
            .gateway_endpoint
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "RDP gateway endpoint is required".to_owned())?;
        if endpoint.len() > MAX_GATEWAY_ENDPOINT_BYTES {
            return Err("RDP gateway endpoint is invalid".into());
        }
        validate_gateway_endpoint(endpoint).map_err(|error| error.to_string())?;
        let username = request
            .gateway_username
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "RDP gateway username is required".to_owned())?;
        validate_metadata(
            username,
            MAX_USERNAME_BYTES,
            "RDP gateway username is invalid",
        )?;
        let credential_id = request
            .gateway_credential_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "RDP gateway credential reference is required".to_owned())?;
        validate_metadata(
            credential_id,
            MAX_CREDENTIAL_REFERENCE_BYTES,
            "RDP gateway credential reference is invalid",
        )?;
    }
    if let Some(credential_id) = request.credential_id.as_deref() {
        validate_metadata(
            credential_id,
            MAX_CREDENTIAL_REFERENCE_BYTES,
            "remote desktop credential reference is invalid",
        )?;
    }
    if request.audio_enabled {
        return Err("remote desktop audio redirection is not enabled in this helper".into());
    }
    if request.protocol == DesktopProtocol::Rdp && request.clipboard_enabled && !cfg!(windows) {
        return Err("RDP clipboard redirection requires the native Windows backend".into());
    }
    if request.color_depth == 0 {
        return Err("remote desktop color depth is invalid".into());
    }
    if request.protocol == DesktopProtocol::Rdp {
        validate_rdp_color_depth(request.color_depth).map_err(|error| error.to_string())?;
    }
    if !matches!(
        request.vnc_quality.as_str(),
        "balanced" | "low-latency" | "low-bandwidth"
    ) {
        return Err("remote desktop VNC quality is invalid".into());
    }
    ReconnectPolicy {
        enabled: request.reconnect_enabled,
        attempts: request.reconnect_attempts,
    }
    .validate()
    .map_err(|_| {
        format!(
            "remote desktop reconnect attempts exceed the safety limit of {}",
            MAX_REMOTE_DESKTOP_RECONNECT_ATTEMPTS
        )
    })?;
    DisplaySize {
        width: request.width,
        height: request.height,
    }
    .validate()
    .map_err(|error| error.to_string())
}

fn validate_metadata(value: &str, max_bytes: usize, message: &str) -> Result<(), String> {
    if value != value.trim() || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(message.into());
    }
    Ok(())
}

fn is_loopback_ip_literal(value: &str) -> bool {
    value
        .parse::<std::net::IpAddr>()
        .map(|address| address.is_loopback())
        .unwrap_or(false)
}

fn default_color_depth() -> u16 {
    32
}

fn default_vnc_quality() -> String {
    "balanced".into()
}

fn protocol_name(protocol: DesktopProtocol) -> &'static str {
    match protocol {
        DesktopProtocol::Rdp => "RDP",
        DesktopProtocol::Vnc => "VNC",
    }
}

async fn write_helper_commands<W: AsyncWrite + Unpin>(
    app: AppHandle,
    session_id: String,
    mut stdin: W,
    mut commands: mpsc::Receiver<HelperCommand>,
    supervisor: Arc<Mutex<HelperSupervisor>>,
    stop_requested: Arc<AtomicBool>,
) {
    while let Some(command) = commands.recv().await {
        let frame = match encode_command_frame(&command) {
            Ok(frame) => frame,
            Err(error) => {
                report_helper_input_failure(
                    &app,
                    &session_id,
                    &supervisor,
                    &stop_requested,
                    &error,
                )
                .await;
                break;
            }
        };
        if let Err(error) = write_frame_with_timeout(&mut stdin, &frame).await {
            report_helper_input_failure(&app, &session_id, &supervisor, &stop_requested, &error)
                .await;
            break;
        }
    }
}

async fn report_helper_input_failure(
    app: &AppHandle,
    session_id: &str,
    supervisor: &Arc<Mutex<HelperSupervisor>>,
    stop_requested: &Arc<AtomicBool>,
    error: &HelperProtocolError,
) {
    if !claim_unexpected_helper_exit(stop_requested) {
        return;
    }
    emit_helper_event(
        app,
        session_id,
        HelperEvent::Diagnostic {
            level: mobarust_remote_desktop::DiagnosticLevel::Error,
            message: helper_input_failure_message(error).into(),
        },
    );
    emit_helper_event(
        app,
        session_id,
        HelperEvent::State {
            state: mobarust_remote_desktop::HelperState::Crashed,
        },
    );
    let _ = supervisor.lock().await.stop(HELPER_GRACE_PERIOD).await;
}

fn helper_input_failure_message(error: &HelperProtocolError) -> &'static str {
    match error {
        HelperProtocolError::PipeWriteTimeout => "remote desktop helper input pipe timed out",
        _ => "remote desktop helper input pipe failed",
    }
}

struct HelperReaderContext {
    sessions: Arc<Mutex<HashMap<String, ManagedSession>>>,
    stop_requested: Arc<AtomicBool>,
    supervisor: Arc<Mutex<HelperSupervisor>>,
    capability_requirements: HelperCapabilityRequirements,
    capabilities: Arc<Mutex<Option<HelperCapabilities>>>,
    phase: Arc<Mutex<HelperSessionPhase>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HelperFrameReadError {
    Protocol,
    HandshakeTimeout,
}

async fn read_next_helper_frame<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    handshake_deadline: Option<Instant>,
) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, HelperFrameReadError> {
    if let Some(deadline) = handshake_deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(HelperFrameReadError::HandshakeTimeout);
        }
        tokio::time::timeout(remaining, read_frame(reader))
            .await
            .map_err(|_| HelperFrameReadError::HandshakeTimeout)?
            .map_err(|_| HelperFrameReadError::Protocol)
    } else {
        read_frame(reader)
            .await
            .map_err(|_| HelperFrameReadError::Protocol)
    }
}

async fn read_helper_events(
    app: AppHandle,
    session_id: String,
    mut stdout: impl tokio::io::AsyncRead + Unpin,
    context: HelperReaderContext,
) {
    let HelperReaderContext {
        sessions,
        stop_requested,
        supervisor,
        capability_requirements,
        capabilities,
        phase,
    } = context;
    let mut progress = HelperEventProgress::default();
    let handshake_deadline = Instant::now() + HELPER_HANDSHAKE_TIMEOUT;
    loop {
        let frame = match read_next_helper_frame(
            &mut stdout,
            (!progress.capabilities_seen).then_some(handshake_deadline),
        )
        .await
        {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                if claim_unexpected_helper_exit(&stop_requested) {
                    emit_helper_event(
                        &app,
                        &session_id,
                        HelperEvent::State {
                            state: mobarust_remote_desktop::HelperState::Crashed,
                        },
                    );
                }
                break;
            }
            Err(HelperFrameReadError::HandshakeTimeout) => {
                if claim_unexpected_helper_exit(&stop_requested) {
                    emit_helper_event(
                        &app,
                        &session_id,
                        HelperEvent::Diagnostic {
                            level: mobarust_remote_desktop::DiagnosticLevel::Error,
                            message: "remote desktop helper handshake timed out".into(),
                        },
                    );
                    emit_helper_event(
                        &app,
                        &session_id,
                        HelperEvent::State {
                            state: mobarust_remote_desktop::HelperState::Failed,
                        },
                    );
                }
                break;
            }
            Err(HelperFrameReadError::Protocol) => {
                if claim_unexpected_helper_exit(&stop_requested) {
                    emit_helper_event(
                        &app,
                        &session_id,
                        HelperEvent::Diagnostic {
                            level: mobarust_remote_desktop::DiagnosticLevel::Error,
                            message: "remote desktop helper protocol failed".into(),
                        },
                    );
                    emit_helper_event(
                        &app,
                        &session_id,
                        HelperEvent::State {
                            state: mobarust_remote_desktop::HelperState::Crashed,
                        },
                    );
                }
                break;
            }
        };
        let event = match decode_event_frame(&frame) {
            Ok(event) => event,
            Err(_) => {
                if claim_unexpected_helper_exit(&stop_requested) {
                    emit_helper_event(
                        &app,
                        &session_id,
                        HelperEvent::Diagnostic {
                            level: mobarust_remote_desktop::DiagnosticLevel::Error,
                            message: "remote desktop helper sent an invalid event".into(),
                        },
                    );
                    emit_helper_event(
                        &app,
                        &session_id,
                        HelperEvent::State {
                            state: mobarust_remote_desktop::HelperState::Crashed,
                        },
                    );
                }
                break;
            }
        };
        match validate_helper_event(&event, capability_requirements, progress) {
            Ok(next_progress) => progress = next_progress,
            Err(message) => {
                if claim_unexpected_helper_exit(&stop_requested) {
                    emit_helper_event(
                        &app,
                        &session_id,
                        HelperEvent::Diagnostic {
                            level: mobarust_remote_desktop::DiagnosticLevel::Error,
                            message: message.into(),
                        },
                    );
                    emit_helper_event(
                        &app,
                        &session_id,
                        HelperEvent::State {
                            state: mobarust_remote_desktop::HelperState::Failed,
                        },
                    );
                }
                break;
            }
        }
        if let HelperEvent::Capabilities {
            capabilities: reported,
        } = &event
        {
            *capabilities.lock().await = Some(reported.clone());
        }
        if let HelperEvent::State { state } = &event {
            *phase.lock().await = match state {
                mobarust_remote_desktop::HelperState::Starting => HelperSessionPhase::Starting,
                mobarust_remote_desktop::HelperState::Ready => HelperSessionPhase::Ready,
                mobarust_remote_desktop::HelperState::Active => HelperSessionPhase::Active,
                mobarust_remote_desktop::HelperState::Reconnecting => {
                    HelperSessionPhase::Reconnecting
                }
                mobarust_remote_desktop::HelperState::Stopping => HelperSessionPhase::Stopping,
                mobarust_remote_desktop::HelperState::Created
                | mobarust_remote_desktop::HelperState::Stopped
                | mobarust_remote_desktop::HelperState::Failed
                | mobarust_remote_desktop::HelperState::Crashed => HelperSessionPhase::Terminal,
            };
        }
        let terminal_event = matches!(
            &event,
            HelperEvent::State {
                state: mobarust_remote_desktop::HelperState::Stopped
                    | mobarust_remote_desktop::HelperState::Failed
                    | mobarust_remote_desktop::HelperState::Crashed
            }
        );
        emit_helper_event(&app, &session_id, event);
        if terminal_event {
            stop_requested.store(true, Ordering::Release);
            break;
        }
    }
    stop_requested.store(true, Ordering::Release);
    // EOF and protocol errors can happen while the child still owns the
    // other side of the pipe. Reuse the same bounded wait/kill/reap path as a
    // user-initiated stop before dropping the supervisor.
    let _ = supervisor.lock().await.stop(HELPER_GRACE_PERIOD).await;
    sessions.lock().await.remove(&session_id);
}

fn validate_helper_event(
    event: &HelperEvent,
    requirements: HelperCapabilityRequirements,
    mut progress: HelperEventProgress,
) -> Result<HelperEventProgress, &'static str> {
    if !progress.hello_seen && !matches!(event, HelperEvent::Hello { .. }) {
        return Err("remote desktop helper sent an event before its handshake");
    }
    match event {
        HelperEvent::Hello { .. } if progress.hello_seen => {
            Err("remote desktop helper sent a duplicate handshake")
        }
        HelperEvent::Hello { .. } => {
            progress.hello_seen = true;
            Ok(progress)
        }
        HelperEvent::Capabilities { capabilities } => {
            if !progress.ready_seen {
                return Err("remote desktop helper reported capabilities before ready state");
            }
            if progress.data_phase == HelperDataPhase::Active {
                return Err("remote desktop helper sent a duplicate capability report");
            }
            if progress.data_phase == HelperDataPhase::AwaitingActive {
                return Err("remote desktop helper sent a duplicate capability report");
            }
            validate_helper_capabilities(capabilities, requirements)?;
            progress.capabilities_seen = true;
            progress.data_phase = HelperDataPhase::AwaitingActive;
            progress.reported_capabilities = Some(capabilities.clone());
            Ok(progress)
        }
        HelperEvent::State {
            state: mobarust_remote_desktop::HelperState::Starting,
        } => {
            progress.starting_seen = true;
            Ok(progress)
        }
        HelperEvent::State {
            state: mobarust_remote_desktop::HelperState::Ready,
        } => {
            if !progress.starting_seen {
                return Err("remote desktop helper sent ready state before starting state");
            }
            if progress.ready_seen {
                return Err("remote desktop helper sent a duplicate ready state");
            }
            progress.ready_seen = true;
            Ok(progress)
        }
        HelperEvent::State {
            state: mobarust_remote_desktop::HelperState::Reconnecting,
        } => {
            if !progress.ready_seen {
                return Err("remote desktop helper sent reconnecting state before ready state");
            }
            progress.ready_seen = false;
            progress.data_phase = HelperDataPhase::AwaitingCapabilities;
            progress.reported_capabilities = None;
            Ok(progress)
        }
        HelperEvent::State {
            state: mobarust_remote_desktop::HelperState::Active,
        } => match progress.data_phase {
            HelperDataPhase::AwaitingCapabilities => {
                Err("remote desktop helper sent active state before reporting capabilities")
            }
            HelperDataPhase::AwaitingActive => {
                progress.data_phase = HelperDataPhase::Active;
                Ok(progress)
            }
            HelperDataPhase::Active => Err("remote desktop helper sent a duplicate active state"),
        },
        HelperEvent::Framebuffer { .. } | HelperEvent::Clipboard { .. }
            if progress.data_phase != HelperDataPhase::Active =>
        {
            Err("remote desktop helper sent active data before entering the active state")
        }
        HelperEvent::Clipboard { .. } if !requirements.clipboard => {
            Err("remote desktop helper sent clipboard data without requested clipboard opt-in")
        }
        HelperEvent::Clipboard { .. }
            if progress
                .reported_capabilities
                .as_ref()
                .is_some_and(|capabilities| !capabilities.clipboard) =>
        {
            Err("remote desktop helper sent clipboard data without clipboard capability")
        }
        _ => Ok(progress),
    }
}

fn validate_helper_capabilities(
    capabilities: &HelperCapabilities,
    requirements: HelperCapabilityRequirements,
) -> Result<(), &'static str> {
    if capabilities.protocol != requirements.protocol {
        return Err("remote desktop helper reported an incompatible protocol");
    }
    if requirements.clipboard && !capabilities.clipboard {
        return Err("remote desktop helper does not support requested clipboard redirection");
    }
    if requirements.audio && !capabilities.audio {
        return Err("remote desktop helper does not support requested audio redirection");
    }
    if requirements.gateway && !capabilities.gateway {
        return Err("remote desktop helper does not support the requested gateway");
    }
    if let Some(depth) = requirements.color_depth
        && !capabilities.color_depths.contains(&depth)
    {
        return Err("remote desktop helper does not support the requested color depth");
    }
    Ok(())
}

fn validate_command_for_session(
    policy: SessionCommandPolicy,
    capabilities: Option<&HelperCapabilities>,
    phase: HelperSessionPhase,
    command: &HelperCommand,
) -> Result<(), String> {
    if !matches!(command, HelperCommand::Stop) && phase != HelperSessionPhase::Active {
        return Err("remote desktop helper is not active yet".into());
    }
    match command {
        HelperCommand::Resize { .. } if policy.protocol != DesktopProtocol::Rdp => {
            Err("remote desktop server resize is unavailable for this protocol".into())
        }
        HelperCommand::Resize { .. } if capabilities.is_some_and(|value| !value.server_resize) => {
            Err("remote desktop helper does not support server resize for this session".into())
        }
        HelperCommand::Clipboard { .. } if !policy.clipboard_enabled => {
            Err("remote desktop clipboard input is disabled for this session".into())
        }
        HelperCommand::Clipboard { .. } if capabilities.is_some_and(|value| !value.clipboard) => {
            Err("remote desktop helper does not support clipboard input for this session".into())
        }
        HelperCommand::Key { scancode, .. }
            if policy.protocol == DesktopProtocol::Vnc && !vnc_keysym_is_supported(*scancode) =>
        {
            Err("VNC keyboard keysym is outside the supported range".into())
        }
        _ => Ok(()),
    }
}

fn emit_helper_event(app: &AppHandle, session_id: &str, event: HelperEvent) {
    let _ = app.emit(
        "remote-desktop://event",
        RemoteDesktopEvent {
            session_id: session_id.to_owned(),
            event,
        },
    );
}

fn claim_unexpected_helper_exit(stop_requested: &AtomicBool) -> bool {
    stop_requested
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::{
        HelperCapabilityRequirements, HelperDataPhase, HelperEventProgress, HelperFrameReadError,
        HelperSessionPhase, MAX_CREDENTIAL_REFERENCE_BYTES, MAX_DOMAIN_BYTES, MAX_HOST_BYTES,
        MAX_USERNAME_BYTES, RemoteDesktopConnectRequest, SessionCommandPolicy,
        claim_unexpected_helper_exit, helper_input_failure_message, read_next_helper_frame,
        validate_command_for_session, validate_helper_capabilities, validate_helper_event,
        validate_helper_resource, validate_request,
    };
    use mobarust_remote_desktop::{
        DEFAULT_REMOTE_DESKTOP_RECONNECT_ATTEMPTS, DesktopProtocol, HelperCapabilities,
        HelperCommand, HelperEvent, HelperProtocolError, HelperState,
        MAX_REMOTE_DESKTOP_RECONNECT_ATTEMPTS, WIRE_VERSION, encode_event_frame,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};
    use tokio::io::AsyncWriteExt;

    #[test]
    fn normal_helper_exit_claims_the_single_failure_path() {
        let stop_requested = AtomicBool::new(false);
        assert!(claim_unexpected_helper_exit(&stop_requested));
        assert!(!claim_unexpected_helper_exit(&stop_requested));
    }

    #[tokio::test]
    async fn helper_handshake_timeout_is_bounded() {
        let (_writer, mut reader) = tokio::io::duplex(1);
        let started = std::time::Instant::now();
        let result = tokio::time::timeout(
            Duration::from_millis(100),
            read_next_helper_frame(
                &mut reader,
                Some(Instant::now() + Duration::from_millis(10)),
            ),
        )
        .await
        .expect("handshake read must return within the test deadline")
        .unwrap_err();

        assert_eq!(result, HelperFrameReadError::HandshakeTimeout);
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[tokio::test]
    async fn helper_capability_handshake_timeout_also_applies_after_hello() {
        let (mut writer, mut reader) = tokio::io::duplex(4096);
        let hello = encode_event_frame(&HelperEvent::Hello {
            version: WIRE_VERSION,
        })
        .unwrap();
        writer.write_all(&hello).await.unwrap();
        let deadline = Instant::now() + Duration::from_millis(10);
        assert!(
            read_next_helper_frame(&mut reader, Some(deadline))
                .await
                .unwrap()
                .is_some()
        );

        tokio::time::sleep(Duration::from_millis(20)).await;
        let started = Instant::now();
        let error = read_next_helper_frame(&mut reader, Some(deadline))
            .await
            .unwrap_err();
        assert_eq!(error, HelperFrameReadError::HandshakeTimeout);
        assert!(started.elapsed() < Duration::from_millis(10));
    }

    #[test]
    fn forced_shutdown_does_not_emit_a_false_crash() {
        let stop_requested = AtomicBool::new(true);
        assert!(!claim_unexpected_helper_exit(&stop_requested));
        stop_requested.store(false, Ordering::Release);
        assert!(claim_unexpected_helper_exit(&stop_requested));
    }

    #[test]
    fn helper_pipe_failure_messages_are_stable_and_do_not_echo_details() {
        assert_eq!(
            helper_input_failure_message(&HelperProtocolError::PipeWriteTimeout),
            "remote desktop helper input pipe timed out"
        );
        let message =
            helper_input_failure_message(&HelperProtocolError::Io("private helper detail".into()));
        assert_eq!(message, "remote desktop helper input pipe failed");
        assert!(!message.contains("private helper detail"));
    }

    #[test]
    fn helper_ready_requires_the_starting_state() {
        let requirements = HelperCapabilityRequirements {
            protocol: DesktopProtocol::Rdp,
            clipboard: false,
            audio: false,
            gateway: false,
            color_depth: Some(32),
        };
        let hello = validate_helper_event(
            &HelperEvent::Hello {
                version: WIRE_VERSION,
            },
            requirements,
            Default::default(),
        )
        .unwrap();
        let error = validate_helper_event(
            &HelperEvent::State {
                state: HelperState::Ready,
            },
            requirements,
            hello.clone(),
        )
        .unwrap_err();
        assert!(error.contains("before starting state"));

        let starting = validate_helper_event(
            &HelperEvent::State {
                state: HelperState::Starting,
            },
            requirements,
            hello,
        )
        .unwrap();
        let error = validate_helper_event(
            &HelperEvent::State {
                state: HelperState::Reconnecting,
            },
            requirements,
            starting.clone(),
        )
        .unwrap_err();
        assert!(error.contains("before ready state"));
        let ready = validate_helper_event(
            &HelperEvent::State {
                state: HelperState::Ready,
            },
            requirements,
            starting,
        )
        .unwrap();
        let error = validate_helper_event(
            &HelperEvent::State {
                state: HelperState::Ready,
            },
            requirements,
            ready,
        )
        .unwrap_err();
        assert!(error.contains("duplicate ready state"));
    }

    #[test]
    fn helper_capabilities_must_match_the_requested_session() {
        let rdp_capabilities = HelperEvent::Capabilities {
            capabilities: HelperCapabilities::rdp(),
        };
        let rdp_requirements = HelperCapabilityRequirements {
            protocol: DesktopProtocol::Rdp,
            clipboard: false,
            audio: false,
            gateway: false,
            color_depth: Some(32),
        };
        assert!(
            validate_helper_event(&rdp_capabilities, rdp_requirements, Default::default()).is_err()
        );
        let handshake = validate_helper_event(
            &HelperEvent::Hello {
                version: WIRE_VERSION,
            },
            rdp_requirements,
            Default::default(),
        )
        .unwrap();
        assert!(handshake.hello_seen);
        let starting = validate_helper_event(
            &HelperEvent::State {
                state: HelperState::Starting,
            },
            rdp_requirements,
            handshake.clone(),
        )
        .unwrap();
        let ready = validate_helper_event(
            &HelperEvent::State {
                state: HelperState::Ready,
            },
            rdp_requirements,
            starting,
        )
        .unwrap();
        assert!(ready.ready_seen);
        let rdp_progress =
            validate_helper_event(&rdp_capabilities, rdp_requirements, ready.clone()).unwrap();
        assert!(rdp_progress.capabilities_seen);
        assert_eq!(rdp_progress.data_phase, HelperDataPhase::AwaitingActive);
        assert!(
            validate_helper_event(
                &rdp_capabilities,
                HelperCapabilityRequirements {
                    protocol: DesktopProtocol::Vnc,
                    ..rdp_requirements
                },
                ready.clone(),
            )
            .is_err()
        );

        let mut vnc_capability_data = HelperCapabilities::vnc();
        vnc_capability_data.clipboard = false;
        let vnc_capabilities = HelperEvent::Capabilities {
            capabilities: vnc_capability_data.clone(),
        };
        assert!(
            validate_helper_event(
                &vnc_capabilities,
                HelperCapabilityRequirements {
                    protocol: DesktopProtocol::Vnc,
                    clipboard: false,
                    audio: false,
                    gateway: false,
                    color_depth: None,
                },
                ready.clone(),
            )
            .is_ok()
        );
        assert!(
            validate_helper_capabilities(
                &vnc_capability_data,
                HelperCapabilityRequirements {
                    protocol: DesktopProtocol::Vnc,
                    clipboard: true,
                    audio: false,
                    gateway: false,
                    color_depth: None,
                },
            )
            .is_err()
        );
        assert!(
            validate_helper_event(
                &HelperEvent::State {
                    state: HelperState::Ready,
                },
                rdp_requirements,
                handshake.clone(),
            )
            .is_err()
        );
        assert!(
            validate_helper_event(
                &HelperEvent::State {
                    state: HelperState::Active,
                },
                rdp_requirements,
                handshake.clone(),
            )
            .is_err()
        );
        let active_progress = validate_helper_event(
            &HelperEvent::State {
                state: HelperState::Active,
            },
            rdp_requirements,
            rdp_progress.clone(),
        )
        .unwrap();
        assert_eq!(active_progress.data_phase, HelperDataPhase::Active);
        assert_ne!(active_progress, rdp_progress);
        assert!(
            validate_helper_event(&rdp_capabilities, rdp_requirements, active_progress.clone(),)
                .is_err()
        );
        assert!(
            validate_helper_event(
                &HelperEvent::Hello {
                    version: WIRE_VERSION,
                },
                rdp_requirements,
                handshake,
            )
            .is_err()
        );
    }

    #[test]
    fn helper_framebuffer_requires_active_state_after_each_reconnect() {
        let requirements = HelperCapabilityRequirements {
            protocol: DesktopProtocol::Rdp,
            clipboard: false,
            audio: false,
            gateway: false,
            color_depth: Some(32),
        };
        let hello = validate_helper_event(
            &HelperEvent::Hello {
                version: WIRE_VERSION,
            },
            requirements,
            Default::default(),
        )
        .unwrap();
        let capabilities = HelperEvent::Capabilities {
            capabilities: HelperCapabilities::rdp(),
        };
        let starting = validate_helper_event(
            &HelperEvent::State {
                state: HelperState::Starting,
            },
            requirements,
            hello,
        )
        .unwrap();
        let ready = validate_helper_event(
            &HelperEvent::State {
                state: HelperState::Ready,
            },
            requirements,
            starting,
        )
        .unwrap();
        let awaiting_active = validate_helper_event(&capabilities, requirements, ready).unwrap();
        let framebuffer = HelperEvent::Framebuffer {
            width: 1,
            height: 1,
            pixels: vec![0; 4],
        };
        let error =
            validate_helper_event(&framebuffer, requirements, awaiting_active.clone()).unwrap_err();
        assert!(error.contains("active state"));

        let active = validate_helper_event(
            &HelperEvent::State {
                state: HelperState::Active,
            },
            requirements,
            awaiting_active,
        )
        .unwrap();
        assert!(validate_helper_event(&framebuffer, requirements, active.clone()).is_ok());

        let reconnecting = validate_helper_event(
            &HelperEvent::State {
                state: HelperState::Reconnecting,
            },
            requirements,
            active,
        )
        .unwrap();
        assert_eq!(
            reconnecting.data_phase,
            HelperDataPhase::AwaitingCapabilities
        );
        assert!(reconnecting.reported_capabilities.is_none());
        assert!(
            validate_helper_event(
                &HelperEvent::State {
                    state: HelperState::Active,
                },
                requirements,
                reconnecting.clone(),
            )
            .is_err()
        );

        let ready = validate_helper_event(
            &HelperEvent::State {
                state: HelperState::Ready,
            },
            requirements,
            reconnecting,
        )
        .unwrap();
        let awaiting_active = validate_helper_event(&capabilities, requirements, ready).unwrap();
        let active = validate_helper_event(
            &HelperEvent::State {
                state: HelperState::Active,
            },
            requirements,
            awaiting_active,
        )
        .unwrap();
        assert!(validate_helper_event(&framebuffer, requirements, active).is_ok());
    }

    #[test]
    fn helper_event_rejects_clipboard_without_runtime_capability() {
        let requirements = HelperCapabilityRequirements {
            protocol: DesktopProtocol::Rdp,
            clipboard: false,
            audio: false,
            gateway: false,
            color_depth: Some(32),
        };
        let mut capabilities = HelperCapabilities::rdp();
        capabilities.clipboard = false;
        let capability_event = HelperEvent::Capabilities {
            capabilities: capabilities.clone(),
        };
        let handshake = validate_helper_event(
            &HelperEvent::Hello {
                version: WIRE_VERSION,
            },
            requirements,
            Default::default(),
        )
        .unwrap();
        let ready = validate_helper_event(
            &HelperEvent::State {
                state: HelperState::Ready,
            },
            requirements,
            validate_helper_event(
                &HelperEvent::State {
                    state: HelperState::Starting,
                },
                requirements,
                handshake,
            )
            .unwrap(),
        )
        .unwrap();
        let progress = validate_helper_event(&capability_event, requirements, ready).unwrap();
        let progress = validate_helper_event(
            &HelperEvent::State {
                state: HelperState::Active,
            },
            requirements,
            progress,
        )
        .unwrap();
        let error = validate_helper_event(
            &HelperEvent::Clipboard {
                text: String::from("fixture clipboard").into(),
            },
            requirements,
            progress,
        )
        .unwrap_err();
        assert!(error.contains("clipboard opt-in"));

        let requested_clipboard = HelperCapabilityRequirements {
            clipboard: true,
            ..requirements
        };
        let mut active_without_clipboard = HelperEventProgress {
            hello_seen: true,
            starting_seen: true,
            ready_seen: true,
            capabilities_seen: true,
            data_phase: HelperDataPhase::Active,
            reported_capabilities: Some(capabilities.clone()),
        };
        let error = validate_helper_event(
            &HelperEvent::Clipboard {
                text: String::from("fixture clipboard").into(),
            },
            requested_clipboard,
            active_without_clipboard.clone(),
        )
        .unwrap_err();
        assert!(error.contains("clipboard capability"));
        active_without_clipboard.reported_capabilities = Some(HelperCapabilities::vnc());
        assert!(
            validate_helper_event(
                &HelperEvent::Clipboard {
                    text: String::from("fixture clipboard").into(),
                },
                requested_clipboard,
                active_without_clipboard,
            )
            .is_ok()
        );

        capabilities.clipboard = true;
        let handshake = validate_helper_event(
            &HelperEvent::Hello {
                version: WIRE_VERSION,
            },
            requested_clipboard,
            Default::default(),
        )
        .unwrap();
        let ready = validate_helper_event(
            &HelperEvent::State {
                state: HelperState::Ready,
            },
            requested_clipboard,
            validate_helper_event(
                &HelperEvent::State {
                    state: HelperState::Starting,
                },
                requested_clipboard,
                handshake,
            )
            .unwrap(),
        )
        .unwrap();
        let progress = validate_helper_event(
            &HelperEvent::Capabilities { capabilities },
            requested_clipboard,
            ready,
        )
        .unwrap();
        let progress = validate_helper_event(
            &HelperEvent::State {
                state: HelperState::Active,
            },
            requested_clipboard,
            progress,
        )
        .unwrap();
        validate_helper_event(
            &HelperEvent::Clipboard {
                text: String::from("fixture clipboard").into(),
            },
            requested_clipboard,
            progress,
        )
        .unwrap();
    }

    fn request(protocol: DesktopProtocol, username: &str) -> RemoteDesktopConnectRequest {
        RemoteDesktopConnectRequest {
            protocol,
            host: "127.0.0.1".into(),
            port: if protocol == DesktopProtocol::Rdp {
                3389
            } else {
                5900
            },
            username: username.into(),
            domain: None,
            gateway_endpoint: None,
            gateway_username: None,
            gateway_credential_id: None,
            credential_id: None,
            width: 1024,
            height: 768,
            color_depth: 32,
            audio_enabled: false,
            clipboard_enabled: false,
            vnc_quality: "balanced".into(),
            allow_insecure_vnc: false,
            reconnect_enabled: true,
            reconnect_attempts: DEFAULT_REMOTE_DESKTOP_RECONNECT_ATTEMPTS,
        }
    }

    #[test]
    fn session_command_policy_blocks_protocol_unsupported_operations() {
        let vnc_policy = SessionCommandPolicy {
            protocol: DesktopProtocol::Vnc,
            clipboard_enabled: false,
        };
        let resize_error = validate_command_for_session(
            vnc_policy,
            None,
            HelperSessionPhase::Active,
            &HelperCommand::Resize {
                display: mobarust_remote_desktop::DisplaySize {
                    width: 1024,
                    height: 768,
                },
            },
        )
        .unwrap_err();
        assert!(resize_error.contains("unavailable"));

        let clipboard_error = validate_command_for_session(
            vnc_policy,
            None,
            HelperSessionPhase::Active,
            &HelperCommand::Clipboard {
                text: String::from("must stay blocked").into(),
            },
        )
        .unwrap_err();
        assert!(clipboard_error.contains("disabled"));

        let keysym_error = validate_command_for_session(
            vnc_policy,
            None,
            HelperSessionPhase::Active,
            &HelperCommand::Key {
                scancode: u32::MAX,
                pressed: true,
            },
        )
        .unwrap_err();
        assert!(keysym_error.contains("keysym"));
    }

    #[test]
    fn interactive_commands_wait_for_active_helper_but_stop_is_always_allowed() {
        let policy = SessionCommandPolicy {
            protocol: DesktopProtocol::Rdp,
            clipboard_enabled: false,
        };
        let error = validate_command_for_session(
            policy,
            None,
            HelperSessionPhase::Starting,
            &HelperCommand::Key {
                scancode: 30,
                pressed: true,
            },
        )
        .unwrap_err();
        assert!(error.contains("not active"));

        validate_command_for_session(
            policy,
            None,
            HelperSessionPhase::Reconnecting,
            &HelperCommand::Stop,
        )
        .unwrap();
    }

    #[test]
    fn session_command_policy_allows_enabled_rdp_input() {
        let policy = SessionCommandPolicy {
            protocol: DesktopProtocol::Rdp,
            clipboard_enabled: true,
        };
        validate_command_for_session(
            policy,
            None,
            HelperSessionPhase::Active,
            &HelperCommand::Resize {
                display: mobarust_remote_desktop::DisplaySize {
                    width: 1280,
                    height: 720,
                },
            },
        )
        .unwrap();
        validate_command_for_session(
            policy,
            None,
            HelperSessionPhase::Active,
            &HelperCommand::Clipboard {
                text: String::from("approved by the session policy").into(),
            },
        )
        .unwrap();
        validate_command_for_session(
            policy,
            None,
            HelperSessionPhase::Active,
            &HelperCommand::Key {
                scancode: 30,
                pressed: true,
            },
        )
        .unwrap();
    }

    #[test]
    fn session_command_policy_allows_opted_in_vnc_clipboard() {
        let policy = SessionCommandPolicy {
            protocol: DesktopProtocol::Vnc,
            clipboard_enabled: true,
        };
        validate_command_for_session(
            policy,
            Some(&HelperCapabilities::vnc()),
            HelperSessionPhase::Active,
            &HelperCommand::Clipboard {
                text: String::from("approved VNC fixture text").into(),
            },
        )
        .unwrap();
    }

    #[test]
    fn session_command_policy_rechecks_advertised_runtime_capabilities() {
        let policy = SessionCommandPolicy {
            protocol: DesktopProtocol::Rdp,
            clipboard_enabled: true,
        };
        let mut capabilities = HelperCapabilities::rdp();
        capabilities.server_resize = false;
        let resize_error = validate_command_for_session(
            policy,
            Some(&capabilities),
            HelperSessionPhase::Active,
            &HelperCommand::Resize {
                display: mobarust_remote_desktop::DisplaySize {
                    width: 1280,
                    height: 720,
                },
            },
        )
        .unwrap_err();
        assert!(resize_error.contains("server resize"));

        capabilities.server_resize = true;
        capabilities.clipboard = false;
        let clipboard_error = validate_command_for_session(
            policy,
            Some(&capabilities),
            HelperSessionPhase::Active,
            &HelperCommand::Clipboard {
                text: String::from("must stay native").into(),
            },
        )
        .unwrap_err();
        assert!(clipboard_error.contains("clipboard input"));
    }

    #[test]
    fn parent_boundary_allows_vnc_no_auth_metadata_without_username() {
        validate_request(&request(DesktopProtocol::Vnc, "")).unwrap();
    }

    #[test]
    fn parent_boundary_allows_opted_in_vnc_clipboard() {
        let mut request = request(DesktopProtocol::Vnc, "viewer");
        request.clipboard_enabled = true;
        validate_request(&request).unwrap();
    }

    #[test]
    fn parent_boundary_still_requires_rdp_username() {
        let error = validate_request(&request(DesktopProtocol::Rdp, "")).unwrap_err();
        assert!(error.contains("username"));
    }

    #[test]
    fn parent_boundary_rejects_rdp_audio_until_a_backend_exists() {
        let mut request = request(DesktopProtocol::Rdp, "Administrator");
        request.audio_enabled = true;
        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("audio"));
    }

    #[test]
    fn parent_boundary_rejects_vnc_audio_until_a_backend_exists() {
        let mut request = request(DesktopProtocol::Vnc, "viewer");
        request.audio_enabled = true;
        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("audio"));
    }

    #[cfg(not(windows))]
    #[test]
    fn parent_boundary_rejects_rdp_clipboard_before_helper_launch() {
        let mut request = request(DesktopProtocol::Rdp, "Administrator");
        request.clipboard_enabled = true;
        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("Windows"));
    }

    #[test]
    fn parent_boundary_rejects_unsupported_rdp_color_depth() {
        let mut request = request(DesktopProtocol::Rdp, "Administrator");
        request.color_depth = 24;
        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("16 or 32"));
    }

    #[test]
    fn parent_boundary_rejects_unbounded_reconnect_attempts_without_echoing_value() {
        let mut request = request(DesktopProtocol::Rdp, "Administrator");
        request.reconnect_attempts = MAX_REMOTE_DESKTOP_RECONNECT_ATTEMPTS + 1;
        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("safety limit"));
        assert!(!error.contains("11"));
    }

    #[test]
    fn parent_boundary_allows_rdp_hostname_metadata_without_network_io() {
        let mut request = request(DesktopProtocol::Rdp, "fixture-user");
        request.host = "example.invalid".into();
        validate_request(&request).unwrap();
    }

    #[test]
    fn parent_boundary_requires_complete_gateway_metadata() {
        let mut request = request(DesktopProtocol::Rdp, "fixture-user");
        request.gateway_endpoint = Some("gateway.invalid:443".into());
        request.gateway_username = Some("gateway-user".into());
        request.gateway_credential_id = Some("gateway-password".into());
        validate_request(&request).unwrap();

        request.gateway_credential_id = None;
        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("credential"));

        request.gateway_credential_id = Some("gateway-password".into());
        request.gateway_endpoint = Some("gateway.invalid".into());
        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("endpoint"));
    }

    #[test]
    fn parent_boundary_rejects_gateway_metadata_for_vnc() {
        let mut request = request(DesktopProtocol::Vnc, "viewer");
        request.gateway_endpoint = Some("gateway.invalid:443".into());
        request.gateway_username = Some("gateway-user".into());
        request.gateway_credential_id = Some("gateway-password".into());
        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("only for RDP"));
    }

    #[test]
    fn parent_boundary_keeps_vnc_targets_loopback_only() {
        let mut request = request(DesktopProtocol::Vnc, "fixture-user");
        request.host = "example.invalid".into();
        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("VNC"));
        assert!(error.contains("loopback"));

        request.allow_insecure_vnc = true;
        validate_request(&request).unwrap();
    }

    #[test]
    fn parent_boundary_rejects_insecure_vnc_flag_for_rdp() {
        let mut request = request(DesktopProtocol::Rdp, "fixture-user");
        request.allow_insecure_vnc = true;
        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("only for VNC"));
    }

    #[test]
    fn parent_boundary_rejects_oversized_connection_metadata() {
        let mut host_request = request(DesktopProtocol::Vnc, "fixture-user");
        host_request.host = "1".repeat(MAX_HOST_BYTES + 1);
        assert_eq!(
            validate_request(&host_request).unwrap_err(),
            "remote desktop host is invalid"
        );

        let mut username_request = request(DesktopProtocol::Rdp, "fixture-user");
        username_request.username = "u".repeat(MAX_USERNAME_BYTES + 1);
        assert_eq!(
            validate_request(&username_request).unwrap_err(),
            "remote desktop username is invalid"
        );

        let mut domain_request = request(DesktopProtocol::Rdp, "fixture-user");
        domain_request.domain = Some("d".repeat(MAX_DOMAIN_BYTES + 1));
        assert_eq!(
            validate_request(&domain_request).unwrap_err(),
            "remote desktop domain is invalid"
        );

        let mut credential_request = request(DesktopProtocol::Rdp, "fixture-user");
        credential_request.credential_id = Some("c".repeat(MAX_CREDENTIAL_REFERENCE_BYTES + 1));
        assert_eq!(
            validate_request(&credential_request).unwrap_err(),
            "remote desktop credential reference is invalid"
        );

        credential_request.credential_id = Some("credential\nreference".into());
        assert_eq!(
            validate_request(&credential_request).unwrap_err(),
            "remote desktop credential reference is invalid"
        );
    }

    #[test]
    fn parent_boundary_rejects_outer_whitespace_before_helper_launch() {
        let mut host_request = request(DesktopProtocol::Vnc, "fixture-user");
        host_request.host = " 127.0.0.1".into();
        assert_eq!(
            validate_request(&host_request).unwrap_err(),
            "remote desktop host is invalid"
        );

        let mut username_request = request(DesktopProtocol::Rdp, "fixture-user");
        username_request.username.push(' ');
        assert_eq!(
            validate_request(&username_request).unwrap_err(),
            "remote desktop username is invalid"
        );

        let mut domain_request = request(DesktopProtocol::Rdp, "fixture-user");
        domain_request.domain = Some("FIXTURE ".into());
        assert_eq!(
            validate_request(&domain_request).unwrap_err(),
            "remote desktop domain is invalid"
        );
    }

    #[test]
    fn parent_request_rejects_unknown_fields() {
        let payload = serde_json::json!({
            "protocol": "vnc",
            "host": "127.0.0.1",
            "port": 5900,
            "username": "viewer",
            "width": 1024,
            "height": 768,
            "unexpected": "must not cross the IPC boundary"
        });
        let error = serde_json::from_value::<RemoteDesktopConnectRequest>(payload)
            .expect_err("unknown fields must be rejected");
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn helper_resource_must_be_a_regular_file() {
        let directory = tempfile::tempdir().unwrap();
        let helper = directory.path().join("mobarust-vnc-helper");
        std::fs::write(&helper, b"fixture helper").unwrap();
        validate_helper_resource(&helper, DesktopProtocol::Vnc).unwrap();

        let nested_directory = directory.path().join("nested");
        std::fs::create_dir(&nested_directory).unwrap();
        let error = validate_helper_resource(&nested_directory, DesktopProtocol::Vnc).unwrap_err();
        assert_eq!(error, "VNC helper resource is not a regular file");
    }

    #[cfg(unix)]
    #[test]
    fn helper_resource_rejects_symlinks_without_following_them() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        let helper = directory.path().join("mobarust-rdp-helper");
        std::fs::write(&target, b"fixture target").unwrap();
        symlink(&target, &helper).unwrap();

        let error = validate_helper_resource(&helper, DesktopProtocol::Rdp).unwrap_err();
        assert_eq!(error, "RDP helper resource is not a regular file");
    }
}
