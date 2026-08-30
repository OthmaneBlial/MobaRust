use mobarust_core::OutputBatcher;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use thiserror::Error;
use uuid::Uuid;

const OUTPUT_BATCH_BYTES: usize = 32 * 1024;
const OUTPUT_CHANNEL_CAPACITY: usize = 64;

#[derive(Debug, Error)]
pub enum TerminalError {
    #[error("failed to create pseudo-terminal: {0}")]
    Open(#[source] anyhow::Error),
    #[error("terminal session not found: {0}")]
    Missing(String),
    #[error("terminal I/O failed: {0}")]
    Io(#[source] std::io::Error),
    #[error("terminal resize failed: {0}")]
    Resize(#[source] anyhow::Error),
    #[error("local terminal target is not available on this platform")]
    UnsupportedTarget,
    #[error("WSL distribution name is invalid")]
    InvalidWslDistribution,
    #[cfg(target_os = "windows")]
    #[error("WSL distribution discovery timed out")]
    WslDiscoveryTimeout,
    #[cfg(target_os = "windows")]
    #[error("WSL distribution discovery failed: {0}")]
    WslDiscovery(#[source] std::io::Error),
    #[cfg(target_os = "windows")]
    #[error("WSL distribution discovery returned an error: {0}")]
    WslDiscoveryStatus(String),
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LocalTerminalTarget {
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "wsl")]
    Wsl { distribution: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalOutput {
    terminal_id: String,
    data: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalClosed {
    terminal_id: String,
}

struct TerminalSession {
    master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
}

#[derive(Clone, Default)]
pub struct TerminalManager {
    sessions: Arc<Mutex<HashMap<String, Arc<TerminalSession>>>>,
}

impl TerminalManager {
    pub fn spawn(
        &self,
        app: AppHandle,
        cols: u16,
        rows: u16,
        target: LocalTerminalTarget,
    ) -> Result<String, TerminalError> {
        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(portable_pty::PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| TerminalError::Open(anyhow::anyhow!(error)))?;

        let mut command = match target {
            LocalTerminalTarget::Default => {
                let shell = default_shell();
                portable_pty::CommandBuilder::new(&shell)
            }
            LocalTerminalTarget::Wsl { distribution } => {
                let distribution = validate_wsl_distribution(&distribution)?;
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = distribution;
                    return Err(TerminalError::UnsupportedTarget);
                }
                #[cfg(target_os = "windows")]
                {
                    let mut command = portable_pty::CommandBuilder::new("wsl.exe");
                    command.arg("--distribution");
                    command.arg(distribution);
                    command
                }
            }
        };
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");

        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| TerminalError::Open(anyhow::anyhow!(error)))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| TerminalError::Open(anyhow::anyhow!(error)))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| TerminalError::Open(anyhow::anyhow!(error)))?;
        let id = Uuid::new_v4().to_string();
        let session = Arc::new(TerminalSession {
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            child: Mutex::new(child),
        });

        self.sessions
            .lock()
            .expect("terminal session map poisoned")
            .insert(id.clone(), Arc::clone(&session));

        let manager = self.clone();
        let terminal_id = id.clone();
        thread::Builder::new()
            .name(format!("mobarust-pty-{id}"))
            .spawn(move || stream_output(app, manager, terminal_id, reader))
            .map_err(TerminalError::Io)?;

        Ok(id)
    }

    pub fn write(&self, id: &str, data: &[u8]) -> Result<(), TerminalError> {
        let session = self.session(id)?;
        let mut writer = session.writer.lock().expect("terminal writer poisoned");
        writer.write_all(data).map_err(TerminalError::Io)?;
        writer.flush().map_err(TerminalError::Io)
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), TerminalError> {
        let session = self.session(id)?;
        session
            .master
            .lock()
            .expect("terminal master poisoned")
            .resize(portable_pty::PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| TerminalError::Resize(anyhow::anyhow!(error)))
    }

    pub fn close(&self, id: &str) -> Result<(), TerminalError> {
        let session = self
            .sessions
            .lock()
            .expect("terminal session map poisoned")
            .remove(id)
            .ok_or_else(|| TerminalError::Missing(id.to_owned()))?;
        session
            .child
            .lock()
            .expect("terminal child poisoned")
            .kill()
            .map_err(TerminalError::Io)
    }

    fn session(&self, id: &str) -> Result<Arc<TerminalSession>, TerminalError> {
        self.sessions
            .lock()
            .expect("terminal session map poisoned")
            .get(id)
            .cloned()
            .ok_or_else(|| TerminalError::Missing(id.to_owned()))
    }
}

fn validate_wsl_distribution(distribution: &str) -> Result<String, TerminalError> {
    if distribution.chars().any(char::is_control) {
        return Err(TerminalError::InvalidWslDistribution);
    }
    let distribution = distribution.trim();
    if distribution.is_empty() || distribution.len() > 128 || distribution.starts_with('-') {
        return Err(TerminalError::InvalidWslDistribution);
    }
    Ok(distribution.to_owned())
}

#[cfg(any(target_os = "windows", test))]
fn parse_wsl_distributions(bytes: &[u8]) -> Vec<String> {
    let text = if bytes.starts_with(&[0xff, 0xfe]) {
        let units = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
        std::char::decode_utf16(units)
            .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect::<String>()
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        let units = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]));
        std::char::decode_utf16(units)
            .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect::<String>()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    let mut distributions = Vec::new();
    for line in text.lines() {
        let name = line
            .trim_matches('\0')
            .trim()
            .trim_start_matches('\u{feff}')
            .trim_start_matches('*')
            .trim();
        if name.is_empty()
            || validate_wsl_distribution(name).is_err()
            || distributions.iter().any(|item| item == name)
        {
            continue;
        }
        distributions.push(name.to_owned());
    }
    distributions
}

