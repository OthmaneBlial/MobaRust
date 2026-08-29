use mobarust_network::{
    NetworkDiagnosticError, PortScanOptions, TcpCheckResult, scan_tcp_with_progress,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use thiserror::Error;
use tokio::sync::watch;
use uuid::Uuid;

const MIN_TIMEOUT_MS: u64 = 50;
const MAX_TIMEOUT_MS: u64 = 60_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkScanRequest {
    pub host: String,
    pub start_port: u16,
    pub end_port: u16,
    pub concurrency: usize,
    pub timeout_ms: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkScanResponse {
    pub scan_id: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum NetworkScanState {
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NetworkScanEvent {
    scan_id: String,
    state: NetworkScanState,
    scanned: usize,
    total: usize,
    result: Option<TcpCheckResult>,
    error: Option<String>,
}

#[derive(Debug, Error)]
pub enum NetworkManagerError {
    #[error("network scan is not found: {0}")]
    MissingScan(String),
    #[error("network scan manager lock poisoned")]
    LockPoisoned,
    #[error(
        "network scan timeout must be between {MIN_TIMEOUT_MS} and {MAX_TIMEOUT_MS} milliseconds"
    )]
    InvalidTimeout,
    #[error(transparent)]
    Diagnostic(#[from] NetworkDiagnosticError),
}

#[derive(Clone, Default)]
pub struct NetworkManager {
    scans: Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
}

impl NetworkManager {
    pub async fn start_scan(
        &self,
        app: AppHandle,
        request: NetworkScanRequest,
    ) -> Result<NetworkScanResponse, NetworkManagerError> {
        if !(MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(&request.timeout_ms) {
            return Err(NetworkManagerError::InvalidTimeout);
        }
        let options = PortScanOptions {
            host: request.host,
            start_port: request.start_port,
            end_port: request.end_port,
            concurrency: request.concurrency,
            timeout: Duration::from_millis(request.timeout_ms),
        };
        options.validate()?;
        let total = usize::from(options.end_port - options.start_port) + 1;
        let scan_id = Uuid::new_v4().to_string();
        let (cancel_sender, mut cancellation) = watch::channel(false);
        self.scans
            .lock()
            .map_err(|_| NetworkManagerError::LockPoisoned)?
            .insert(scan_id.clone(), cancel_sender);

        emit_scan_event(
            &app,
            NetworkScanEvent {
                scan_id: scan_id.clone(),
                state: NetworkScanState::Running,
                scanned: 0,
                total,
                result: None,
                error: None,
            },
        );

        let manager = self.clone();
        let id_for_task = scan_id.clone();
        let last_scanned = Arc::new(AtomicUsize::new(0));
        let progress_scanned = Arc::clone(&last_scanned);
        tauri::async_runtime::spawn(async move {
            let progress_app = app.clone();
            let progress_id = id_for_task.clone();
            let result = scan_tcp_with_progress(
                options,
                &mut cancellation,
                move |port_result, scanned, total| {
                    progress_scanned.store(scanned, Ordering::Relaxed);
                    emit_scan_event(
                        &progress_app,
                        NetworkScanEvent {
                            scan_id: progress_id.clone(),
                            state: NetworkScanState::Running,
                            scanned,
                            total,
                            result: Some(port_result.clone()),
                            error: None,
                        },
                    );
                },
            )
            .await;

            match result {
                Ok(results) => emit_scan_event(
                    &app,
                    NetworkScanEvent {
                        scan_id: id_for_task.clone(),
                        state: NetworkScanState::Completed,
                        scanned: results.len(),
                        total,
                        result: None,
                        error: None,
                    },
                ),
                Err(NetworkDiagnosticError::Cancelled) => emit_scan_event(
                    &app,
                    NetworkScanEvent {
                        scan_id: id_for_task.clone(),
                        state: NetworkScanState::Cancelled,
                        scanned: last_scanned.load(Ordering::Relaxed),
                        total,
                        result: None,
                        error: None,
                    },
                ),
                Err(error) => emit_scan_event(
                    &app,
                    NetworkScanEvent {
                        scan_id: id_for_task.clone(),
                        state: NetworkScanState::Failed,
                        scanned: last_scanned.load(Ordering::Relaxed),
                        total,
                        result: None,
                        error: Some(error.to_string()),
                    },
                ),
            }
            manager.remove(&id_for_task);
        });

        Ok(NetworkScanResponse { scan_id })
    }

    pub fn cancel_scan(&self, scan_id: &str) -> Result<bool, NetworkManagerError> {
        let sender = self
            .scans
            .lock()
            .map_err(|_| NetworkManagerError::LockPoisoned)?
            .get(scan_id)
            .cloned();
        let Some(sender) = sender else {
            return Ok(false);
        };
        sender
            .send(true)
            .map_err(|_| NetworkManagerError::MissingScan(scan_id.to_owned()))?;
        Ok(true)
    }

    fn remove(&self, scan_id: &str) {
        if let Ok(mut scans) = self.scans.lock() {
            scans.remove(scan_id);
        }
    }
}

fn emit_scan_event(app: &AppHandle, event: NetworkScanEvent) {
    let _ = app.emit("network://scan", event);
}
