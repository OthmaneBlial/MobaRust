use mobarust_remote_desktop::{
    DesktopProtocol, DisplaySize, HelperCommand, HelperCredential, HelperEvent, HelperLaunchConfig,
    HelperProtocolError, HelperSupervisor, MAX_CREDENTIAL_REFERENCE_BYTES, MAX_DOMAIN_BYTES,
    MAX_HOST_BYTES, MAX_USERNAME_BYTES, decode_event_frame, encode_command_frame, read_frame,
    validate_rdp_color_depth, write_frame_with_timeout,
};
use mobarust_vault::{CredentialId, CredentialLookup};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWrite;
use tokio::sync::{Mutex, mpsc};
use uuid::Uuid;

const HELPER_GRACE_PERIOD: Duration = Duration::from_secs(2);
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
    pub credential_id: Option<String>,
    pub width: u16,
    pub height: u16,
    #[serde(default = "default_color_depth")]
    pub color_depth: u16,
    #[serde(default = "default_vnc_quality")]
    pub vnc_quality: String,
    #[serde(default)]
    pub audio_enabled: bool,
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

#[derive(Clone)]
struct ManagedSession {
    commands: mpsc::Sender<HelperCommand>,
    supervisor: Arc<Mutex<HelperSupervisor>>,
    stop_requested: Arc<AtomicBool>,
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
        let credential_id = request
            .credential_id
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
        let config = HelperLaunchConfig {
            program,
            protocol: request.protocol,
            host: request.host.clone(),
            port: request.port,
            username: request.username,
            domain: request.domain,
            display: DisplaySize {
                width: request.width,
                height: request.height,
            },
            color_depth: request.color_depth,
            audio_enabled: request.audio_enabled,
            vnc_quality: request.vnc_quality,
            credential_ref: credential_id,
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
        self.sessions.lock().await.insert(
            session_id.clone(),
            ManagedSession {
                commands: command_tx,
                supervisor: Arc::clone(&supervisor),
                stop_requested: Arc::clone(&stop_requested),
            },
        );
        let sessions = Arc::clone(&self.sessions);
        let reader_session_id = session_id.clone();
        let reader_supervisor = Arc::clone(&supervisor);
        let writer_app = app.clone();
        let writer_session_id = session_id.clone();
        let writer_supervisor = Arc::clone(&supervisor);
        let writer_stop_requested = Arc::clone(&stop_requested);
        tokio::spawn(read_helper_events(
            app,
            reader_session_id,
            stdout,
            sessions,
            stop_requested,
            reader_supervisor,
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
    if request.protocol == DesktopProtocol::Vnc && !is_loopback_ip_literal(&request.host) {
        return Err(
            "the experimental VNC helper is restricted to a loopback IP during candidate review"
                .into(),
        );
    }
    if (request.protocol == DesktopProtocol::Rdp && request.username.trim().is_empty())
        || request.username.len() > MAX_USERNAME_BYTES
        || request.username.chars().any(char::is_control)
    {
        return Err("remote desktop username is invalid".into());
    }
    if let Some(domain) = request.domain.as_deref() {
        validate_metadata(domain, MAX_DOMAIN_BYTES, "remote desktop domain is invalid")?;
    }
    if let Some(credential_id) = request.credential_id.as_deref() {
        validate_metadata(
            credential_id,
            MAX_CREDENTIAL_REFERENCE_BYTES,
            "remote desktop credential reference is invalid",
        )?;
    }
    if request.protocol == DesktopProtocol::Rdp && request.audio_enabled {
        return Err("RDP audio redirection is not enabled in this helper".into());
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
    DisplaySize {
        width: request.width,
        height: request.height,
    }
    .validate()
    .map_err(|error| error.to_string())
}

fn validate_metadata(value: &str, max_bytes: usize, message: &str) -> Result<(), String> {
    if value.len() > max_bytes || value.chars().any(char::is_control) {
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

async fn read_helper_events(
    app: AppHandle,
    session_id: String,
    mut stdout: impl tokio::io::AsyncRead + Unpin,
    sessions: Arc<Mutex<HashMap<String, ManagedSession>>>,
    stop_requested: Arc<AtomicBool>,
    supervisor: Arc<Mutex<HelperSupervisor>>,
) {
    loop {
        let frame = match read_frame(&mut stdout).await {
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
            Err(_) => {
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
        emit_helper_event(&app, &session_id, event.clone());
        if matches!(
            event,
            HelperEvent::State {
                state: mobarust_remote_desktop::HelperState::Stopped
                    | mobarust_remote_desktop::HelperState::Failed
                    | mobarust_remote_desktop::HelperState::Crashed
            }
        ) {
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
        MAX_CREDENTIAL_REFERENCE_BYTES, MAX_DOMAIN_BYTES, MAX_HOST_BYTES, MAX_USERNAME_BYTES,
        RemoteDesktopConnectRequest, claim_unexpected_helper_exit, helper_input_failure_message,
        validate_helper_resource, validate_request,
    };
    use mobarust_remote_desktop::{DesktopProtocol, HelperProtocolError};
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn normal_helper_exit_claims_the_single_failure_path() {
        let stop_requested = AtomicBool::new(false);
        assert!(claim_unexpected_helper_exit(&stop_requested));
        assert!(!claim_unexpected_helper_exit(&stop_requested));
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
            credential_id: None,
            width: 1024,
            height: 768,
            color_depth: 32,
            audio_enabled: false,
            vnc_quality: "balanced".into(),
        }
    }

    #[test]
    fn parent_boundary_allows_vnc_no_auth_metadata_without_username() {
        validate_request(&request(DesktopProtocol::Vnc, "")).unwrap();
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
    fn parent_boundary_rejects_unsupported_rdp_color_depth() {
        let mut request = request(DesktopProtocol::Rdp, "Administrator");
        request.color_depth = 24;
        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("16 or 32"));
    }

    #[test]
    fn parent_boundary_allows_rdp_hostname_metadata_without_network_io() {
        let mut request = request(DesktopProtocol::Rdp, "fixture-user");
        request.host = "example.invalid".into();
        validate_request(&request).unwrap();
    }

    #[test]
    fn parent_boundary_keeps_vnc_targets_loopback_only() {
        let mut request = request(DesktopProtocol::Vnc, "fixture-user");
        request.host = "example.invalid".into();
        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("VNC"));
        assert!(error.contains("loopback"));
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
