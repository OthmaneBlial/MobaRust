use mobarust_core::{
    ConnectionState, TerminalInputError, Utf8OutputDecoder, validate_terminal_input,
};
use mobarust_telnet::{TelnetConnection, TelnetEncoding, TelnetError, TelnetOptions};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};
use thiserror::Error;
use tokio::sync::mpsc;
use uuid::Uuid;

const COMMAND_CAPACITY: usize = 64;
const PENDING_OUTPUT_CHUNKS: usize = 32;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelnetConnectRequest {
    pub host: String,
    pub port: u16,
    #[serde(default = "default_terminal")]
    pub terminal: String,
    #[serde(default)]
    pub encoding: TelnetEncoding,
    #[serde(default = "default_columns")]
    pub columns: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelnetConnectResponse {
    pub terminal_id: String,
    pub host: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum TelnetSessionState {
    Connected,
    Reconnecting,
    Disconnected,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TelnetSessionEvent {
    terminal_id: String,
    state: TelnetSessionState,
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TelnetOutputEvent {
    pub(crate) terminal_id: String,
    pub(crate) data: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TelnetClosedEvent {
    terminal_id: String,
    reason: String,
}

enum TelnetCommand {
    Write(Vec<u8>),
    Resize { columns: u16, rows: u16 },
    Reconnect,
    Close,
}

struct TelnetSessionStateData {
    sender: mpsc::Sender<TelnetCommand>,
    attached: bool,
    pending_output: Vec<String>,
}

#[derive(Debug, Error)]
pub enum TelnetManagerError {
    #[error("Telnet session is not found: {0}")]
    MissingSession(String),
    #[error("Telnet session command queue is closed")]
    Closed,
    #[error("invalid Telnet request: {0}")]
    InvalidRequest(String),
    #[error(transparent)]
    Transport(#[from] TelnetError),
    #[error(transparent)]
    Input(#[from] TerminalInputError),
}

#[derive(Clone, Default)]
pub struct TelnetManager {
    sessions: Arc<Mutex<HashMap<String, TelnetSessionStateData>>>,
}

impl TelnetManager {
    pub async fn connect(
        &self,
        app: AppHandle,
        request: TelnetConnectRequest,
    ) -> Result<TelnetConnectResponse, TelnetManagerError> {
        let host = request.host.clone();
        let options = TelnetOptions {
            host: request.host,
            port: request.port,
            terminal: request.terminal,
            encoding: request.encoding,
            columns: request.columns,
            rows: request.rows,
            ..TelnetOptions::new("127.0.0.1", 23)
        };
        options
            .validate()
            .map_err(|error| TelnetManagerError::InvalidRequest(error.to_string()))?;
        let connection = TelnetConnection::connect(options).await?;
        let terminal_id = Uuid::new_v4().to_string();
        let (sender, receiver) = mpsc::channel(COMMAND_CAPACITY);
        self.sessions
            .lock()
            .map_err(|_| TelnetManagerError::Closed)?
            .insert(
                terminal_id.clone(),
                TelnetSessionStateData {
                    sender,
                    attached: false,
                    pending_output: Vec::new(),
                },
            );

        let manager = self.clone();
        let id_for_task = terminal_id.clone();
        tauri::async_runtime::spawn(async move {
            run_telnet_session(manager, app, id_for_task, connection, receiver).await;
        });

        Ok(TelnetConnectResponse { terminal_id, host })
    }

    pub async fn write(&self, terminal_id: &str, data: String) -> Result<(), TelnetManagerError> {
        validate_terminal_input(data.as_bytes())?;
        self.sender(terminal_id)?
            .send(TelnetCommand::Write(data.into_bytes()))
            .await
            .map_err(|_| TelnetManagerError::Closed)
    }

    pub async fn close(&self, terminal_id: &str) -> Result<(), TelnetManagerError> {
        self.sender(terminal_id)?
            .send(TelnetCommand::Close)
            .await
            .map_err(|_| TelnetManagerError::Closed)
    }

    pub async fn reconnect(&self, terminal_id: &str) -> Result<(), TelnetManagerError> {
        self.sender(terminal_id)?
            .send(TelnetCommand::Reconnect)
            .await
            .map_err(|_| TelnetManagerError::Closed)
    }

    pub async fn resize(
        &self,
        terminal_id: &str,
        columns: u16,
        rows: u16,
    ) -> Result<(), TelnetManagerError> {
        if columns == 0 || rows == 0 {
            return Err(TelnetManagerError::InvalidRequest(
                "Telnet dimensions must be positive".into(),
            ));
        }
        self.sender(terminal_id)?
            .send(TelnetCommand::Resize { columns, rows })
            .await
            .map_err(|_| TelnetManagerError::Closed)
    }

    pub fn attach(
        &self,
        terminal_id: &str,
        mut emit: impl FnMut(String),
    ) -> Result<(), TelnetManagerError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| TelnetManagerError::Closed)?;
        let state = sessions
            .get_mut(terminal_id)
            .ok_or_else(|| TelnetManagerError::MissingSession(terminal_id.to_owned()))?;
        // Emit the backlog under the session lock before enabling live output.
        // Returning it separately races newer events against the IPC response.
        for data in std::mem::take(&mut state.pending_output) {
            emit(data);
        }
        state.attached = true;
        Ok(())
    }

    fn sender(&self, terminal_id: &str) -> Result<mpsc::Sender<TelnetCommand>, TelnetManagerError> {
        self.sessions
            .lock()
            .map_err(|_| TelnetManagerError::Closed)?
            .get(terminal_id)
            .map(|state| state.sender.clone())
            .ok_or_else(|| TelnetManagerError::MissingSession(terminal_id.to_owned()))
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
                "telnet://output",
                TelnetOutputEvent {
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
        state: TelnetSessionState,
        error: Option<String>,
    ) {
        let _ = app.emit(
            "telnet://state",
            TelnetSessionEvent {
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

async fn run_telnet_session(
    manager: TelnetManager,
    app: AppHandle,
    terminal_id: String,
    mut connection: TelnetConnection,
    mut commands: mpsc::Receiver<TelnetCommand>,
) {
    manager.emit_state(&app, &terminal_id, TelnetSessionState::Connected, None);
    let mut output_decoder = Utf8OutputDecoder::default();
    let mut buffer = vec![0_u8; 16 * 1024];
    let reason = 'session: loop {
        tokio::select! {
            read = connection.read(&mut buffer), if connection.state() == ConnectionState::Connected => {
                match read {
                    Ok(0) => {
                        manager.emit_state(
                            &app,
                            &terminal_id,
                            TelnetSessionState::Reconnecting,
                            Some("remote disconnected".to_owned()),
                        );
                        continue 'session;
                    }
                    Ok(bytes) => {
                        let text = match connection.encoding() {
                            TelnetEncoding::Utf8 => output_decoder.push(&buffer[..bytes]),
                            encoding => encoding.decode(&buffer[..bytes]),
                        };
                        if !text.is_empty() {
                            manager.publish_output(&app, &terminal_id, text);
                        }
                    }
                    Err(error) => {
                        let reason = error.to_string();
                        let _ = connection.mark_connection_lost();
                        manager.emit_state(
                            &app,
                            &terminal_id,
                            TelnetSessionState::Reconnecting,
                            Some(reason),
                        );
                        continue 'session;
                    }
                }
            }
            command = commands.recv() => {
                match command {
                    Some(TelnetCommand::Write(data)) => {
                        if connection.state() != ConnectionState::Connected {
                            continue;
                        }
                        if let Err(error) = connection.write(&data).await {
                            let reason = error.to_string();
                            let _ = connection.mark_connection_lost();
                            manager.emit_state(
                                &app,
                                &terminal_id,
                                TelnetSessionState::Reconnecting,
                                Some(reason),
                            );
                        }
                    }
                    Some(TelnetCommand::Resize { columns, rows }) => {
                        if connection.state() != ConnectionState::Connected {
                            continue;
                        }
                        if let Err(error) = connection.resize(columns, rows).await {
                            let reason = error.to_string();
                            let _ = connection.mark_connection_lost();
                            manager.emit_state(
                                &app,
                                &terminal_id,
                                TelnetSessionState::Reconnecting,
                                Some(reason),
                            );
                        }
                    }
                    Some(TelnetCommand::Reconnect) => {
                        if connection.state() == ConnectionState::Connected {
                            continue;
                        }
                        manager.emit_state(&app, &terminal_id, TelnetSessionState::Reconnecting, None);
                        match connection.reconnect().await {
                            Ok(()) => {
                                let tail = output_decoder.finish();
                                if !tail.is_empty() {
                                    manager.publish_output(&app, &terminal_id, tail);
                                }
                                manager.emit_state(&app, &terminal_id, TelnetSessionState::Connected, None);
                            }
                            Err(error) => manager.emit_state(&app, &terminal_id, TelnetSessionState::Failed, Some(error.to_string())),
                        }
                    }
                    Some(TelnetCommand::Close) | None => {
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
    manager.emit_state(&app, &terminal_id, TelnetSessionState::Disconnected, None);
    let _ = app.emit(
        "telnet://closed",
        TelnetClosedEvent {
            terminal_id: terminal_id.clone(),
            reason,
        },
    );
    manager.remove(&terminal_id);
}

fn default_terminal() -> String {
    "xterm-256color".into()
}

fn default_columns() -> u16 {
    120
}

fn default_rows() -> u16 {
    32
}

#[cfg(test)]
mod tests {
    use super::{TelnetManager, TelnetManagerError};

    #[test]
    fn attach_replays_pending_output_before_live_output() {
        let manager = TelnetManager::default();
        let chunks = vec!["\x1b[3".to_owned(), "2mready ☃\x1b[0m\r\n".to_owned()];
        let (sender, _commands) = tokio::sync::mpsc::channel(super::COMMAND_CAPACITY);
        manager.sessions.lock().unwrap().insert(
            "fixture".into(),
            super::TelnetSessionStateData {
                sender,
                attached: false,
                pending_output: chunks.clone(),
            },
        );
        let mut emitted = Vec::new();
        manager
            .attach("fixture", |data| {
                assert!(
                    matches!(
                        manager.sessions.try_lock(),
                        Err(std::sync::TryLockError::WouldBlock)
                    ),
                    "live publishers cannot enter while the backlog is being emitted"
                );
                emitted.push(data);
            })
            .unwrap();
        assert_eq!(
            emitted, chunks,
            "preserve fragmented ANSI and Unicode output in order"
        );
        let sessions = manager.sessions.lock().unwrap();
        assert!(sessions["fixture"].attached);
        assert!(sessions["fixture"].pending_output.is_empty());
        drop(sessions);
        manager
            .attach("fixture", |_| panic!("backlog must not be replayed twice"))
            .unwrap();
        assert!(matches!(
            manager.attach("missing", |_| panic!("missing session cannot emit")),
            Err(TelnetManagerError::MissingSession(_))
        ));
    }

    #[tokio::test]
    async fn oversized_telnet_write_is_rejected_before_session_lookup() {
        let data = "x".repeat(mobarust_core::MAX_TERMINAL_INPUT_BYTES + 1);
        let error = TelnetManager::default()
            .write("missing", data)
            .await
            .expect_err("oversized Telnet input must be rejected");
        assert!(matches!(
            error,
            TelnetManagerError::Input(mobarust_core::TerminalInputError::TooLarge)
        ));
    }
}