/// Discover installed WSL distributions without invoking a shell.
///
/// The non-Windows branch returns an explicit unsupported error and performs
/// no process or filesystem access. Windows callers get a short bounded
/// `wsl.exe --list --quiet` query; the frontend can only launch names returned
/// by this query.
pub async fn list_wsl_distributions() -> Result<Vec<String>, TerminalError> {
    #[cfg(not(target_os = "windows"))]
    {
        Err(TerminalError::UnsupportedTarget)
    }

    #[cfg(target_os = "windows")]
    {
        use std::process::Stdio;
        use tokio::process::Command;

        let output = tokio::time::timeout(
            Duration::from_secs(3),
            Command::new("wsl.exe")
                .args(["--list", "--quiet"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output(),
        )
        .await
        .map_err(|_| TerminalError::WslDiscoveryTimeout)?
        .map_err(TerminalError::WslDiscovery)?;

        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return Err(TerminalError::WslDiscoveryStatus(if detail.is_empty() {
                format!("process exited with {}", output.status)
            } else {
                detail
            }));
        }
        Ok(parse_wsl_distributions(&output.stdout))
    }
}

fn stream_output<R: Read + Send + 'static>(
    app: AppHandle,
    manager: TerminalManager,
    terminal_id: String,
    mut reader: R,
) {
    let (sender, receiver) = mpsc::sync_channel::<Vec<u8>>(OUTPUT_CHANNEL_CAPACITY);
    let reader_thread = thread::Builder::new()
        .name(format!("mobarust-pty-reader-{terminal_id}"))
        .spawn(move || {
            let mut buffer = vec![0_u8; OUTPUT_BATCH_BYTES];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(size) => {
                        if sender.send(buffer[..size].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

    if reader_thread.is_err() {
        let _ = app.emit(
            "terminal://closed",
            TerminalClosed {
                terminal_id: terminal_id.clone(),
            },
        );
        manager.remove(&terminal_id);
        return;
    }

    let mut batcher = OutputBatcher::new(OUTPUT_BATCH_BYTES);
    loop {
        match receiver.recv_timeout(Duration::from_millis(8)) {
            Ok(bytes) => {
                for chunk in batcher.push(&bytes) {
                    emit_chunk(&app, &terminal_id, chunk.bytes);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(chunk) = batcher.flush() {
                    emit_chunk(&app, &terminal_id, chunk.bytes);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Some(chunk) = batcher.flush() {
                    emit_chunk(&app, &terminal_id, chunk.bytes);
                }
                break;
            }
        }
    }

    let _ = app.emit(
        "terminal://closed",
        TerminalClosed {
            terminal_id: terminal_id.clone(),
        },
    );
    manager.remove(&terminal_id);
}

fn emit_chunk(app: &AppHandle, terminal_id: &str, bytes: Vec<u8>) {
    let _ = app.emit(
        "terminal://output",
        TerminalOutput {
            terminal_id: terminal_id.to_owned(),
            data: String::from_utf8_lossy(&bytes).into_owned(),
        },
    );
}

impl TerminalManager {
    fn remove(&self, id: &str) {
        self.sessions
            .lock()
            .expect("terminal session map poisoned")
            .remove(id);
    }
}

#[cfg(target_os = "windows")]
fn default_shell() -> String {
    std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_owned())
}

#[cfg(not(target_os = "windows"))]
fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())
}

#[cfg(unix)]
#[cfg(test)]
mod tests {
    use super::*;
    use portable_pty::{CommandBuilder, PtySize};

    #[test]
    fn native_pty_supports_resize_input_output_and_exit() {
        let system = portable_pty::native_pty_system();
        let pair = system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open test pty");

        pair.master
            .resize(PtySize {
                rows: 40,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("resize test pty");

        let mut command = CommandBuilder::new("/bin/sh");
        command.args([
            "-c",
            "printf 'MOBARUST_PTY_OK\\n'; read line; printf 'INPUT:%s\\n' \"$line\"",
        ]);
        let mut child = pair.slave.spawn_command(command).expect("spawn test shell");
        let mut reader = pair.master.try_clone_reader().expect("clone test reader");
        let mut writer = pair.master.take_writer().expect("take test writer");
        writer.write_all(b"hello\n").expect("write test input");
        writer.flush().expect("flush test input");

        let mut output = String::new();
        reader
            .read_to_string(&mut output)
            .expect("read test pty output");
        child.wait().expect("wait for test shell");

        assert!(output.contains("MOBARUST_PTY_OK"));
        assert!(output.contains("INPUT:hello"));
    }

    #[test]
    fn wsl_listing_normalizes_windows_output_without_accepting_options() {
        let output = b"\xff\xfeU\0b\0u\0n\0t\0u\0\r\0\n\0*\0D\0e\0b\0i\0a\0n\0\r\0\n\0-\0u\0n\0s\0a\0f\0e\0\r\0\n\0";
        let distributions = parse_wsl_distributions(output);

        assert_eq!(distributions, vec!["Ubuntu", "Debian"]);
    }

    #[test]
    fn wsl_distribution_validation_rejects_control_and_option_like_names() {
        assert!(validate_wsl_distribution("Ubuntu\n").is_err());
        assert!(validate_wsl_distribution("--root").is_err());
        assert_eq!(validate_wsl_distribution(" Ubuntu ").unwrap(), "Ubuntu");
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn wsl_discovery_is_not_attempted_on_non_windows() {
        assert!(matches!(
            list_wsl_distributions().await,
            Err(TerminalError::UnsupportedTarget)
        ));
    }
}
