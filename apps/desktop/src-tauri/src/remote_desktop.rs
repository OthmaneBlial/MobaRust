use mobarust_remote_desktop::{
    DesktopProtocol, DisplaySize, HelperCommand, HelperCredential, HelperEvent, HelperLaunchConfig,
    HelperSupervisor, decode_event_frame, encode_command_frame, read_frame,
};
use mobarust_vault::{CredentialId, CredentialLookup};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex, mpsc};
use uuid::Uuid;

const HELPER_GRACE_PERIOD: Duration = Duration::from_secs(2);
const COMMAND_CAPACITY: usize = 32;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
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
            HelperCredential::new(secret.as_str())
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
        self.sessions.lock().await.insert(
            session_id.clone(),
            ManagedSession {
                commands: command_tx,
                supervisor: Arc::clone(&supervisor),
            },
        );
        let sessions = Arc::clone(&self.sessions);
        let reader_session_id = session_id.clone();
        tokio::spawn(read_helper_events(app, reader_session_id, stdout, sessions));
        tokio::spawn(write_helper_commands(stdin, command_rx));

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
    if !program.is_file() {
        return Err(format!(
            "{} helper is not installed in the application package",
            protocol_name(protocol)
        ));
    }
    Ok(program)
}

fn validate_request(request: &RemoteDesktopConnectRequest) -> Result<(), String> {
    if request.host.trim().is_empty()
        || request.host.chars().any(char::is_control)
        || request.port == 0
    {
        return Err("remote desktop host and port are invalid".into());
    }
    if request.username.trim().is_empty() || request.username.chars().any(char::is_control) {
        return Err("remote desktop username is invalid".into());
    }
    if request
        .domain
        .as_deref()
        .is_some_and(|domain| domain.chars().any(char::is_control))
    {
        return Err("remote desktop domain is invalid".into());
    }
    if request.color_depth == 0 {
        return Err("remote desktop color depth is invalid".into());
    }
    DisplaySize {
        width: request.width,
        height: request.height,
    }
    .validate()
    .map_err(|error| error.to_string())
}

fn default_color_depth() -> u16 {
    32
}

fn protocol_name(protocol: DesktopProtocol) -> &'static str {
    match protocol {
        DesktopProtocol::Rdp => "RDP",
        DesktopProtocol::Vnc => "VNC",
    }
}

async fn write_helper_commands<W: AsyncWrite + Unpin>(
    mut stdin: W,
    mut commands: mpsc::Receiver<HelperCommand>,
) {
    while let Some(command) = commands.recv().await {
        let Ok(frame) = encode_command_frame(&command) else {
            break;
        };
        if stdin.write_all(&frame[..]).await.is_err() || stdin.flush().await.is_err() {
            break;
        }
    }
}

async fn read_helper_events(
    app: AppHandle,
    session_id: String,
    mut stdout: impl tokio::io::AsyncRead + Unpin,
    sessions: Arc<Mutex<HashMap<String, ManagedSession>>>,
) {
    loop {
        let frame = match read_frame(&mut stdout).await {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                emit_helper_event(
                    &app,
                    &session_id,
                    HelperEvent::State {
                        state: mobarust_remote_desktop::HelperState::Crashed,
                    },
                );
                break;
            }
            Err(_) => {
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
                break;
            }
        };
        let event = match decode_event_frame(&frame) {
            Ok(event) => event,
            Err(_) => {
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
            break;
        }
    }
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
