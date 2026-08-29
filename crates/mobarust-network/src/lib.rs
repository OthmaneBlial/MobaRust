//! Bounded, administration-focused network diagnostics.
//!
//! This crate deliberately does not provide an unrestricted scanner. Every
//! scan has an explicit host, an explicit bounded port range, a timeout, and a
//! cancellation receiver. Tests use loopback listeners only.

use std::io;
use std::net::IpAddr;
use std::process::{Output, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::net::{TcpStream, lookup_host};
use tokio::process::Command;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::Instant;

const MAX_PORTS_PER_SCAN: u16 = 4096;
const MAX_CONCURRENCY: usize = 128;
const MAX_TRACE_HOPS: u8 = 32;
const MAX_TRACE_LINES: usize = 64;
const MAX_TRACE_LINE_BYTES: usize = 256;

#[derive(Debug, Error)]
pub enum NetworkDiagnosticError {
    #[error("network diagnostic target is invalid")]
    InvalidTarget,
    #[error("port range is invalid or exceeds the bounded diagnostic limit")]
    InvalidPortRange,
    #[error("network diagnostic timed out")]
    Timeout,
    #[error("DNS lookup failed: {0}")]
    Resolution(String),
    #[error("network diagnostic was cancelled")]
    Cancelled,
    #[error("network diagnostic worker failed: {0}")]
    Worker(String),
    #[error("network diagnostic process failed: {0}")]
    Process(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpCheckOptions {
    pub host: String,
    pub port: u16,
    pub timeout: Duration,
}

impl TcpCheckOptions {
    pub fn validate(&self) -> Result<(), NetworkDiagnosticError> {
        validate_host(&self.host)?;
        if self.port == 0 || self.timeout.is_zero() {
            return Err(NetworkDiagnosticError::InvalidTarget);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TcpPortStatus {
    Open,
    Closed,
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TcpCheckResult {
    pub host: String,
    pub port: u16,
    pub status: TcpPortStatus,
}

pub async fn check_tcp(options: TcpCheckOptions) -> Result<TcpCheckResult, NetworkDiagnosticError> {
    options.validate()?;
    let host = options.host.clone();
    let status = match tokio::time::timeout(
        options.timeout,
        TcpStream::connect((options.host.as_str(), options.port)),
    )
    .await
    {
        Ok(Ok(stream)) => {
            drop(stream);
            TcpPortStatus::Open
        }
        Ok(Err(error)) if error.kind() == io::ErrorKind::ConnectionRefused => TcpPortStatus::Closed,
        Ok(Err(_)) | Err(_) => TcpPortStatus::TimedOut,
    };
    Ok(TcpCheckResult {
        host,
        port: options.port,
        status,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortScanOptions {
    pub host: String,
    pub start_port: u16,
    pub end_port: u16,
    pub concurrency: usize,
    pub timeout: Duration,
}

impl PortScanOptions {
    pub fn validate(&self) -> Result<(), NetworkDiagnosticError> {
        validate_host(&self.host)?;
        if self.start_port == 0
            || self.end_port < self.start_port
            || self
                .end_port
                .saturating_sub(self.start_port)
                .saturating_add(1)
                > MAX_PORTS_PER_SCAN
            || self.concurrency == 0
            || self.concurrency > MAX_CONCURRENCY
            || self.timeout.is_zero()
        {
            return Err(NetworkDiagnosticError::InvalidPortRange);
        }
        Ok(())
    }
}

/// Scans one explicit bounded TCP range. The caller owns cancellation and may
/// flip the watch value to `true`; all outstanding tasks are then aborted and
/// joined before returning.
pub async fn scan_tcp(
    options: PortScanOptions,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<Vec<TcpCheckResult>, NetworkDiagnosticError> {
    scan_tcp_with_progress(options, cancellation, |_, _, _| {}).await
}

/// Scans one explicit bounded TCP range and calls `on_progress` after each
/// completed port. The callback is synchronous and must remain lightweight;
/// it is intended for a native event bridge rather than arbitrary work.
pub async fn scan_tcp_with_progress<F>(
    options: PortScanOptions,
    cancellation: &mut watch::Receiver<bool>,
    mut on_progress: F,
) -> Result<Vec<TcpCheckResult>, NetworkDiagnosticError>
where
    F: FnMut(&TcpCheckResult, usize, usize),
{
    options.validate()?;
    if *cancellation.borrow() {
        return Err(NetworkDiagnosticError::Cancelled);
    }

    let mut tasks = JoinSet::new();
    let mut next_port = Some(options.start_port);
    let mut results = Vec::new();
    let mut cancellation_closed = false;
    let total = usize::from(options.end_port - options.start_port) + 1;

    schedule_ports(&mut tasks, &options, &mut next_port);
    while !tasks.is_empty() {
        if cancellation_closed {
            if let Some(result) = tasks.join_next().await {
                let result =
                    result.map_err(|error| NetworkDiagnosticError::Worker(error.to_string()))??;
                on_progress(&result, results.len() + 1, total);
                results.push(result);
                schedule_ports(&mut tasks, &options, &mut next_port);
            }
            continue;
        }

        tokio::select! {
            changed = cancellation.changed() => {
                match changed {
                    Ok(()) if *cancellation.borrow() => {
                        tasks.abort_all();
                        while tasks.join_next().await.is_some() {}
                        return Err(NetworkDiagnosticError::Cancelled);
                    }
                    Ok(()) => {}
                    Err(_) => cancellation_closed = true,
                }
            }
            result = tasks.join_next() => {
                if let Some(result) = result {
                    let result = result.map_err(|error| NetworkDiagnosticError::Worker(error.to_string()))??;
                    on_progress(&result, results.len() + 1, total);
                    results.push(result);
                    schedule_ports(&mut tasks, &options, &mut next_port);
                }
            }
        }
    }
    results.sort_by_key(|result| result.port);
    Ok(results)
}

fn schedule_ports(
    tasks: &mut JoinSet<Result<TcpCheckResult, NetworkDiagnosticError>>,
    options: &PortScanOptions,
    next_port: &mut Option<u16>,
) {
    while tasks.len() < options.concurrency {
        let Some(port) = *next_port else {
            break;
        };
        if port > options.end_port {
            break;
        }
        let check = TcpCheckOptions {
            host: options.host.clone(),
            port,
            timeout: options.timeout,
        };
        tasks.spawn(async move { check_tcp(check).await });
        *next_port = port.checked_add(1);
    }
}

pub async fn resolve_host(
    host: impl Into<String>,
    timeout: Duration,
) -> Result<Vec<IpAddr>, NetworkDiagnosticError> {
    let host = host.into();
    validate_host(&host)?;
    if timeout.is_zero() {
        return Err(NetworkDiagnosticError::InvalidTarget);
    }
    let addresses = tokio::time::timeout(timeout, lookup_host((host.as_str(), 0)))
        .await
        .map_err(|_| NetworkDiagnosticError::Timeout)?
        .map_err(|error| NetworkDiagnosticError::Resolution(error.to_string()))?
        .map(|address| address.ip())
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(NetworkDiagnosticError::Resolution(
            "host resolved to no addresses".into(),
        ));
    }
    Ok(addresses)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PingOptions {
    pub host: String,
    pub timeout: Duration,
}

impl PingOptions {
    pub fn validate(&self) -> Result<(), NetworkDiagnosticError> {
        validate_host(&self.host)?;
        if self.timeout.is_zero() {
            return Err(NetworkDiagnosticError::InvalidTarget);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PingResult {
    pub host: String,
    pub reachable: bool,
    pub elapsed_ms: u64,
}

/// Runs one platform-native ping with one explicit echo request. The command
/// receives an argument array, never a shell string, and is killed when the
/// operation times out or is cancelled.
pub async fn ping(
    options: PingOptions,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<PingResult, NetworkDiagnosticError> {
    options.validate()?;
    if *cancellation.borrow() {
        return Err(NetworkDiagnosticError::Cancelled);
    }
    let host = options.host.clone();
    let started = Instant::now();
    let output = run_diagnostic_command(
        ping_program(),
        ping_arguments(&options.host),
        options.timeout,
        cancellation,
    )
    .await?;
    Ok(PingResult {
        host,
        reachable: output.status.success(),
        elapsed_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TracerouteOptions {
    pub host: String,
    pub timeout: Duration,
    pub max_hops: u8,
}

impl TracerouteOptions {
    pub fn validate(&self) -> Result<(), NetworkDiagnosticError> {
        validate_host(&self.host)?;
        if self.timeout.is_zero() || !(1..=MAX_TRACE_HOPS).contains(&self.max_hops) {
            return Err(NetworkDiagnosticError::InvalidTarget);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TracerouteResult {
    pub host: String,
    pub reached: bool,
    pub hops: Vec<String>,
    pub elapsed_ms: u64,
}

/// Runs one bounded platform-native traceroute. Hop text is treated as
/// untrusted diagnostic output and is truncated before crossing the IPC
/// boundary.
pub async fn traceroute(
    options: TracerouteOptions,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<TracerouteResult, NetworkDiagnosticError> {
    options.validate()?;
    if *cancellation.borrow() {
        return Err(NetworkDiagnosticError::Cancelled);
    }
    let started = Instant::now();
    let output = run_diagnostic_command(
        traceroute_program(),
        traceroute_arguments(&options.host, options.timeout, options.max_hops),
        options.timeout,
        cancellation,
    )
    .await?;
    let hops = bounded_trace_lines(&output.stdout);
    Ok(TracerouteResult {
        host: options.host,
        reached: output.status.success(),
        hops,
        elapsed_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
    })
}

async fn run_diagnostic_command(
    program: &'static str,
    arguments: Vec<String>,
    timeout: Duration,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<Output, NetworkDiagnosticError> {
    let child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| NetworkDiagnosticError::Process(format!("{program}: {error}")))?;
    let mut output = Box::pin(child.wait_with_output());
    let mut deadline = Box::pin(tokio::time::sleep(timeout));
    let mut cancellation_closed = false;

    loop {
        tokio::select! {
            result = &mut output => {
                return result.map_err(|error| NetworkDiagnosticError::Process(error.to_string()));
            }
            changed = cancellation.changed(), if !cancellation_closed => {
                match changed {
                    Ok(()) if *cancellation.borrow() => return Err(NetworkDiagnosticError::Cancelled),
                    Ok(()) => {},
                    Err(_) => cancellation_closed = true,
                }
            }
            _ = &mut deadline => return Err(NetworkDiagnosticError::Timeout),
        }
    }
}

#[cfg(unix)]
fn ping_program() -> &'static str {
    "ping"
}

#[cfg(windows)]
fn ping_program() -> &'static str {
    "ping.exe"
}

#[cfg(unix)]
fn ping_arguments(host: &str) -> Vec<String> {
    vec!["-n".into(), "-c".into(), "1".into(), host.into()]
}

#[cfg(windows)]
fn ping_arguments(host: &str) -> Vec<String> {
    vec!["-n".into(), "1".into(), host.into()]
}

#[cfg(unix)]
fn traceroute_program() -> &'static str {
    "traceroute"
}

#[cfg(windows)]
fn traceroute_program() -> &'static str {
    "tracert.exe"
}

#[cfg(unix)]
fn traceroute_arguments(host: &str, timeout: Duration, max_hops: u8) -> Vec<String> {
    let per_hop_seconds = timeout.as_secs().clamp(1, 60);
    vec![
        "-n".into(),
        "-m".into(),
        max_hops.to_string(),
        "-w".into(),
        per_hop_seconds.to_string(),
        host.into(),
    ]
}

#[cfg(windows)]
fn traceroute_arguments(host: &str, timeout: Duration, max_hops: u8) -> Vec<String> {
    vec![
        "-d".into(),
        "-h".into(),
        max_hops.to_string(),
        "-w".into(),
        timeout.as_millis().clamp(1, 60_000).to_string(),
        host.into(),
    ]
}

fn bounded_trace_lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .take(MAX_TRACE_LINES)
        .map(|line| line.chars().take(MAX_TRACE_LINE_BYTES).collect())
        .collect()
}

fn validate_host(host: &str) -> Result<(), NetworkDiagnosticError> {
    if host.trim().is_empty()
        || host.len() > 253
        || host.contains('\0')
        || host.chars().any(char::is_control)
    {
        return Err(NetworkDiagnosticError::InvalidTarget);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn options_require_explicit_bounded_targets() {
        let options = PortScanOptions {
            host: "127.0.0.1".into(),
            start_port: 1,
            end_port: MAX_PORTS_PER_SCAN,
            concurrency: MAX_CONCURRENCY,
            timeout: Duration::from_millis(25),
        };
        assert!(options.validate().is_ok());

        let mut invalid = options.clone();
        invalid.end_port = MAX_PORTS_PER_SCAN + 1;
        assert!(matches!(
            invalid.validate(),
            Err(NetworkDiagnosticError::InvalidPortRange)
        ));
        invalid.end_port = MAX_PORTS_PER_SCAN;
        invalid.host = "".into();
        assert!(matches!(
            invalid.validate(),
            Err(NetworkDiagnosticError::InvalidTarget)
        ));
    }

    #[test]
    fn cancellation_is_observed_before_any_socket_is_opened() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let options = PortScanOptions {
                host: "127.0.0.1".into(),
                start_port: 1,
                end_port: 32,
                concurrency: 4,
                timeout: Duration::from_millis(25),
            };
            let (_sender, mut cancellation) = watch::channel(true);
            assert!(matches!(
                scan_tcp(options, &mut cancellation).await,
                Err(NetworkDiagnosticError::Cancelled)
            ));
        });
    }

    #[test]
    fn loopback_scan_reports_an_explicit_open_port() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let options = PortScanOptions {
                host: "127.0.0.1".into(),
                start_port: port,
                end_port: port,
                concurrency: 1,
                timeout: Duration::from_millis(250),
            };
            let (_sender, mut cancellation) = watch::channel(false);
            let results = scan_tcp(options, &mut cancellation).await.unwrap();
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].port, port);
            assert_eq!(results[0].status, TcpPortStatus::Open);
        });
    }

    #[test]
    fn progress_reports_each_completed_port_with_a_bounded_total() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let options = PortScanOptions {
                host: "127.0.0.1".into(),
                start_port: 31_000,
                end_port: 31_002,
                concurrency: 2,
                timeout: Duration::from_millis(25),
            };
            let (_sender, mut cancellation) = watch::channel(false);
            let mut progress = Vec::new();
            let results =
                scan_tcp_with_progress(options, &mut cancellation, |result, scanned, total| {
                    progress.push((result.port, scanned, total));
                })
                .await
                .unwrap();
            assert_eq!(results.len(), 3);
            assert_eq!(progress.len(), 3);
            assert!(progress.iter().all(|(_, _, total)| *total == 3));
            assert_eq!(progress.last().map(|(_, scanned, _)| *scanned), Some(3));
        });
    }

    #[test]
    fn highest_port_is_scanned_once_without_wraparound() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let options = PortScanOptions {
                host: "127.0.0.1".into(),
                start_port: u16::MAX,
                end_port: u16::MAX,
                concurrency: 4,
                timeout: Duration::from_millis(25),
            };
            let (_sender, mut cancellation) = watch::channel(false);
            let results = scan_tcp(options, &mut cancellation).await.unwrap();
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].port, u16::MAX);
        });
    }

    #[test]
    fn ping_uses_one_explicit_loopback_probe() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let options = PingOptions {
                host: "127.0.0.1".into(),
                timeout: Duration::from_secs(5),
            };
            let (_sender, mut cancellation) = watch::channel(false);
            let result = ping(options, &mut cancellation).await.unwrap();
            assert_eq!(result.host, "127.0.0.1");
            assert!(result.reachable);
        });
    }

    #[test]
    fn diagnostics_cancel_before_process_start() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (_sender, mut cancellation) = watch::channel(true);
            let ping_result = ping(
                PingOptions {
                    host: "127.0.0.1".into(),
                    timeout: Duration::from_secs(5),
                },
                &mut cancellation,
            )
            .await;
            assert!(matches!(
                ping_result,
                Err(NetworkDiagnosticError::Cancelled)
            ));

            let trace_result = traceroute(
                TracerouteOptions {
                    host: "127.0.0.1".into(),
                    timeout: Duration::from_secs(5),
                    max_hops: 4,
                },
                &mut cancellation,
            )
            .await;
            assert!(matches!(
                trace_result,
                Err(NetworkDiagnosticError::Cancelled)
            ));
        });
    }

    #[test]
    fn traceroute_output_is_bounded_before_ipc() {
        let input = vec![b'x'; MAX_TRACE_LINE_BYTES + 20];
        let output = bounded_trace_lines(&[input, b"\nsecond".to_vec()].concat());
        assert_eq!(output.len(), 2);
        assert_eq!(output[0].len(), MAX_TRACE_LINE_BYTES);
        assert_eq!(output[1], "second");
    }
}
