use mobarust_core::{
    ConnectionState, TerminalInputError, Utf8OutputDecoder, validate_terminal_input,
};
use mobarust_serial::{
    SerialConnection, SerialDataBits, SerialFlowControl, SerialOptions, SerialParity,
    SerialStopBits,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use thiserror::Error;
use tokio::sync::{Semaphore, mpsc};
use uuid::Uuid;

const COMMAND_CAPACITY: usize = 64;
const PENDING_OUTPUT_CHUNKS: usize = 32;
// ponytail: one enumeration worker; raise only after validating driver concurrency on each OS.
const SERIAL_ENUMERATION_WORKERS: usize = 1;
const SERIAL_ENUMERATION_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
    Reconnecting,
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
    Reconnect,
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
    #[error("serial device enumeration is already in progress")]
    EnumerationInProgress,
    #[error("serial device enumeration timed out")]
    EnumerationTimeout,
    #[error("invalid serial request: {0}")]
    InvalidRequest(String),
    #[error(transparent)]
    Transport(#[from] mobarust_serial::SerialError),
    #[error(transparent)]
    Input(#[from] TerminalInputError),
}

#[derive(Clone)]
pub struct SerialManager {
    sessions: Arc<Mutex<HashMap<String, SerialSessionStateData>>>,
    enumeration_slots: Arc<Semaphore>,
}

impl Default for SerialManager {
    fn default() -> Self {
        Self {
            sessions: Arc::default(),
            enumeration_slots: Arc::new(Semaphore::new(SERIAL_ENUMERATION_WORKERS)),
        }
    }
}

impl SerialManager {
    pub async fn list_devices(
        &self,
    ) -> Result<Vec<mobarust_serial::SerialDeviceInfo>, SerialManagerError> {
        enumerate_with_worker_slot(
            Arc::clone(&self.enumeration_slots),
            SERIAL_ENUMERATION_TIMEOUT,
            mobarust_serial::enumerate_devices,
        )
        .await
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
        validate_terminal_input(data.as_bytes())?;
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

    pub async fn reconnect(&self, terminal_id: &str) -> Result<(), SerialManagerError> {
        self.sender(terminal_id)?
            .send(SerialCommand::Reconnect)
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

async fn enumerate_with_worker_slot<F>(
    slots: Arc<Semaphore>,
    timeout: Duration,
    enumerate: F,
) -> Result<Vec<mobarust_serial::SerialDeviceInfo>, SerialManagerError>
where
    F: FnOnce() -> Result<Vec<mobarust_serial::SerialDeviceInfo>, mobarust_serial::SerialError>
        + Send
        + 'static,
{
    let slot = slots
        .try_acquire_owned()
        .map_err(|_| SerialManagerError::EnumerationInProgress)?;
    let worker = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        enumerate()
    });
    tokio::time::timeout(timeout, worker)
        .await
        .map_err(|_| SerialManagerError::EnumerationTimeout)?
        .map_err(|_| SerialManagerError::Transport(mobarust_serial::SerialError::Worker))?
        .map_err(SerialManagerError::Transport)
}

async fn run_serial_session(
    manager: SerialManager,
    app: AppHandle,
    terminal_id: String,
    connection: SerialConnection,
    mut commands: mpsc::Receiver<SerialCommand>,
) {
    manager.emit_state(&app, &terminal_id, SerialSessionState::Connected, None);
    let mut output_decoder = Utf8OutputDecoder::default();
    let reason = 'session: loop {
        tokio::select! {
            read = connection.read(16 * 1024), if connection.state() == ConnectionState::Connected => {
                match read {
                    Ok(bytes) if bytes.is_empty() => continue,
                    Ok(bytes) => {
                        let text = output_decoder.push(&bytes);
                        if !text.is_empty() {
                            manager.publish_output(&app, &terminal_id, text);
                        }
                    }
                    Err(error) => {
                        let reason = error.to_string();
                        if matches!(error, mobarust_serial::SerialError::DeviceDisconnected { .. }) {
                            manager.emit_state(&app, &terminal_id, SerialSessionState::Reconnecting, Some(reason));
                            continue 'session;
                        }
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
                            if matches!(error, mobarust_serial::SerialError::DeviceDisconnected { .. }) {
                                manager.emit_state(&app, &terminal_id, SerialSessionState::Reconnecting, Some(reason));
                                continue 'session;
                            }
                            if connection.state() != ConnectionState::Connected {
                                manager.emit_state(&app, &terminal_id, SerialSessionState::Failed, Some(reason));
                                continue 'session;
                            }
                            manager.emit_state(&app, &terminal_id, SerialSessionState::Failed, Some(reason.clone()));
                            break 'session reason;
                        }
                    }
                    Some(SerialCommand::Reconnect) => {
                        if connection.state() == ConnectionState::Connected {
                            continue 'session;
                        }
                        manager.emit_state(&app, &terminal_id, SerialSessionState::Reconnecting, None);
                        match connection.reconnect().await {
                            Ok(()) => {
                                let tail = output_decoder.finish();
                                if !tail.is_empty() {
                                    manager.publish_output(&app, &terminal_id, tail);
                                }
                                manager.emit_state(&app, &terminal_id, SerialSessionState::Connected, None);
                            }
                            Err(error) => manager.emit_state(&app, &terminal_id, SerialSessionState::Failed, Some(error.to_string())),
                        }
                    }
                    Some(SerialCommand::Close) | None => {
                        break 'session "closed by application".to_owned();
                    }
                }
            }
        }
    };

    let tail = output_decoder.finish();
    if !tail.is_empty() {
        manager.publish_output(&app, &terminal_id, tail);
    }
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

#[cfg(test)]
mod tests {
    use super::{SerialManager, SerialManagerError, enumerate_with_worker_slot};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;
    use tokio::sync::{Semaphore, oneshot};

    #[tokio::test]
    async fn oversized_serial_write_is_rejected_before_session_lookup() {
        let data = "x".repeat(mobarust_core::MAX_TERMINAL_INPUT_BYTES + 1);
        let error = SerialManager::default()
            .write("missing", data)
            .await
            .expect_err("oversized serial input must be rejected");
        assert!(matches!(
            error,
            SerialManagerError::Input(mobarust_core::TerminalInputError::TooLarge)
        ));
    }

    #[tokio::test]
    async fn timed_out_device_enumeration_keeps_its_worker_slot_until_exit() {
        let slots = Arc::new(Semaphore::new(1));
        let (started_sender, started_receiver) = oneshot::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let worker = tokio::spawn(enumerate_with_worker_slot(
            Arc::clone(&slots),
            Duration::from_millis(100),
            move || {
                let _ = started_sender.send(());
                let _ = release_receiver.recv();
                Ok(Vec::new())
            },
        ));

        started_receiver
            .await
            .expect("enumeration worker must start");
        assert!(matches!(
            worker.await.expect("enumeration task must finish"),
            Err(SerialManagerError::EnumerationTimeout)
        ));
        assert!(matches!(
            enumerate_with_worker_slot(Arc::clone(&slots), Duration::from_secs(1), || {
                Ok(Vec::new())
            })
            .await,
            Err(SerialManagerError::EnumerationInProgress)
        ));

        release_sender
            .send(())
            .expect("blocked enumeration worker must be released");
        let slot = tokio::time::timeout(Duration::from_secs(1), Arc::clone(&slots).acquire_owned())
            .await
            .expect("worker slot must return after enumeration exits")
            .expect("enumeration semaphore must remain open");
        drop(slot);

        assert!(
            enumerate_with_worker_slot(slots, Duration::from_secs(1), || Ok(Vec::new()))
                .await
                .is_ok()
        );
    }
}
