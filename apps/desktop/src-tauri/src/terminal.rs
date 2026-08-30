use mobarust_core::{
    OutputBatcher, TerminalInputError, validate_session_environment, validate_session_startup,
    validate_terminal_input,
};
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
    #[error("failed to create pseudo-terminal")]
    Open(#[source] anyhow::Error),
    #[error("terminal session not found")]
    Missing(String),
    #[error("terminal I/O failed")]
    Io(#[source] std::io::Error),
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
        let mut writer = match pair.master.take_writer() {
            Ok(writer) => writer,
            Err(error) => {
                let _ = cleanup_child(child.as_mut());
                return Err(TerminalError::Open(anyhow::anyhow!(error)));
            }
        };
        if let Some(startup_command) = startup_command {
            let startup_result = writer
                .write_all(startup_command.as_bytes())
                .and_then(|()| writer.write_all(b"\r"))
                .and_then(|()| writer.flush());
            if let Err(error) = startup_result {
                let _ = cleanup_child(child.as_mut());
                return Err(TerminalError::Io(error));
            }
        }
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
        if let Err(error) = thread::Builder::new()
            .name(format!("mobarust-pty-{id}"))
            .spawn(move || stream_output(app, manager, terminal_id, reader))
        {
            // The session is inserted before the stream worker starts so the
            // worker can race safely with an immediate close. If the OS
            // refuses the worker, take the session back and reap its child
            // instead of leaving a native process behind.
            let _ = self.take_session(&id);
            let _ = cleanup_session(&session);
            return Err(TerminalError::Io(error));
        }

        Ok(id)
    }

    pub fn write(&self, id: &str, data: &[u8]) -> Result<(), TerminalError> {
        validate_terminal_input(data)?;
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
            .take_session(id)
            .ok_or_else(|| TerminalError::Missing(id.to_owned()))?;
        cleanup_session(&session)
    }

    fn session(&self, id: &str) -> Result<Arc<TerminalSession>, TerminalError> {
        self.sessions
            .lock()
            .expect("terminal session map poisoned")
            .get(id)
            .cloned()
            .ok_or_else(|| TerminalError::Missing(id.to_owned()))
    }

    fn take_session(&self, id: &str) -> Option<Arc<TerminalSession>> {
        self.sessions
            .lock()
            .expect("terminal session map poisoned")
            .remove(id)
    }
}

/// Stop and reap a native PTY child exactly once after its session has been
/// removed from the manager. This is shared by explicit close, worker-start
/// failure, and reader EOF so every local process has a deterministic owner.
fn cleanup_session(session: &TerminalSession) -> Result<(), TerminalError> {
    let mut child = session.child.lock().expect("terminal child poisoned");
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

    cleanup_stream_session(&manager, &terminal_id);
    let _ = app.emit(
        "terminal://closed",
        TerminalClosed {
            terminal_id: terminal_id.clone(),
        },
    );
    manager.remove(&terminal_id);
}

fn cleanup_stream_session(manager: &TerminalManager, terminal_id: &str) {
    if let Some(session) = manager.take_session(terminal_id) {
        // The stream has ended, so an otherwise-live child no longer has a
        // usable terminal. Reuse the same bounded kill-and-reap policy as an
        // explicit close; there is no silent orphan path on reader EOF.
        let _ = cleanup_session(&session);
    }
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
        let _ = self.take_session(id);
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

        let command = fixture_command();
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
    fn closing_a_running_pty_terminates_and_reaps_the_fixture_child() {
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
        let manager = TerminalManager::default();
        manager
            .sessions
            .lock()
            .expect("lock cleanup sessions")
            .insert(
                "cleanup-fixture".into(),
                Arc::new(TerminalSession {
                    master: Mutex::new(pair.master),
                    writer: Mutex::new(writer),
                    child: Mutex::new(child),
                }),
            );

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
                    master: Mutex::new(pair.master),
                    writer: Mutex::new(writer),
                    child: Mutex::new(child),
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

    #[test]
    fn oversized_terminal_write_is_rejected_before_session_lookup() {
        let data = vec![b'x'; mobarust_core::MAX_TERMINAL_INPUT_BYTES + 1];
        let error = TerminalManager::default()
            .write("missing", &data)
            .expect_err("oversized terminal input must be rejected");
        assert!(matches!(
            error,
            TerminalError::Input(TerminalInputError::TooLarge)
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
            let mut command = CommandBuilder::new("cmd.exe");
            command.args([
                "/C",
                "echo MOBARUST_PTY_OK && set /p line= && echo. && echo INPUT:%line%",
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
            let mut command = CommandBuilder::new("cmd.exe");
            command.args(["/C", "ping 127.0.0.1 -n 30 > nul"]);
            command
        }

        #[cfg(not(target_os = "windows"))]
        {
            let mut command = CommandBuilder::new("/bin/sh");
            command.args(["-c", "sleep 30"]);
            command
        }
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
