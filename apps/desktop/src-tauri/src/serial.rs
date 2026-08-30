use mobarust_core::OutputBatcher;
use mobarust_serial::{
    SerialConnection, SerialDataBits, SerialFlowControl, SerialOptions, SerialParity,
    SerialStopBits,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};
use thiserror::Error;
use tokio::sync::mpsc;
use uuid::Uuid;

const COMMAND_CAPACITY: usize = 64;
const OUTPUT_BATCH_BYTES: usize = 32 * 1024;
const PENDING_OUTPUT_CHUNKS: usize = 32;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SerialConnectRequest {
    pub device: String,
    pub baud_rate: u32,
    pub data_bits: SerialDataBits,
    pub stop_bits: SerialStopBits,
    pub parity: SerialParity,
    pub flow_control: SerialFlowControl,
    #[serde(default)]
    pub line_ending: mobarust_serial::LineEnding,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SerialConnectResponse {
    pub terminal_id: String,
    pub device: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum SerialSessionState {
    Connected,
    Disconnected,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SerialSessionEvent {
    terminal_id: String,
    state: SerialSessionState,
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SerialOutputEvent {
    terminal_id: String,
    data: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SerialClosedEvent {
    terminal_id: String,
    reason: String,
}

enum SerialCommand {
    Write(Vec<u8>),
    Close,
}

struct SerialSessionStateData {
    sender: mpsc::Sender<SerialCommand>,
    attached: bool,
    pending_output: Vec<String>,
}

#[derive(Debug, Error)]
pub enum SerialManagerError {
    #[error("serial session is not found: {0}")]
    MissingSession(String),
    #[error("serial session command queue is closed")]
    Closed,
    #[error("invalid serial request: {0}")]
    InvalidRequest(String),
    #[error(transparent)]
    Transport(#[from] mobarust_serial::SerialError),
}

#[derive(Clone, Default)]
pub struct SerialManager {
    sessions: Arc<Mutex<HashMap<String, SerialSessionStateData>>>,
}

impl SerialManager {
    pub async fn list_devices() -> Result<Vec<mobarust_serial::SerialDeviceInfo>, SerialManagerError>
    {
        tokio::task::spawn_blocking(mobarust_serial::enumerate_devices)
            .await
            .map_err(|_| SerialManagerError::Transport(mobarust_serial::SerialError::Worker))?
            .map_err(SerialManagerError::Transport)
    }

    pub async fn connect(
        &self,
        app: AppHandle,
        request: SerialConnectRequest,
    ) -> Result<SerialConnectResponse, SerialManagerError> {
        let device = request.device.clone();
        let options = SerialOptions {
            device: request.device,
            baud_rate: request.baud_rate,
            data_bits: request.data_bits,
            stop_bits: request.stop_bits,
            parity: request.parity,
            flow_control: request.flow_control,
            line_ending: request.line_ending,
            ..SerialOptions::new("unused", 115_200)
        };
        options
            .validate()
            .map_err(|error| SerialManagerError::InvalidRequest(error.to_string()))?;
        let connection = SerialConnection::connect(options).await?;
        let terminal_id = Uuid::new_v4().to_string();
        let (sender, receiver) = mpsc::channel(COMMAND_CAPACITY);
        self.sessions
            .lock()
            .map_err(|_| SerialManagerError::Closed)?
            .insert(
                terminal_id.clone(),
                SerialSessionStateData {
                    sender,
                    attached: false,
                    pending_output: Vec::new(),
                },
            );

        let manager = self.clone();
        let id_for_task = terminal_id.clone();
        tauri::async_runtime::spawn(async move {
            run_serial_session(manager, app, id_for_task, connection, receiver).await;
        });

        Ok(SerialConnectResponse {
            terminal_id,
            device,
        })
    }

    pub async fn write(&self, terminal_id: &str, data: String) -> Result<(), SerialManagerError> {
        self.sender(terminal_id)?
            .send(SerialCommand::Write(data.into_bytes()))
            .await
            .map_err(|_| SerialManagerError::Closed)
    }

    pub async fn close(&self, terminal_id: &str) -> Result<(), SerialManagerError> {
        self.sender(terminal_id)?
            .send(SerialCommand::Close)
            .await
            .map_err(|_| SerialManagerError::Closed)
    }

    pub fn attach(&self, terminal_id: &str) -> Result<Vec<String>, SerialManagerError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| SerialManagerError::Closed)?;
        let state = sessions
            .get_mut(terminal_id)
            .ok_or_else(|| SerialManagerError::MissingSession(terminal_id.to_owned()))?;
        state.attached = true;
        Ok(std::mem::take(&mut state.pending_output))
    }

    fn sender(&self, terminal_id: &str) -> Result<mpsc::Sender<SerialCommand>, SerialManagerError> {
        self.sessions
            .lock()
            .map_err(|_| SerialManagerError::Closed)?
            .get(terminal_id)
            .map(|state| state.sender.clone())
            .ok_or_else(|| SerialManagerError::MissingSession(terminal_id.to_owned()))
    }

    fn publish_output(&self, app: &AppHandle, terminal_id: &str, data: String) {
        let should_emit = if let Ok(mut sessions) = self.sessions.lock() {
            let Some(state) = sessions.get_mut(terminal_id) else {
                return;
            };
            if state.attached {
                true
            } else {
                state.pending_output.push(data.clone());
                if state.pending_output.len() > PENDING_OUTPUT_CHUNKS {
                    state.pending_output.remove(0);
                }
                false
            }
        } else {
            false
        };
        if should_emit {
            let _ = app.emit(
                "serial://output",
                SerialOutputEvent {
                    terminal_id: terminal_id.to_owned(),
                    data,
                },
            );
        }
    }

    fn emit_state(
        &self,
        app: &AppHandle,
        terminal_id: &str,
        state: SerialSessionState,
        error: Option<String>,
    ) {
        let _ = app.emit(
            "serial://state",
            SerialSessionEvent {
                terminal_id: terminal_id.to_owned(),
                state,
                error,
            },
        );
    }

    fn remove(&self, terminal_id: &str) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.remove(terminal_id);
        }
    }
}

async fn run_serial_session(
    manager: SerialManager,
    app: AppHandle,
    terminal_id: String,
    connection: SerialConnection,
    mut commands: mpsc::Receiver<SerialCommand>,
) {
    manager.emit_state(&app, &terminal_id, SerialSessionState::Connected, None);
    let mut output_batcher = OutputBatcher::new(OUTPUT_BATCH_BYTES);
    let reason = 'session: loop {
        tokio::select! {
            read = connection.read(16 * 1024) => {
                match read {
                    Ok(bytes) if bytes.is_empty() => continue,
                    Ok(bytes) => {
                        for chunk in output_batcher.push(&bytes) {
                            manager.publish_output(&app, &terminal_id, String::from_utf8_lossy(&chunk.bytes).into_owned());
                        }
                        if let Some(chunk) = output_batcher.flush() {
                            manager.publish_output(&app, &terminal_id, String::from_utf8_lossy(&chunk.bytes).into_owned());
                        }
                    }
                    Err(error) => {
                        let reason = error.to_string();
                        manager.emit_state(&app, &terminal_id, SerialSessionState::Failed, Some(reason.clone()));
                        break 'session reason;
                    }
                }
            }
            command = commands.recv() => {
                match command {
                    Some(SerialCommand::Write(data)) => {
                        let data = connection.options().frame_terminal_input(&data);
                        if data.is_empty() {
                            continue;
                        }
                        if let Err(error) = connection.write(&data).await {
                            let reason = error.to_string();
                            manager.emit_state(&app, &terminal_id, SerialSessionState::Failed, Some(reason.clone()));
                            break 'session reason;
                        }
                    }
                    Some(SerialCommand::Close) | None => {
                        break 'session "closed by application".to_owned();
                    }
                }
            }
        }
    };

    let _ = connection.close().await;
    manager.emit_state(&app, &terminal_id, SerialSessionState::Disconnected, None);
    let _ = app.emit(
        "serial://closed",
        SerialClosedEvent {
            terminal_id: terminal_id.clone(),
            reason,
        },
    );
    manager.remove(&terminal_id);
}
