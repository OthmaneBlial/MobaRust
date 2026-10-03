use mobarust_core::{
    OutputBatcher, TerminalInputError, Utf8OutputDecoder, validate_session_environment,
    validate_session_startup, validate_terminal_input,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use thiserror::Error;
use uuid::Uuid;

const OUTPUT_BATCH_BYTES: usize = 32 * 1024;
const OUTPUT_CHANNEL_CAPACITY: usize = 64;
const ATTACH_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum TerminalError {
    #[error("failed to create pseudo-terminal")]
    Open(#[source] anyhow::Error),
    #[error("terminal session not found")]
    Missing(String),
    #[error("terminal session is already attached")]
    AlreadyAttached,
    #[error("MobaRust is closing; no new local terminal can be opened")]
    ShuttingDown,
    #[error("local terminal shutdown cleanup failed")]
    ShutdownIncomplete,
    #[error("local terminal state is unavailable")]
    LockPoisoned,
    #[error("terminal I/O failed")]
    Io(#[source] std::io::Error),
    #[error("terminal input is busy; check the terminal before retrying")]
    InputBusy,
    #[error("terminal input worker failed; check the terminal before retrying")]
    InputWorker,
    #[error("terminal resize failed")]
    Resize(#[source] anyhow::Error),
    #[error("terminal process cleanup failed")]
    Wait(#[source] std::io::Error),
    #[error("local terminal target is not available on this platform")]
    UnsupportedTarget,
    #[error("WSL distribution name is invalid")]
    InvalidWslDistribution,
    #[error("local terminal working directory is invalid")]
    InvalidWorkingDirectory,
    #[error("local terminal environment is invalid")]
    InvalidEnvironment,
    #[error("local terminal startup command is invalid")]
    InvalidStartupCommand,
    #[error(transparent)]
    Input(#[from] TerminalInputError),
    #[cfg(target_os = "windows")]
    #[error("WSL distribution discovery timed out")]
    WslDiscoveryTimeout,
    #[cfg(target_os = "windows")]
    #[error("WSL distribution discovery failed")]
    WslDiscovery(#[source] std::io::Error),
    #[cfg(target_os = "windows")]
    #[error("WSL distribution discovery returned a non-zero status")]
    WslDiscoveryStatus,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum LocalTerminalTarget {
    #[serde(rename = "default")]
    Default {
        #[serde(default)]
        shell: LocalShell,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        environment: Vec<(String, String)>,
        #[serde(default)]
        #[serde(rename = "startupCommand")]
        startup_command: Option<String>,
    },
    #[serde(rename = "wsl")]
    Wsl {
        distribution: String,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        environment: Vec<(String, String)>,
        #[serde(default)]
        #[serde(rename = "startupCommand")]
        startup_command: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LocalShell {
    #[serde(rename = "default")]
    #[default]
    Default,
    #[serde(rename = "powershell")]
    PowerShell,
    #[serde(rename = "cmd")]
    Cmd,
    #[serde(rename = "bash")]
    Bash,
    #[serde(rename = "zsh")]
    Zsh,
    #[serde(rename = "fish")]
    Fish,
}

impl LocalTerminalTarget {
    fn validate(&self) -> Result<(), TerminalError> {
        let (cwd, environment, startup_command) = match self {
            Self::Default {
                shell: _,
                cwd,
                environment,
                startup_command,
            }
            | Self::Wsl {
                cwd,
                environment,
                startup_command,
                ..
            } => (cwd, environment, startup_command),
        };
        validate_session_startup(cwd.as_deref(), None)
            .map_err(|_| TerminalError::InvalidWorkingDirectory)?;
        validate_session_startup(None, startup_command.as_deref())
            .map_err(|_| TerminalError::InvalidStartupCommand)?;
        validate_session_environment(environment).map_err(|_| TerminalError::InvalidEnvironment)
    }
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
    closing: AtomicBool,
    master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    writer: Arc<tokio::sync::Mutex<Box<dyn Write + Send>>>,
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
    start: Mutex<Option<TerminalStart>>,
}

struct TerminalStart {
    ready: mpsc::Sender<()>,
    startup_command: Option<String>,
}

#[derive(Clone, Copy)]
enum ShutdownPhase {
    Open,
    Closing,
    Complete { failed: bool },
}

#[derive(Clone)]
pub struct TerminalManager {
    sessions: Arc<Mutex<HashMap<String, Arc<TerminalSession>>>>,
    shutdown: tokio::sync::watch::Sender<ShutdownPhase>,
}

impl Default for TerminalManager {
    fn default() -> Self {
        Self {
            sessions: Arc::default(),
            shutdown: tokio::sync::watch::channel(ShutdownPhase::Open).0,
        }
    }
}

impl TerminalManager {
    pub fn spawn(
        &self,
        app: AppHandle,
        cols: u16,
        rows: u16,
        target: LocalTerminalTarget,
    ) -> Result<String, TerminalError> {
        self.ensure_open()?;
        target.validate()?;
        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(portable_pty::PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| TerminalError::Open(anyhow::anyhow!(error)))?;

        let (mut command, startup_command) = match target {
            LocalTerminalTarget::Default {
                shell,
                cwd,
                environment,
                startup_command,
            } => {
                let shell = shell_command(shell)?;
                let mut command = portable_pty::CommandBuilder::new(&shell);
                if let Some(cwd) = cwd {
                    command.cwd(cwd);
                }
                for (name, value) in environment {
                    command.env(name, value);
                }
                (command, startup_command)
            }
            LocalTerminalTarget::Wsl {
                distribution,
                cwd,
                environment,
                startup_command,
            } => {
                let distribution = validate_wsl_distribution(&distribution)?;
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = (distribution, cwd, environment, startup_command);
                    return Err(TerminalError::UnsupportedTarget);
                }
                #[cfg(target_os = "windows")]
                {
                    let mut command = portable_pty::CommandBuilder::new("wsl.exe");
                    command.arg("--distribution");
                    command.arg(distribution);
                    if let Some(cwd) = cwd {
                        command.arg("--cd");
                        command.arg(cwd);
                    }
                    for (name, value) in environment {
                        command.env(name, value);
                    }
                    (command, startup_command)
                }
            }
        };
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");

        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| TerminalError::Open(anyhow::anyhow!(error)))?;
        let reader = match pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(error) => {
                let _ = cleanup_child(child.as_mut());
                return Err(TerminalError::Open(anyhow::anyhow!(error)));
            }
        };
        let writer = match pair.master.take_writer() {
            Ok(writer) => writer,
            Err(error) => {
                let _ = cleanup_child(child.as_mut());
                return Err(TerminalError::Open(anyhow::anyhow!(error)));
            }
        };
        let id = Uuid::new_v4().to_string();
        let (start, ready) = mpsc::channel();
        let session = Arc::new(TerminalSession {
            closing: AtomicBool::new(false),
            master: Mutex::new(pair.master),
            writer: Arc::new(tokio::sync::Mutex::new(writer)),
            child: Mutex::new(child),
            start: Mutex::new(Some(TerminalStart {
                ready: start,
                startup_command,
            })),
        });

        self.register_session(&id, Arc::clone(&session))?;

        let manager = self.clone();
        let terminal_id = id.clone();
        // The frontend must know the ID before any PTY output can be emitted.
        if let Err(error) = thread::Builder::new()
            .name(format!("mobarust-pty-{id}"))
            .spawn(move || match ready.recv_timeout(ATTACH_TIMEOUT) {
                Ok(()) => stream_output(app, manager, terminal_id, reader),
                Err(_) => {
                    cleanup_stream_session(&manager, &terminal_id);
                    let _ = app.emit("terminal://closed", TerminalClosed { terminal_id });
                }
            })
        {
            // The session is inserted before the stream worker starts so the
            // worker can race safely with an immediate close. If the OS
            // refuses the worker, close the session and reap its child
            // instead of leaving a native process behind.
            let _ = self.close(&id);
            return Err(TerminalError::Io(error));
        }

        Ok(id)
    }

    pub async fn attach(&self, id: &str) -> Result<(), TerminalError> {
        let session = self.session(id)?;
        let start = session
            .start
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .take()
            .ok_or(TerminalError::AlreadyAttached)?;
        if start.ready.send(()).is_err() {
            let _ = self.close(id);
            return Err(TerminalError::Io(std::io::ErrorKind::BrokenPipe.into()));
        }
        // Start the output reader before sending saved input. The ID is
        // already published, so Close can interrupt a blocked startup write.
        if let Some(command) = start.startup_command {
            let mut data = command.into_bytes();
            data.push(b'\r');
            if let Err(error) = self.write(id, data).await {
                let _ = self.close(id);
                return Err(error);
            }
        }
        Ok(())
    }

    pub async fn write(&self, id: &str, data: Vec<u8>) -> Result<(), TerminalError> {
        validate_terminal_input(&data)?;
        let session = self.session(id)?;
        // Reserve before spawning: even direct IPC callers cannot accumulate
        // blocking tasks behind a non-reading child. The frontend orders input.
        let mut writer = Arc::clone(&session.writer)
            .try_lock_owned()
            .map_err(|_| TerminalError::InputBusy)?;
        tauri::async_runtime::spawn_blocking(move || {
            writer.write_all(&data).map_err(TerminalError::Io)?;
            writer.flush().map_err(TerminalError::Io)
        })
        .await
        .map_err(|_| TerminalError::InputWorker)?
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), TerminalError> {
        let session = self.session(id)?;
        session
            .master
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .resize(portable_pty::PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| TerminalError::Resize(anyhow::anyhow!(error)))
    }

    pub fn close(&self, id: &str) -> Result<(), TerminalError> {
        let session = self.session(id)?;
        // Keep the cleanup owner visible to shutdown until the child is reaped.
        cleanup_session(&session)?;
        self.take_session(id)?;
        Ok(())
    }

    fn ensure_open(&self) -> Result<(), TerminalError> {
        if matches!(*self.shutdown.borrow(), ShutdownPhase::Open) {
            Ok(())
        } else {
            Err(TerminalError::ShuttingDown)
        }
    }

    fn register_session(
        &self,
        id: &str,
        session: Arc<TerminalSession>,
    ) -> Result<(), TerminalError> {
        let result = self
            .sessions
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)
            .and_then(|mut sessions| {
                self.ensure_open()?;
                sessions.insert(id.to_owned(), Arc::clone(&session));
                Ok(())
            });
        if result.is_err() {
            let _ = cleanup_session(&session);
        }
        result
    }

    /// Seal the manager immediately; every caller awaits the same cleanup.
    pub fn shutdown(&self) -> impl Future<Output = Result<(), TerminalError>> + Send + use<> {
        let mut finished = self.shutdown.subscribe();
        let first = self.shutdown.send_if_modified(|phase| {
            if matches!(phase, ShutdownPhase::Open) {
                *phase = ShutdownPhase::Closing;
                true
            } else {
                false
            }
        });
        if first {
            let manager = self.clone();
            // Use a dedicated worker: blocked input may occupy the runtime's
            // blocking pool, but shutdown must still be able to kill its children.
            if thread::Builder::new()
                .name("mobarust-pty-shutdown".into())
                .spawn(move || manager.finish_shutdown())
                .is_err()
            {
                self.finish_shutdown();
            }
        }
        async move {
            loop {
                let phase = *finished.borrow();
                if let ShutdownPhase::Complete { failed } = phase {
                    return if failed {
                        Err(TerminalError::ShutdownIncomplete)
                    } else {
                        Ok(())
                    };
                }
                finished
                    .changed()
                    .await
                    .map_err(|_| TerminalError::ShutdownIncomplete)?;
            }
        }
    }

    fn finish_shutdown(&self) {
        let sessions = std::mem::take(
            &mut *self
                .sessions
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        let mut failed = false;
        for session in sessions.into_values() {
            if let Err(error) = cleanup_session(&session) {
                failed = true;
                tracing::warn!(event = "local_terminal_cleanup_failed", error = %error);
            }
        }
        self.shutdown
            .send_replace(ShutdownPhase::Complete { failed });
    }

    fn session(&self, id: &str) -> Result<Arc<TerminalSession>, TerminalError> {
        self.sessions
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .get(id)
            .filter(|session| !session.closing.load(Ordering::Acquire))
            .cloned()
            .ok_or_else(|| TerminalError::Missing(id.to_owned()))
    }

    fn take_session(&self, id: &str) -> Result<Option<Arc<TerminalSession>>, TerminalError> {
        self.sessions
            .lock()
            .map(|mut sessions| sessions.remove(id))
            .map_err(|_| TerminalError::LockPoisoned)
    }
}

/// Stop and reap a native PTY child under its child lock. Shared by close, worker-start
/// failure, and reader EOF so every local process has a deterministic owner.
fn cleanup_session(session: &TerminalSession) -> Result<(), TerminalError> {
    session.closing.store(true, Ordering::Release);
    // Cleanup must retain ownership even if an earlier operation panicked.
    session
        .start
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take();
    let mut child = session
        .child
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    cleanup_child(child.as_mut())
}

fn cleanup_child(child: &mut dyn portable_pty::Child) -> Result<(), TerminalError> {
    if child.try_wait().map_err(TerminalError::Io)?.is_some() {
        return Ok(());
    }

    // A PTY close is cooperative at the application boundary but must still
    // reap the native child. Treat a process that exited during the race
    // between try_wait and kill as already closed, then wait once so no
    // zombie or unreaped helper remains behind.
    match child.kill() {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // The child can exit between try_wait and kill. Waiting here still
            // reaps that child instead of abandoning the cleanup.
            child.wait().map_err(TerminalError::Wait)?;
            return Ok(());
        }
        Err(error) => return Err(TerminalError::Io(error)),
    }
    child.wait().map_err(TerminalError::Wait)?;
    Ok(())
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
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair));
        std::char::decode_utf16(units)
            .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect::<String>()
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        let units = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_be_bytes(*pair));
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
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| TerminalError::WslDiscoveryTimeout)?
        .map_err(TerminalError::WslDiscovery)?;

        if !output.status.success() {
            return Err(TerminalError::WslDiscoveryStatus);
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
        cleanup_stream_session(&manager, &terminal_id);
        let _ = app.emit(
            "terminal://closed",
            TerminalClosed {
                terminal_id: terminal_id.clone(),
            },
        );
        return;
    }

    let mut batcher = OutputBatcher::new(OUTPUT_BATCH_BYTES);
    let mut decoder = Utf8OutputDecoder::default();
    loop {
        match receiver.recv_timeout(Duration::from_millis(8)) {
            Ok(bytes) => {
                for chunk in batcher.push(&bytes) {
                    emit_chunk(&app, &terminal_id, &mut decoder, chunk.bytes);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(chunk) = batcher.flush() {
                    emit_chunk(&app, &terminal_id, &mut decoder, chunk.bytes);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Some(chunk) = batcher.flush() {
                    emit_chunk(&app, &terminal_id, &mut decoder, chunk.bytes);
                }
                break;
            }
        }
    }

    emit_text(&app, &terminal_id, decoder.finish());

    cleanup_stream_session(&manager, &terminal_id);
    let _ = app.emit(
        "terminal://closed",
        TerminalClosed {
            terminal_id: terminal_id.clone(),
        },
    );
}

fn cleanup_stream_session(manager: &TerminalManager, terminal_id: &str) {
    // Keep the cleanup owner registered until it is reaped, including at EOF.
    if let Err(error) = manager.close(terminal_id) {
        if !matches!(error, TerminalError::Missing(_)) {
            tracing::warn!(event = "local_terminal_stream_cleanup_failed", error = %error);
        }
    }
}

fn emit_chunk(app: &AppHandle, terminal_id: &str, decoder: &mut Utf8OutputDecoder, bytes: Vec<u8>) {
    emit_text(app, terminal_id, decoder.push(&bytes));
}

fn emit_text(app: &AppHandle, terminal_id: &str, data: String) {
    if data.is_empty() {
        return;
    }
    let _ = app.emit(
        "terminal://output",
        TerminalOutput {
            terminal_id: terminal_id.to_owned(),
            data,
        },
    );
}

#[cfg(target_os = "windows")]
fn default_shell() -> String {
    std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_owned())
}

#[cfg(not(target_os = "windows"))]
fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())
}

#[cfg(target_os = "windows")]
fn shell_command(shell: LocalShell) -> Result<String, TerminalError> {
    match shell {
        LocalShell::Default => Ok(default_shell()),
        LocalShell::PowerShell => Ok("powershell.exe".into()),
        LocalShell::Cmd => Ok("cmd.exe".into()),
        LocalShell::Bash | LocalShell::Zsh | LocalShell::Fish => {
            Err(TerminalError::UnsupportedTarget)
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn shell_command(shell: LocalShell) -> Result<String, TerminalError> {
    match shell {
        LocalShell::Default => Ok(default_shell()),
        LocalShell::Bash => Ok("bash".into()),
        LocalShell::Zsh => Ok("zsh".into()),
        LocalShell::Fish => Ok("fish".into()),
        LocalShell::PowerShell | LocalShell::Cmd => Err(TerminalError::UnsupportedTarget),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use portable_pty::{CommandBuilder, PtySize};

    #[test]
    fn poisoned_session_map_does_not_panic_terminal_lookup() {
        let manager = TerminalManager::default();
        let sessions = Arc::clone(&manager.sessions);
        assert!(
            thread::spawn(move || {
                let _sessions = sessions.lock().expect("lock test session map");
                panic!("poison test session map");
            })
            .join()
            .is_err()
        );

        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| manager.session("missing")));
        assert!(matches!(result, Ok(Err(TerminalError::LockPoisoned))));
    }

    #[test]
    fn native_pty_supports_resize_input_output_and_exit() {
        // ConPTY startup failed intermittently in CI. Every repetition must
        // pass; this is not a retry that hides a failed startup.
        let repetitions = if cfg!(target_os = "windows") { 3 } else { 1 };
        for _ in 0..repetitions {
            assert_native_pty_round_trip(fixture_command());
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn cmd_round_trips_input_and_exits_through_conpty() {
        for _ in 0..3 {
            let mut command = CommandBuilder::new("cmd.exe");
            command.args([
                "/D",
                "/Q",
                "/V:ON",
                "/C",
                "echo MOBARUST_PTY_OK & set /p line= & echo INPUT:!line! & exit /b 0",
            ]);
            assert_native_pty_round_trip(command);
        }
    }

    fn assert_native_pty_round_trip(command: CommandBuilder) {
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

        let mut child = pair.slave.spawn_command(command).expect("spawn test shell");
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().expect("clone test reader");
        let mut writer = pair.master.take_writer().expect("take test writer");

        let mut output = String::new();
        let (output_tx, output_rx) = mpsc::channel();
        let reader_thread = thread::spawn(move || {
            let mut buffer = [0; 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        let _ = output_tx.send(Err("PTY closed before expected output".into()));
                        break;
                    }
                    Ok(size) => {
                        if output_tx.send(Ok(buffer[..size].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = output_tx.send(Err(format!("PTY read failed: {error}")));
                        break;
                    }
                }
            }
        });

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            #[cfg(not(target_os = "windows"))]
            {
                writer.write_all(b"hello\n").expect("write test input");
                writer.flush().expect("flush test input");
            }

            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            #[cfg(target_os = "windows")]
            let mut sent_input = false;
            #[cfg(target_os = "windows")]
            let mut cursor_queries_replied = 0;
            while !output.contains("INPUT:hello") {
                #[cfg(target_os = "windows")]
                {
                    // PowerShell asks its host for cursor position before running the command.
                    let queries = output.match_indices("\u{1b}[6n").count();
                    while cursor_queries_replied < queries {
                        writer
                            .write_all(b"\x1b[1;1R")
                            .expect("reply to PTY cursor query");
                        writer.flush().expect("flush PTY cursor reply");
                        cursor_queries_replied += 1;
                    }
                    if output.contains("MOBARUST_PTY_OK") && !sent_input {
                        writer.write_all(b"hello\r").expect("write test input");
                        writer.flush().expect("flush test input");
                        sent_input = true;
                    }
                }
                match output_rx
                    .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                {
                    Ok(Ok(bytes)) => output.push_str(&String::from_utf8_lossy(&bytes)),
                    Ok(Err(error)) => panic!("{error}; captured PTY output: {output:?}"),
                    Err(_) => {
                        panic!("timed out waiting for test PTY output; captured: {output:?}");
                    }
                }
            }

            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let status = loop {
                if let Some(status) = child.try_wait().expect("poll test shell") {
                    break status;
                }
                if std::time::Instant::now() >= deadline {
                    panic!("test shell did not exit after writing its output");
                }
                thread::sleep(Duration::from_millis(10));
            };

            assert!(status.success());
            assert!(output.contains("MOBARUST_PTY_OK"), "{output:?}");
            assert!(output.contains("INPUT:hello"));
        }));
        // Close ConPTY and join its output reader before xtask removes the
        // disposable HOME, including when an assertion or deadline fails.
        let cleanup = cleanup_child(child.as_mut());
        drop(writer);
        drop(pair.master);
        drop(output_rx);
        let joined = reader_thread.join();
        if let Err(failure) = result {
            std::panic::resume_unwind(failure);
        }
        cleanup.expect("reap test shell");
        joined.expect("join test PTY reader");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn explicit_unix_shells_round_trip_through_a_native_pty_when_installed() {
        for shell in [LocalShell::Bash, LocalShell::Zsh, LocalShell::Fish] {
            let executable = shell_command(shell).expect("Unix shell should be supported");
            let Some(version) = executable_version(&executable) else {
                eprintln!("skipping native PTY shell fixture: {executable} is unavailable");
                continue;
            };

            let system = portable_pty::native_pty_system();
            let pair = system
                .openpty(PtySize {
                    rows: 24,
                    cols: 80,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .expect("open explicit shell pty");
            let _writer = pair
                .master
                .take_writer()
                .expect("take explicit shell writer");
            let mut command = CommandBuilder::new(&executable);
            command.args(["-c", "printf 'MOBARUST_EXPLICIT_SHELL_OK\\n'"]);
            let mut child = pair
                .slave
                .spawn_command(command)
                .expect("spawn explicit Unix shell");
            drop(pair.slave);
            let mut reader = pair
                .master
                .try_clone_reader()
                .expect("clone explicit shell reader");
            let mut output = String::new();
            reader
                .read_to_string(&mut output)
                .expect("read explicit shell output");
            let status = child.wait().expect("wait for explicit Unix shell");

            assert!(status.success(), "{executable} must exit successfully");
            assert!(
                output.contains("MOBARUST_EXPLICIT_SHELL_OK"),
                "{executable} did not round-trip through the PTY"
            );
            eprintln!("native PTY passed: {executable}; {version}");
        }
    }

    #[tokio::test]
    async fn attaching_then_closing_a_running_pty_reaps_the_fixture_child() {
        let system = portable_pty::native_pty_system();
        let pair = system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open cleanup test pty");
        let writer = pair.master.take_writer().expect("take cleanup writer");
        let child = pair
            .slave
            .spawn_command(long_running_fixture_command())
            .expect("spawn cleanup fixture");
        let (start, ready) = mpsc::channel();
        let manager = TerminalManager::default();
        manager
            .sessions
            .lock()
            .expect("lock cleanup sessions")
            .insert(
                "cleanup-fixture".into(),
                Arc::new(TerminalSession {
                    closing: AtomicBool::new(false),
                    master: Mutex::new(pair.master),
                    writer: Arc::new(tokio::sync::Mutex::new(writer)),
                    child: Mutex::new(child),
                    start: Mutex::new(Some(TerminalStart {
                        ready: start,
                        startup_command: None,
                    })),
                }),
            );

        manager
            .attach("cleanup-fixture")
            .await
            .expect("attach fixture");
        ready
            .recv_timeout(Duration::from_secs(1))
            .expect("release reader after attach");
        assert!(matches!(
            manager.attach("cleanup-fixture").await,
            Err(TerminalError::AlreadyAttached)
        ));
        manager
            .close("cleanup-fixture")
            .expect("close should reap the fixture child");
        assert!(
            manager
                .sessions
                .lock()
                .expect("lock cleanup sessions after close")
                .is_empty()
        );
    }

    #[test]
    fn stream_cleanup_takes_and_reaps_a_running_pty_child() {
        let system = portable_pty::native_pty_system();
        let pair = system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open stream cleanup pty");
        let writer = pair
            .master
            .take_writer()
            .expect("take stream cleanup writer");
        let child = pair
            .slave
            .spawn_command(long_running_fixture_command())
            .expect("spawn stream cleanup fixture");
        let manager = TerminalManager::default();
        manager
            .sessions
            .lock()
            .expect("lock stream cleanup sessions")
            .insert(
                "stream-cleanup-fixture".into(),
                Arc::new(TerminalSession {
                    closing: AtomicBool::new(false),
                    master: Mutex::new(pair.master),
                    writer: Arc::new(tokio::sync::Mutex::new(writer)),
                    child: Mutex::new(child),
                    start: Mutex::new(None),
                }),
            );

        cleanup_stream_session(&manager, "stream-cleanup-fixture");
        assert!(
            manager
                .sessions
                .lock()
                .expect("lock stream cleanup sessions after cleanup")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn oversized_terminal_write_is_rejected_before_session_lookup() {
        let data = vec![b'x'; mobarust_core::MAX_TERMINAL_INPUT_BYTES + 1];
        let error = TerminalManager::default()
            .write("missing", data)
            .await
            .expect_err("oversized terminal input must be rejected");
        assert!(matches!(
            error,
            TerminalError::Input(TerminalInputError::TooLarge)
        ));
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // Hold the child gate to test shared shutdown acknowledgement.
    async fn shutdown_reaps_children_and_refuses_late_publication_even_after_poison() {
        for poisoned in [false, true] {
            let manager = TerminalManager::default();
            let mut sessions = Vec::new();
            let mut ready_receivers = Vec::new();
            for index in 0..3 {
                let pair = portable_pty::native_pty_system()
                    .openpty(PtySize {
                        rows: 24,
                        cols: 80,
                        pixel_width: 0,
                        pixel_height: 0,
                    })
                    .expect("open shutdown fixture");
                let writer = pair.master.take_writer().expect("take fixture writer");
                let child = pair
                    .slave
                    .spawn_command(long_running_fixture_command())
                    .expect("spawn fixture");
                drop(pair.slave);
                let (ready, receiver) = mpsc::channel();
                let session = Arc::new(TerminalSession {
                    closing: AtomicBool::new(false),
                    master: Mutex::new(pair.master),
                    writer: Arc::new(tokio::sync::Mutex::new(writer)),
                    child: Mutex::new(child),
                    start: Mutex::new(Some(TerminalStart {
                        ready,
                        startup_command: Some("exit".into()),
                    })),
                });
                if index < 2 {
                    manager
                        .register_session(&format!("shutdown-{index}"), Arc::clone(&session))
                        .unwrap();
                }
                sessions.push(session);
                ready_receivers.push(receiver);
            }
            // A cleanup must not need the input writer, even during pressure.
            let writer = Arc::clone(&sessions[0].writer).try_lock_owned().unwrap();
            if poisoned {
                let session = Arc::clone(&sessions[0]);
                let registry = Arc::clone(&manager.sessions);
                let _ = thread::spawn(move || {
                    let _child = session.child.lock().unwrap();
                    let _sessions = registry.lock().unwrap();
                    panic!("poison owned shutdown controls");
                })
                .join();
            }
            let child_gate = sessions[0]
                .child
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let cleanup = manager.shutdown();
            let repeated = manager.clone().shutdown();
            tokio::pin!(repeated);
            let premature_ack = tokio::time::timeout(Duration::from_millis(20), &mut repeated)
                .await
                .is_ok();
            drop(child_gate);
            let sealed = manager.ensure_open();
            // Simulate a child created before Quit reaching publication late.
            let late = manager.register_session("late", Arc::clone(&sessions[2]));
            let completed = tokio::time::timeout(Duration::from_secs(3), async {
                (cleanup.await, repeated.await)
            })
            .await;
            drop(writer);
            let reaped = sessions.iter().all(|session| {
                session
                    .child
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .try_wait()
                    .is_ok_and(|exit| exit.is_some())
            });
            let closing = sessions
                .iter()
                .all(|session| session.closing.load(Ordering::Acquire));
            let startup_cancelled = ready_receivers
                .iter()
                .all(|ready| matches!(ready.try_recv(), Err(mpsc::TryRecvError::Disconnected)));
            // Always attempt cleanup before assertions, including on timeout.
            for session in &sessions {
                let _ = cleanup_session(session);
            }
            assert!(matches!(sealed, Err(TerminalError::ShuttingDown)));
            assert!(matches!(
                late,
                Err(TerminalError::ShuttingDown | TerminalError::LockPoisoned)
            ));
            assert!(matches!(completed, Ok((Ok(()), Ok(())))));
            assert!(
                !premature_ack,
                "repeated Quit must wait for the first cleanup owner"
            );
            assert!(
                reaped,
                "shutdown and late-publication refusal must reap their children"
            );
            assert!(closing);
            assert!(startup_cancelled, "pending startup must be cancelled");
            assert!(
                manager
                    .sessions
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .is_empty()
            );
            manager
                .shutdown()
                .await
                .expect("completed shutdown stays idempotent");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_non_reading_pty_does_not_block_async_input_or_close() {
        assert_non_reading_pty_responsive(false, false).await;
        assert_non_reading_pty_responsive(true, false).await;
        assert_non_reading_pty_responsive(false, true).await;
    }

    #[cfg(unix)]
    async fn assert_non_reading_pty_responsive(startup: bool, shutdown: bool) {
        let pair = portable_pty::native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open non-reading fixture");
        let writer = pair.master.take_writer().expect("take fixture writer");
        let mut reader = pair
            .master
            .try_clone_reader()
            .expect("clone fixture reader");
        let mut command = CommandBuilder::new("/bin/sh");
        // Disable line discipline dropping/echo before filling the input pipe.
        // The fixture also expires on its own if the test fails during setup.
        command.args(["-c", "stty raw -echo; printf READY; exec sleep 10"]);
        let child = pair.slave.spawn_command(command).expect("spawn fixture");
        drop(pair.slave);
        let (start, attached) = mpsc::channel();
        let session = Arc::new(TerminalSession {
            closing: AtomicBool::new(false),
            master: Mutex::new(pair.master),
            writer: Arc::new(tokio::sync::Mutex::new(writer)),
            child: Mutex::new(child),
            start: Mutex::new(startup.then(|| TerminalStart {
                ready: start,
                startup_command: Some("x".repeat(mobarust_core::MAX_SESSION_STARTUP_COMMAND_BYTES)),
            })),
        });
        let manager = TerminalManager::default();
        manager
            .sessions
            .lock()
            .unwrap()
            .insert("non-reader".into(), Arc::clone(&session));
        let (ready_tx, ready_rx) = mpsc::channel();
        let reader_thread = thread::spawn(move || {
            let mut ready = [0; 5];
            let result = reader.read_exact(&mut ready).map(|()| ready);
            let _ = ready_tx.send(result);
        });
        let ready = ready_rx.recv_timeout(Duration::from_secs(2));
        // PTY input capacities differ by OS. Fill this disposable input pipe
        // without blocking before testing a valid 16 KiB startup command.
        let pressured = if startup {
            let fd = session.master.lock().unwrap().as_raw_fd().unwrap();
            // SAFETY: the descriptor belongs to this live, private test PTY.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            let nonblocking = flags >= 0
                && unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == 0;
            let mut full = false;
            if nonblocking {
                let mut writer = session.writer.try_lock().unwrap();
                for _ in 0..1024 {
                    match writer.write(&[b'x'; 1024]) {
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            full = true;
                            break;
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                        Ok(size) if size > 0 => {}
                        _ => break,
                    }
                }
            }
            // Restore the original flags before any production write runs.
            let restored = nonblocking && unsafe { libc::fcntl(fd, libc::F_SETFL, flags) } == 0;
            full && restored
        } else {
            true
        };
        let write_manager = manager.clone();
        let write = tokio::spawn(async move {
            if startup {
                write_manager.attach("non-reader").await
            } else {
                write_manager
                    .write(
                        "non-reader",
                        vec![b'x'; mobarust_core::MAX_TERMINAL_INPUT_BYTES],
                    )
                    .await
            }
        });
        let started = std::time::Instant::now();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let responsive = started.elapsed() < Duration::from_secs(2);
        let blocked = !write.is_finished();
        let attached_before_write_finished = !startup || attached.try_recv().is_ok();
        let repeat_rejected = !startup
            || matches!(
                manager.attach("non-reader").await,
                Err(TerminalError::AlreadyAttached)
            );
        let busy = manager.write("non-reader", b"tail".to_vec()).await;
        // Cleanup before asserting: close never needs the input writer lock.
        let close = if shutdown {
            manager.shutdown().await
        } else {
            manager.close("non-reader")
        };
        let closed = session.child.lock().unwrap().try_wait();
        let completion = tokio::time::timeout(Duration::from_secs(2), write).await;
        drop(session);
        reader_thread.join().expect("join fixture readiness reader");
        assert!(matches!(ready, Ok(Ok(bytes)) if bytes == *b"READY"));
        assert!(
            pressured,
            "startup fixture must fill and restore its input pipe"
        );
        assert!(responsive, "blocked write stalled the async runtime");
        assert!(blocked, "fixture must actually stop reading PTY input");
        assert!(
            attached_before_write_finished,
            "release output before startup input"
        );
        assert!(
            repeat_rejected,
            "startup must not be replayed by another attach"
        );
        assert!(matches!(busy, Err(TerminalError::InputBusy)));
        close.expect("close non-reading child");
        assert!(matches!(closed, Ok(Some(_))), "child must be reaped");
        assert!(matches!(completion, Ok(Ok(Err(TerminalError::Io(_))))));
        assert!(matches!(
            manager.session("non-reader"),
            Err(TerminalError::Missing(_))
        ));
    }

    #[test]
    fn terminal_errors_do_not_echo_paths_or_process_details() {
        let private_path = "/Users/example/.ssh/private-key";
        let errors = [
            TerminalError::Open(anyhow::anyhow!("could not open {private_path}")),
            TerminalError::Missing(private_path.into()),
            TerminalError::Io(std::io::Error::other(format!(
                "write failed at {private_path}"
            ))),
            TerminalError::Resize(anyhow::anyhow!("resize failed at {private_path}")),
            TerminalError::Wait(std::io::Error::other(format!(
                "wait failed at {private_path}"
            ))),
        ];

        for error in errors {
            let display = error.to_string();
            assert!(
                !display.contains(private_path),
                "leaked terminal detail: {display}"
            );
            assert!(
                !display.contains("private-key"),
                "leaked terminal detail: {display}"
            );
        }
    }

    fn fixture_command() -> CommandBuilder {
        #[cfg(target_os = "windows")]
        {
            let mut command = CommandBuilder::new("powershell.exe");
            // Readiness must reach ConPTY directly, independently of the
            // PowerShell pipeline's output formatting around a blocking read.
            command.args([
                "-NoLogo",
                "-NoProfile",
                "-Command",
                "[Console]::WriteLine('MOBARUST_PTY_OK'); $line = [Console]::ReadLine(); [Console]::WriteLine('INPUT:' + $line); exit 0",
            ]);
            command
        }

        #[cfg(not(target_os = "windows"))]
        {
            let mut command = CommandBuilder::new("/bin/sh");
            command.args([
                "-c",
                "printf 'MOBARUST_PTY_OK\\n'; read line; printf 'INPUT:%s\\n' \"$line\"",
            ]);
            command
        }
    }

    fn long_running_fixture_command() -> CommandBuilder {
        #[cfg(target_os = "windows")]
        {
            let mut command = CommandBuilder::new("powershell.exe");
            command.args([
                "-NoLogo",
                "-NoProfile",
                "-Command",
                "Start-Sleep -Seconds 30",
            ]);
            command
        }

        #[cfg(not(target_os = "windows"))]
        {
            let mut command = CommandBuilder::new("/bin/sh");
            command.args(["-c", "exec sleep 30"]);
            command
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn executable_version(executable: &str) -> Option<String> {
        let output = std::process::Command::new(executable)
            .arg("--version")
            .output()
            .ok()?;
        output.status.success().then(|| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("version unavailable")
                .chars()
                .take(200)
                .collect()
        })
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

    #[test]
    fn local_target_defaults_keep_legacy_deserialization_compatible() {
        let target: LocalTerminalTarget =
            serde_json::from_str(r#"{"type":"default"}"#).expect("deserialize legacy local target");
        assert_eq!(
            target,
            LocalTerminalTarget::Default {
                shell: LocalShell::Default,
                cwd: None,
                environment: Vec::new(),
                startup_command: None,
            }
        );
    }

    #[test]
    fn explicit_shell_choices_deserialize_without_accepting_an_executable_path() {
        let target: LocalTerminalTarget = serde_json::from_str(
            r#"{"type":"default","shell":"powershell","startupCommand":"printf ok"}"#,
        )
        .expect("deserialize typed shell target");
        assert_eq!(
            target,
            LocalTerminalTarget::Default {
                shell: LocalShell::PowerShell,
                cwd: None,
                environment: Vec::new(),
                startup_command: Some("printf ok".into()),
            }
        );

        let arbitrary: Result<LocalTerminalTarget, _> =
            serde_json::from_str(r#"{"type":"default","shell":"/tmp/attacker-shell"}"#);
        assert!(arbitrary.is_err());
    }

    #[test]
    fn local_terminal_target_rejects_unknown_nested_fields() {
        let default_target: Result<LocalTerminalTarget, _> = serde_json::from_str(
            r#"{"type":"default","cwd":null,"environment":[],"unknown":true}"#,
        );
        assert!(default_target.is_err());

        let wsl_target: Result<LocalTerminalTarget, _> =
            serde_json::from_str(r#"{"type":"wsl","distribution":"Ubuntu","unknown":true}"#);
        assert!(wsl_target.is_err());
    }

    #[test]
    fn local_target_rejects_invalid_startup_configuration_without_touching_paths() {
        let target = LocalTerminalTarget::Default {
            shell: LocalShell::Default,
            cwd: Some("/tmp/mobarust\nfixture".into()),
            environment: vec![("SAFE_NAME".into(), "safe".into())],
            startup_command: None,
        };
        assert!(matches!(
            target.validate(),
            Err(TerminalError::InvalidWorkingDirectory)
        ));

        let target = LocalTerminalTarget::Default {
            shell: LocalShell::Default,
            cwd: None,
            environment: vec![("BAD-NAME".into(), "value".into())],
            startup_command: None,
        };
        assert!(matches!(
            target.validate(),
            Err(TerminalError::InvalidEnvironment)
        ));

        let target = LocalTerminalTarget::Default {
            shell: LocalShell::Default,
            cwd: None,
            environment: Vec::new(),
            startup_command: Some("printf\nunsafe".into()),
        };
        assert!(matches!(
            target.validate(),
            Err(TerminalError::InvalidStartupCommand)
        ));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn explicit_windows_shells_are_rejected_without_spawning_or_discovery() {
        assert!(matches!(
            shell_command(LocalShell::PowerShell),
            Err(TerminalError::UnsupportedTarget)
        ));
        assert!(matches!(
            shell_command(LocalShell::Cmd),
            Err(TerminalError::UnsupportedTarget)
        ));
        assert_eq!(shell_command(LocalShell::Bash).unwrap(), "bash");
        assert_eq!(shell_command(LocalShell::Zsh).unwrap(), "zsh");
        assert_eq!(shell_command(LocalShell::Fish).unwrap(), "fish");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn explicit_windows_shells_use_fixed_executables() {
        assert_eq!(
            shell_command(LocalShell::PowerShell).unwrap(),
            "powershell.exe"
        );
        assert_eq!(shell_command(LocalShell::Cmd).unwrap(), "cmd.exe");
        assert!(matches!(
            shell_command(LocalShell::Bash),
            Err(TerminalError::UnsupportedTarget)
        ));
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
