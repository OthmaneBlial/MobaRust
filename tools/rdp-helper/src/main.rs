//! Isolated IronRDP adapter for the native helper boundary.
//!
//! This binary is intentionally a separate Cargo workspace. IronRDP currently
//! resolves a prerelease crypto dependency that conflicts with the main
//! application's vault dependency, so keeping the adapter isolated prevents a
//! protocol experiment from changing the vault's dependency graph.
//!
//! The helper accepts only non-secret connection metadata in argv. The
//! password arrives as one zeroizing native-pipe frame after startup. No
//! frontend, shell command, SSH agent, personal key, or host configuration is
//! consulted here.

use std::env;
use std::error::Error;
use std::fmt;
use std::num::NonZeroU16;
use std::time::Duration;

use ironrdp_client::config::{ClipboardType, ConfigBuilder, Destination};
use ironrdp_client::rdp::{RdpClient, RdpInputEvent, RdpOutputEvent};
use ironrdp_pdu::gcc::KeyboardType;
use ironrdp_pdu::input::MousePdu;
use ironrdp_pdu::input::fast_path::{FastPathInputEvent, KeyboardFlags};
use ironrdp_pdu::input::mouse::PointerFlags;
use ironrdp_pdu::rdp::capability_sets::MajorPlatformType;
use mobarust_remote_desktop::{
    DesktopProtocol, DisplaySize, HelperCommand, HelperCredential, HelperEvent,
    HelperProtocolError, HelperState, MAX_DOMAIN_BYTES, MAX_FRAME_BYTES, MAX_HOST_BYTES,
    MAX_USERNAME_BYTES, decode_command_frame, decode_credential_frame, validate_rdp_color_depth,
    write_event_frame,
};
use smallvec::SmallVec;
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;
use tokio::task::LocalSet;
use tokio::time::timeout;

const STOP_GRACE_PERIOD: Duration = Duration::from_secs(5);
// IronRDP's connector does not expose an operation-specific startup deadline
// at this boundary, so the helper owns one and aborts the task if graceful
// shutdown cannot complete within the separate grace period.
const RDP_STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RECONNECT_ATTEMPTS: u8 = 3;
const RECONNECT_INITIAL_BACKOFF: Duration = Duration::from_millis(250);
const AUDIO_UNSUPPORTED: &str = "RDP audio redirection is not enabled in this helper";
const TLS_ENVIRONMENT_UNSUPPORTED: &str =
    "RDP helper refuses ambient TLS certificate override variables";

#[derive(Debug)]
struct Arguments {
    host: String,
    port: u16,
    username: String,
    domain: Option<String>,
    display: DisplaySize,
    color_depth: u16,
    audio_requested: bool,
}

#[derive(Debug)]
enum Incoming {
    Command(HelperCommand),
    Credential(HelperCredential),
    End,
    Invalid,
}

#[derive(Debug)]
struct ArgumentError(String);

impl fmt::Display for ArgumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ArgumentError {}

#[derive(Debug)]
struct RdpInputError(&'static str);

impl fmt::Display for RdpInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for RdpInputError {}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    LocalSet::new().run_until(run_main()).await
}

async fn run_main() -> Result<(), Box<dyn Error>> {
    if std::env::var_os("SSLKEYLOGFILE").is_some() {
        return Err(Box::new(ArgumentError(
            "TLS key logging is disabled for the RDP helper".into(),
        )));
    }
    if tls_certificate_override_present() {
        return Err(Box::new(ArgumentError(TLS_ENVIRONMENT_UNSUPPORTED.into())));
    }
    let arguments = parse_arguments(env::args().skip(1))?;
    let mut stdout = tokio::io::stdout();
    write_event_frame(&mut stdout, &HelperEvent::Hello { version: 1 }).await?;
    write_state(&mut stdout, HelperState::Starting).await?;
    if let Some(error) = unsupported_option(&arguments) {
        send_error(&mut stdout, error).await?;
        write_state(&mut stdout, HelperState::Failed).await?;
        return Ok(());
    }

    let (incoming_tx, mut incoming_rx) = mpsc::channel(8);
    // Tokio's stdin adapter uses an uncancellable blocking read. A dedicated
    // native thread lets this helper return after a terminal failure even if
    // the parent keeps the pipe open while it consumes the final events.
    let _command_thread = std::thread::spawn(move || read_commands(incoming_tx));

    let mut start_display = None;
    let mut credential = None;
    loop {
        match incoming_rx.recv().await {
            Some(Incoming::Command(HelperCommand::Start { protocol, display })) => {
                if protocol != DesktopProtocol::Rdp {
                    send_error(&mut stdout, "unsupported helper protocol").await?;
                    return Ok(());
                }
                start_display = Some(display);
            }
            Some(Incoming::Credential(value)) => credential = Some(value),
            Some(Incoming::Command(HelperCommand::Stop)) | Some(Incoming::End) => {
                write_state(&mut stdout, HelperState::Stopped).await?;
                return Ok(());
            }
            Some(Incoming::Invalid) | None => {
                send_error(&mut stdout, "invalid helper input").await?;
                write_state(&mut stdout, HelperState::Failed).await?;
                return Ok(());
            }
            Some(Incoming::Command(_)) => {}
        }

        if let (Some(display), Some(secret)) = (start_display, credential.take()) {
            return run_rdp_session(&arguments, display, secret, &mut stdout, &mut incoming_rx)
                .await;
        }
    }
}

fn read_commands(sender: mpsc::Sender<Incoming>) {
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    loop {
        let frame = match read_frame_blocking(&mut reader) {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                let _ = sender.blocking_send(Incoming::End);
                return;
            }
            Err(_) => {
                let _ = sender.blocking_send(Incoming::Invalid);
                return;
            }
        };

        if let Ok(command) = decode_command_frame(&frame) {
            if sender.blocking_send(Incoming::Command(command)).is_err() {
                return;
            }
            continue;
        }

        if let Ok(credential) = decode_credential_frame(&frame) {
            if sender
                .blocking_send(Incoming::Credential(credential))
                .is_err()
            {
                return;
            }
            continue;
        }

        let _ = sender.blocking_send(Incoming::Invalid);
        return;
    }
}

fn read_frame_blocking<R: std::io::Read>(
    reader: &mut R,
) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, std::io::Error> {
    let mut length_bytes = [0u8; 4];
    match reader.read_exact(&mut length_bytes) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }

    let body_length = u32::from_be_bytes(length_bytes) as usize;
    if body_length > MAX_FRAME_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "helper frame exceeds the safety limit",
        ));
    }
    let mut frame = zeroize::Zeroizing::new(vec![0u8; body_length.saturating_add(4)]);
    frame[..4].copy_from_slice(&length_bytes);
    reader.read_exact(&mut frame[4..])?;
    Ok(Some(frame))
}

async fn run_rdp_session<W: AsyncWrite + Unpin>(
    arguments: &Arguments,
    display: DisplaySize,
    credential: HelperCredential,
    stdout: &mut W,
    incoming_rx: &mut mpsc::Receiver<Incoming>,
) -> Result<(), Box<dyn Error>> {
    run_rdp_session_with_policy(
        arguments,
        display,
        credential,
        stdout,
        incoming_rx,
        RDP_STARTUP_TIMEOUT,
        STOP_GRACE_PERIOD,
    )
    .await
}

async fn run_rdp_session_with_policy<W: AsyncWrite + Unpin>(
    arguments: &Arguments,
    display: DisplaySize,
    credential: HelperCredential,
    stdout: &mut W,
    incoming_rx: &mut mpsc::Receiver<Incoming>,
    startup_timeout: Duration,
    startup_stop_grace: Duration,
) -> Result<(), Box<dyn Error>> {
    let mut current_display = display;
    let mut reconnect_attempt = 0u8;

    loop {
        if reconnect_attempt > 0
            && !wait_rdp_reconnect_backoff(
                reconnect_delay(reconnect_attempt - 1),
                stdout,
                incoming_rx,
            )
            .await?
        {
            return Ok(());
        }

        let reconnecting = reconnect_attempt > 0;
        match run_rdp_attempt(
            arguments,
            &credential,
            &mut current_display,
            stdout,
            incoming_rx,
            RdpAttemptPolicy {
                startup_timeout,
                startup_stop_grace,
                reconnecting,
            },
        )
        .await?
        {
            RdpAttemptOutcome::Stopped | RdpAttemptOutcome::Fatal => return Ok(()),
            RdpAttemptOutcome::Lost {
                reason,
                reconnect_established,
            } => {
                if let Some(next_attempt) =
                    next_reconnect_attempt(reconnect_attempt, reconnect_established)
                {
                    write_state(stdout, HelperState::Reconnecting).await?;
                    reconnect_attempt = next_attempt;
                } else {
                    send_error(stdout, reason).await?;
                    write_state(stdout, HelperState::Failed).await?;
                    return Ok(());
                }
            }
        }
    }
}

enum RdpAttemptOutcome {
    Lost {
        reason: &'static str,
        reconnect_established: bool,
    },
    Fatal,
    Stopped,
}

#[derive(Clone, Copy)]
struct RdpAttemptPolicy {
    startup_timeout: Duration,
    startup_stop_grace: Duration,
    reconnecting: bool,
}

async fn run_rdp_attempt<W: AsyncWrite + Unpin>(
    arguments: &Arguments,
    credential: &HelperCredential,
    current_display: &mut DisplaySize,
    stdout: &mut W,
    incoming_rx: &mut mpsc::Receiver<Incoming>,
    policy: RdpAttemptPolicy,
) -> Result<RdpAttemptOutcome, Box<dyn Error>> {
    let config = match build_config(arguments, *current_display, credential) {
        Ok(config) => config,
        Err(_) => {
            send_error(stdout, "invalid RDP configuration").await?;
            write_state(stdout, HelperState::Failed).await?;
            return Ok(RdpAttemptOutcome::Fatal);
        }
    };

    // The third-party RDP config owns its password for the connection
    // lifetime. Rebuild it only inside this native helper for a retry; the
    // zeroizing source credential never crosses into the parent or React.
    let (output_tx, mut output_rx) = mpsc::channel(2);
    let client = RdpClient::new(config, output_tx);
    let input_tx = client.input_sender();
    let mut client_task = tokio::task::spawn_local(client.run());
    let mut last_buttons = 0u8;
    let mut active_sent = false;
    let startup_deadline = tokio::time::sleep(policy.startup_timeout);
    tokio::pin!(startup_deadline);
    let mut startup_pending = true;

    write_state(stdout, HelperState::Ready).await?;

    loop {
        tokio::select! {
            _ = &mut startup_deadline, if startup_pending => {
                let _ = input_tx.send(RdpInputEvent::Close);
                stop_client_with_grace(&input_tx, &mut client_task, policy.startup_stop_grace).await;
                if policy.reconnecting {
                    return Ok(RdpAttemptOutcome::Lost {
                        reason: "RDP connection timed out",
                        reconnect_established: false,
                    });
                }
                send_error(stdout, "RDP connection timed out").await?;
                write_state(stdout, HelperState::Failed).await?;
                return Ok(RdpAttemptOutcome::Fatal);
            }
            incoming = incoming_rx.recv() => {
                match incoming {
                    Some(Incoming::Command(command)) => {
                        let command_result = handle_command(
                            command,
                            &input_tx,
                            current_display,
                            &mut last_buttons,
                            stdout,
                            &mut client_task,
                        )
                        .await;
                        match command_result {
                            Ok(true) => return Ok(RdpAttemptOutcome::Stopped),
                            Ok(false) => {}
                            Err(_) => {
                                stop_client(&input_tx, &mut client_task).await;
                                if active_sent || policy.reconnecting {
                                    return Ok(lost_outcome(
                                        "RDP input handling failed",
                                        active_sent,
                                        policy.reconnecting,
                                    ));
                                }
                                send_error(stdout, "RDP input handling failed").await?;
                                write_state(stdout, HelperState::Failed).await?;
                                return Ok(RdpAttemptOutcome::Fatal);
                            }
                        }
                    }
                    Some(Incoming::End) | None => {
                        stop_client(&input_tx, &mut client_task).await;
                        write_state(stdout, HelperState::Stopped).await?;
                        return Ok(RdpAttemptOutcome::Stopped);
                    }
                    Some(Incoming::Invalid) | Some(Incoming::Credential(_)) => {
                        send_error(stdout, "invalid helper input").await?;
                        let _ = input_tx.send(RdpInputEvent::Close);
                        stop_client(&input_tx, &mut client_task).await;
                        write_state(stdout, HelperState::Failed).await?;
                        return Ok(RdpAttemptOutcome::Fatal);
                    }
                }
            }
            output = output_rx.recv() => {
                match output {
                    Some(RdpOutputEvent::Image { buffer, width, height }) => {
                        startup_pending = false;
                        if !active_sent {
                            active_sent = true;
                            write_state(stdout, HelperState::Active).await?;
                        }
                        match framebuffer_event(buffer, width, height) {
                            Ok(event) => write_event_frame(stdout, &event).await?,
                            Err(_) => {
                                send_error(stdout, "RDP framebuffer exceeds the helper safety limit").await?;
                                let _ = input_tx.send(RdpInputEvent::Close);
                                stop_client(&input_tx, &mut client_task).await;
                                write_state(stdout, HelperState::Failed).await?;
                                return Ok(RdpAttemptOutcome::Fatal);
                            }
                        }
                    }
                    Some(RdpOutputEvent::ConnectionFailure(error)) => {
                        if active_sent {
                            stop_client(&input_tx, &mut client_task).await;
                            return Ok(lost_outcome(
                                "RDP connection was lost",
                                active_sent,
                                policy.reconnecting,
                            ));
                        }
                        if policy.reconnecting {
                            stop_client(&input_tx, &mut client_task).await;
                            return Ok(lost_outcome(
                                rdp_failure_message(&error),
                                active_sent,
                                policy.reconnecting,
                            ));
                        }
                        send_error(stdout, rdp_failure_message(&error)).await?;
                        stop_client(&input_tx, &mut client_task).await;
                        write_state(stdout, HelperState::Failed).await?;
                        return Ok(RdpAttemptOutcome::Fatal);
                    }
                    Some(RdpOutputEvent::Terminated(Ok(_))) => {
                        write_state(stdout, HelperState::Stopped).await?;
                        return Ok(RdpAttemptOutcome::Stopped);
                    }
                    Some(RdpOutputEvent::Terminated(Err(_))) => {
                        stop_client(&input_tx, &mut client_task).await;
                        if active_sent {
                            return Ok(lost_outcome(
                                "RDP session ended unexpectedly",
                                active_sent,
                                policy.reconnecting,
                            ));
                        }
                        if policy.reconnecting {
                            return Ok(lost_outcome(
                                "RDP session terminated unexpectedly",
                                active_sent,
                                policy.reconnecting,
                            ));
                        }
                        send_error(stdout, "RDP session terminated unexpectedly").await?;
                        write_state(stdout, HelperState::Failed).await?;
                        return Ok(RdpAttemptOutcome::Fatal);
                    }
                    Some(RdpOutputEvent::PointerDefault)
                    | Some(RdpOutputEvent::PointerHidden)
                    | Some(RdpOutputEvent::PointerPosition { .. })
                    | Some(RdpOutputEvent::PointerBitmap(_)) => {
                        startup_pending = false;
                    }
                    None => {
                        stop_client(&input_tx, &mut client_task).await;
                        if active_sent {
                            return Ok(lost_outcome(
                                "RDP connection was lost",
                                active_sent,
                                policy.reconnecting,
                            ));
                        }
                        if policy.reconnecting {
                            return Ok(lost_outcome(
                                "RDP connection was lost",
                                active_sent,
                                policy.reconnecting,
                            ));
                        }
                        write_state(stdout, HelperState::Crashed).await?;
                        return Ok(RdpAttemptOutcome::Fatal);
                    }
                }
            }
            result = &mut client_task => {
                if active_sent {
                    return Ok(lost_outcome(
                        "RDP connection was lost",
                        active_sent,
                        policy.reconnecting,
                    ));
                }
                if policy.reconnecting {
                    return Ok(lost_outcome(
                        "RDP connection was lost",
                        active_sent,
                        policy.reconnecting,
                    ));
                }
                if result.is_err() {
                    send_error(stdout, "RDP helper engine stopped unexpectedly").await?;
                    write_state(stdout, HelperState::Crashed).await?;
                } else {
                    write_state(stdout, HelperState::Stopped).await?;
                }
                return Ok(RdpAttemptOutcome::Fatal);
            }
        }
    }
}

fn reconnect_delay(attempt: u8) -> Duration {
    RECONNECT_INITIAL_BACKOFF.saturating_mul(1_u32 << u32::from(attempt.min(10)))
}

fn next_reconnect_attempt(current: u8, reconnect_established: bool) -> Option<u8> {
    let completed_attempts = if reconnect_established { 0 } else { current };
    (completed_attempts < MAX_RECONNECT_ATTEMPTS).then_some(completed_attempts + 1)
}

fn lost_outcome(reason: &'static str, active_sent: bool, reconnecting: bool) -> RdpAttemptOutcome {
    RdpAttemptOutcome::Lost {
        reason,
        reconnect_established: active_sent && reconnecting,
    }
}

async fn wait_rdp_reconnect_backoff<W: AsyncWrite + Unpin>(
    duration: Duration,
    stdout: &mut W,
    incoming_rx: &mut mpsc::Receiver<Incoming>,
) -> Result<bool, Box<dyn Error>> {
    let delay = tokio::time::sleep(duration);
    tokio::pin!(delay);
    loop {
        tokio::select! {
            _ = &mut delay => return Ok(true),
            incoming = incoming_rx.recv() => match incoming {
                Some(Incoming::Command(HelperCommand::Stop)) | Some(Incoming::End) | None => {
                    write_state(stdout, HelperState::Stopped).await?;
                    return Ok(false);
                }
                Some(Incoming::Invalid) | Some(Incoming::Credential(_)) => {
                    send_error(stdout, "invalid helper input").await?;
                    write_state(stdout, HelperState::Failed).await?;
                    return Ok(false);
                }
                Some(Incoming::Command(_)) => {}
            }
        }
    }
}

async fn handle_command<W: AsyncWrite + Unpin>(
    command: HelperCommand,
    input_tx: &mpsc::UnboundedSender<RdpInputEvent>,
    current_display: &mut DisplaySize,
    last_buttons: &mut u8,
    stdout: &mut W,
    client_task: &mut tokio::task::JoinHandle<()>,
) -> Result<bool, Box<dyn Error>> {
    match command {
        HelperCommand::Stop => {
            write_state(stdout, HelperState::Stopping).await?;
            stop_client(input_tx, client_task).await;
            write_state(stdout, HelperState::Stopped).await?;
            Ok(true)
        }
        HelperCommand::Resize { display } => {
            display.validate()?;
            send_rdp_input(
                input_tx,
                RdpInputEvent::Resize {
                    width: display.width,
                    height: display.height,
                    scale_factor: 100,
                    physical_size: None,
                },
            )?;
            *current_display = display;
            Ok(false)
        }
        HelperCommand::Key { scancode, pressed } => {
            let Some(scancode) = u8::try_from(scancode).ok() else {
                send_error(stdout, "keyboard scancode is outside the RDP range").await?;
                return Ok(false);
            };
            let flags = if pressed {
                KeyboardFlags::empty()
            } else {
                KeyboardFlags::RELEASE
            };
            let mut events = SmallVec::<[FastPathInputEvent; 2]>::new();
            events.push(FastPathInputEvent::KeyboardEvent(flags, scancode));
            send_rdp_input(input_tx, RdpInputEvent::FastPath(events))?;
            Ok(false)
        }
        HelperCommand::Pointer { x, y, buttons } => {
            let events = pointer_events(x, y, buttons, *last_buttons);
            send_rdp_input(input_tx, RdpInputEvent::FastPath(events))?;
            *last_buttons = buttons;
            Ok(false)
        }
        HelperCommand::Wheel { x, y, delta } => {
            send_rdp_input(
                input_tx,
                RdpInputEvent::FastPath(smallvec::smallvec![wheel_event(x, y, delta),]),
            )?;
            Ok(false)
        }
        HelperCommand::Clipboard { .. } => {
            // Clipboard needs a native OS backend and a user-controlled
            // trust policy. This helper build keeps the RDP channel enabled
            // for future wiring but does not silently bridge local clipboard
            // contents.
            send_error(
                stdout,
                "RDP clipboard redirection is not enabled in this helper build",
            )
            .await?;
            Ok(false)
        }
        HelperCommand::Start { .. } => Ok(false),
    }
}

fn send_rdp_input(
    input_tx: &mpsc::UnboundedSender<RdpInputEvent>,
    event: RdpInputEvent,
) -> Result<(), RdpInputError> {
    input_tx
        .send(event)
        .map_err(|_| RdpInputError("RDP input channel is unavailable"))
}

async fn stop_client(
    input_tx: &mpsc::UnboundedSender<RdpInputEvent>,
    client_task: &mut tokio::task::JoinHandle<()>,
) {
    stop_client_with_grace(input_tx, client_task, STOP_GRACE_PERIOD).await;
}

async fn stop_client_with_grace(
    input_tx: &mpsc::UnboundedSender<RdpInputEvent>,
    client_task: &mut tokio::task::JoinHandle<()>,
    grace_period: Duration,
) {
    let _ = input_tx.send(RdpInputEvent::Close);
    if timeout(grace_period, &mut *client_task).await.is_err() {
        client_task.abort();
        let _ = client_task.await;
    }
}

fn build_config(
    arguments: &Arguments,
    display: DisplaySize,
    credential: &HelperCredential,
) -> Result<ironrdp_client::config::Config, Box<dyn Error>> {
    let mut builder = ConfigBuilder::new()
        .with_destination(Destination::from_parts(
            arguments.host.clone(),
            arguments.port,
        ))
        .with_username(arguments.username.clone())
        .with_password(credential.password().to_owned())
        .with_client_build(1)
        .with_client_dir("MobaRust")
        .with_client_name("MobaRust")
        .with_platform(MajorPlatformType::UNSPECIFIED)
        .with_keyboard_type(KeyboardType::IbmEnhanced)
        .with_color_depth(u32::from(arguments.color_depth))
        .with_desktop_width(display.width)
        .with_desktop_height(display.height)
        .with_tls(true)
        .with_credssp(true)
        .with_server_pointer(true)
        .with_pointer_software_rendering(true)
        .with_clipboard(ClipboardType::Stub);
    if let Some(domain) = arguments.domain.as_deref() {
        builder = builder.with_domain(domain);
    }
    Ok(builder.build()?)
}

fn unsupported_option(arguments: &Arguments) -> Option<&'static str> {
    if arguments.audio_requested {
        return Some(AUDIO_UNSUPPORTED);
    }

    // Hostnames and IP literals are accepted here because the patched native
    // TLS adapter owns DNS/SNI and platform certificate verification. The
    // helper still refuses ambient certificate overrides and TLS key logging
    // before any connection attempt.
    None
}

fn tls_certificate_override_present() -> bool {
    std::env::vars_os().any(|(name, _)| is_tls_certificate_override(&name))
}

fn is_tls_certificate_override(name: &std::ffi::OsStr) -> bool {
    matches!(
        name.to_string_lossy().to_ascii_uppercase().as_str(),
        "SSL_CERT_FILE" | "SSL_CERT_DIR"
    )
}

fn rdp_failure_message(error: &ironrdp_connector::ConnectorError) -> &'static str {
    use ironrdp_connector::ConnectorErrorKind;

    match error.kind() {
        ConnectorErrorKind::AccessDenied | ConnectorErrorKind::Credssp(_) => {
            "RDP authentication or access was rejected"
        }
        ConnectorErrorKind::Negotiation(_) => "RDP protocol negotiation failed",
        ConnectorErrorKind::Decode(_) | ConnectorErrorKind::Encode(_) => {
            "RDP protocol data was invalid"
        }
        ConnectorErrorKind::Custom => "RDP TLS/certificate or transport validation failed",
        ConnectorErrorKind::Reason(_) | ConnectorErrorKind::General => "RDP connection failed",
        _ => "RDP connection failed",
    }
}

fn framebuffer_event(
    buffer: Vec<u32>,
    width: NonZeroU16,
    height: NonZeroU16,
) -> Result<HelperEvent, HelperProtocolError> {
    let width = width.get();
    let height = height.get();
    let expected_pixels = usize::from(width).saturating_mul(usize::from(height));
    if buffer.len() != expected_pixels {
        return Err(HelperProtocolError::InvalidFramebuffer {
            expected: expected_pixels.saturating_mul(4),
            actual: buffer.len().saturating_mul(4),
        });
    }
    let expected_bytes = expected_pixels.saturating_mul(4);
    if expected_bytes > MAX_FRAME_BYTES {
        return Err(HelperProtocolError::FrameTooLarge {
            bytes: expected_bytes,
        });
    }

    let mut pixels = Vec::with_capacity(expected_bytes);
    for pixel in buffer {
        let [_, red, green, blue] = pixel.to_be_bytes();
        pixels.extend_from_slice(&[red, green, blue, 0xff]);
    }
    Ok(HelperEvent::Framebuffer {
        width,
        height,
        pixels,
    })
}

fn pointer_events(
    x: u16,
    y: u16,
    buttons: u8,
    previous_buttons: u8,
) -> SmallVec<[FastPathInputEvent; 2]> {
    let mut events = SmallVec::new();
    events.push(FastPathInputEvent::MouseEvent(MousePdu {
        flags: PointerFlags::MOVE,
        number_of_wheel_rotation_units: 0,
        x_position: x,
        y_position: y,
    }));

    for (mask, button) in [
        (0b001u8, PointerFlags::LEFT_BUTTON),
        (0b010u8, PointerFlags::RIGHT_BUTTON),
        (0b100u8, PointerFlags::MIDDLE_BUTTON_OR_WHEEL),
    ] {
        if (buttons ^ previous_buttons) & mask != 0 {
            let mut flags = button;
            if buttons & mask != 0 {
                flags |= PointerFlags::DOWN;
            }
            events.push(FastPathInputEvent::MouseEvent(MousePdu {
                flags,
                number_of_wheel_rotation_units: 0,
                x_position: x,
                y_position: y,
            }));
        }
    }
    events
}

fn wheel_event(x: u16, y: u16, delta: i16) -> FastPathInputEvent {
    FastPathInputEvent::MouseEvent(MousePdu {
        flags: PointerFlags::VERTICAL_WHEEL,
        number_of_wheel_rotation_units: delta,
        x_position: x,
        y_position: y,
    })
}

async fn write_state<W: AsyncWrite + Unpin>(
    writer: &mut W,
    state: HelperState,
) -> Result<(), HelperProtocolError> {
    write_event_frame(writer, &HelperEvent::State { state }).await
}

async fn send_error<W: AsyncWrite + Unpin>(
    writer: &mut W,
    message: &str,
) -> Result<(), HelperProtocolError> {
    write_event_frame(
        writer,
        &HelperEvent::Diagnostic {
            level: mobarust_remote_desktop::DiagnosticLevel::Error,
            message: message.to_owned(),
        },
    )
    .await
}

fn parse_arguments<I>(arguments: I) -> Result<Arguments, ArgumentError>
where
    I: IntoIterator<Item = String>,
{
    let mut host = None;
    let mut port = None;
    let mut username = None;
    let mut domain = None;
    let mut width = 1280u16;
    let mut height = 720u16;
    let mut color_depth = 32u16;
    let mut audio_requested = false;
    let mut iterator = arguments.into_iter();

    while let Some(argument) = iterator.next() {
        match argument.as_str() {
            "--mobarust-protocol" => {
                if next_argument(&mut iterator, "--mobarust-protocol")? != "rdp" {
                    return Err(ArgumentError(
                        "only the rdp helper protocol is supported".into(),
                    ));
                }
            }
            "--host" => {
                host = Some(validated_text(
                    "host",
                    &next_argument(&mut iterator, "--host")?,
                    MAX_HOST_BYTES,
                )?)
            }
            "--port" => port = Some(parse_u16("port", &next_argument(&mut iterator, "--port")?)?),
            "--username" => {
                username = Some(validated_text(
                    "username",
                    &next_argument(&mut iterator, "--username")?,
                    MAX_USERNAME_BYTES,
                )?)
            }
            "--domain" => {
                domain = Some(validated_text(
                    "domain",
                    &next_argument(&mut iterator, "--domain")?,
                    MAX_DOMAIN_BYTES,
                )?)
            }
            "--width" => width = parse_u16("width", &next_argument(&mut iterator, "--width")?)?,
            "--height" => height = parse_u16("height", &next_argument(&mut iterator, "--height")?)?,
            "--color-depth" => {
                color_depth = parse_u16(
                    "color depth",
                    &next_argument(&mut iterator, "--color-depth")?,
                )?
            }
            "--audio" => audio_requested = true,
            // Do not echo the complete token: a caller must never be able to
            // turn a mistaken secret-bearing argument into a diagnostic leak.
            _ => return Err(ArgumentError("unknown helper argument".into())),
        }
    }

    let host = host.ok_or_else(|| ArgumentError("missing host".into()))?;
    ironrdp_tls::validate_server_name(&host).map_err(|_| ArgumentError("invalid host".into()))?;

    let arguments = Arguments {
        host,
        port: port.ok_or_else(|| ArgumentError("missing port".into()))?,
        username: username.ok_or_else(|| ArgumentError("missing username".into()))?,
        domain,
        display: DisplaySize { width, height },
        color_depth,
        audio_requested,
    };
    arguments
        .display
        .validate()
        .map_err(|error| ArgumentError(error.to_string()))?;
    validate_rdp_color_depth(arguments.color_depth)
        .map_err(|error| ArgumentError(error.to_string()))?;
    Ok(arguments)
}

fn next_argument<I>(iterator: &mut I, name: &str) -> Result<String, ArgumentError>
where
    I: Iterator<Item = String>,
{
    iterator
        .next()
        .ok_or_else(|| ArgumentError(format!("missing value for {name}")))
}

fn parse_u16(name: &str, value: &str) -> Result<u16, ArgumentError> {
    value
        .parse::<u16>()
        .map_err(|_| ArgumentError(format!("invalid {name}")))
        .and_then(|value| {
            if value == 0 {
                Err(ArgumentError(format!("{name} must be non-zero")))
            } else {
                Ok(value)
            }
        })
}

fn validated_text(name: &str, value: &str, max_bytes: usize) -> Result<String, ArgumentError> {
    if value.trim().is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(ArgumentError(format!("invalid {name}")));
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mobarust_remote_desktop::read_frame;

    #[test]
    fn parser_accepts_only_non_secret_metadata() {
        let arguments = parse_arguments(
            [
                "--mobarust-protocol",
                "rdp",
                "--host",
                "127.0.0.1",
                "--port",
                "3389",
                "--username",
                "fixture-user",
                "--domain",
                "FIXTURE",
                "--width",
                "1280",
                "--height",
                "720",
                "--color-depth",
                "32",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        assert_eq!(arguments.host, "127.0.0.1");
        assert_eq!(arguments.username, "fixture-user");
        assert_eq!(
            arguments.display,
            (DisplaySize {
                width: 1280,
                height: 720
            })
        );
    }

    #[test]
    fn parser_does_not_echo_secret_bearing_unknown_arguments() {
        let error = parse_arguments(["--password=fixture-secret".to_owned()]).unwrap_err();
        assert_eq!(error.to_string(), "unknown helper argument");
        assert!(!error.to_string().contains("fixture-secret"));
    }

    #[test]
    fn parser_rejects_oversized_metadata_without_echoing_input() {
        let oversized_host = "h".repeat(MAX_HOST_BYTES + 1);
        let error = parse_arguments(vec![
            "--mobarust-protocol".into(),
            "rdp".into(),
            "--host".into(),
            oversized_host.clone(),
            "--port".into(),
            "3389".into(),
            "--username".into(),
            "fixture-user".into(),
        ])
        .unwrap_err();
        assert_eq!(error.to_string(), "invalid host");
        assert!(!error.to_string().contains(&oversized_host));
    }

    #[test]
    fn parser_accepts_hostname_and_ip_targets_without_resolving_them() {
        for host in ["example.invalid", "192.0.2.10", "::1"] {
            let arguments = parse_arguments(
                [
                    "--mobarust-protocol",
                    "rdp",
                    "--host",
                    host,
                    "--port",
                    "3389",
                    "--username",
                    "fixture-user",
                ]
                .into_iter()
                .map(String::from),
            )
            .unwrap();
            assert_eq!(arguments.host, host);
        }
    }

    #[test]
    fn parser_rejects_invalid_server_names_before_connecting_without_echoing_them() {
        let invalid_host = "not a valid/server name";
        let error = parse_arguments(
            [
                "--mobarust-protocol",
                "rdp",
                "--host",
                invalid_host,
                "--port",
                "3389",
                "--username",
                "fixture-user",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "invalid host");
        assert!(!error.to_string().contains(invalid_host));
    }

    #[test]
    fn parser_rejects_unsupported_color_depth() {
        let error = parse_arguments(
            [
                "--mobarust-protocol",
                "rdp",
                "--host",
                "127.0.0.1",
                "--port",
                "3389",
                "--username",
                "fixture-user",
                "--color-depth",
                "24",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap_err();
        assert!(error.to_string().contains("16 or 32"));
    }

    #[test]
    fn audio_option_is_reported_as_unsupported_instead_of_ignored() {
        let arguments = parse_arguments(
            [
                "--mobarust-protocol",
                "rdp",
                "--host",
                "127.0.0.1",
                "--port",
                "3389",
                "--username",
                "fixture-user",
                "--audio",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        assert_eq!(unsupported_option(&arguments), Some(AUDIO_UNSUPPORTED));
    }

    #[test]
    fn validated_tls_candidate_accepts_hostname_and_ip_metadata_without_network_io() {
        let mut arguments = Arguments {
            host: "example.invalid".into(),
            port: 3389,
            username: "fixture-user".into(),
            domain: None,
            display: DisplaySize {
                width: 320,
                height: 200,
            },
            color_depth: 32,
            audio_requested: false,
        };
        assert_eq!(unsupported_option(&arguments), None);

        arguments.host = "192.0.2.10".into();
        assert_eq!(unsupported_option(&arguments), None);
        arguments.host = "localhost".into();
        assert_eq!(unsupported_option(&arguments), None);
    }

    #[test]
    fn rdp_config_preserves_hostname_metadata_without_connecting() {
        let arguments = Arguments {
            host: "example.invalid".into(),
            port: 3389,
            username: "fixture-user".into(),
            domain: None,
            display: DisplaySize {
                width: 320,
                height: 200,
            },
            color_depth: 32,
            audio_requested: false,
        };

        assert!(
            build_config(
                &arguments,
                arguments.display,
                &HelperCredential::new("fixture-secret"),
            )
            .is_ok()
        );
    }

    #[test]
    fn ambient_certificate_overrides_are_not_allowed() {
        for name in ["SSL_CERT_FILE", "ssl_cert_dir"] {
            assert!(is_tls_certificate_override(std::ffi::OsStr::new(name)));
        }
        assert!(!is_tls_certificate_override(std::ffi::OsStr::new("PATH")));
    }

    #[test]
    fn framebuffer_conversion_is_bounded_and_explicit_rgba() {
        let event = framebuffer_event(
            vec![0x0011_2233],
            NonZeroU16::new(1).unwrap(),
            NonZeroU16::new(1).unwrap(),
        )
        .unwrap();
        assert!(
            matches!(event, HelperEvent::Framebuffer { pixels, .. } if pixels == vec![0x11, 0x22, 0x33, 0xff])
        );
    }

    #[test]
    fn pointer_events_send_move_and_only_changed_buttons() {
        let events = pointer_events(10, 20, 0b001, 0);
        assert_eq!(events.len(), 2);
        assert!(
            matches!(events[0], FastPathInputEvent::MouseEvent(MousePdu { flags, .. }) if flags == PointerFlags::MOVE)
        );
        assert!(
            matches!(events[1], FastPathInputEvent::MouseEvent(MousePdu { flags, .. }) if flags.contains(PointerFlags::LEFT_BUTTON) && flags.contains(PointerFlags::DOWN))
        );
    }

    #[test]
    fn wheel_events_use_bounded_vertical_rotation_units() {
        assert!(matches!(
            wheel_event(10, 20, 120),
            FastPathInputEvent::MouseEvent(MousePdu {
                flags,
                number_of_wheel_rotation_units: 120,
                x_position: 10,
                y_position: 20,
            }) if flags == PointerFlags::VERTICAL_WHEEL
        ));
        assert!(matches!(
            wheel_event(10, 20, -120),
            FastPathInputEvent::MouseEvent(MousePdu {
                flags,
                number_of_wheel_rotation_units: -120,
                ..
            }) if flags == PointerFlags::VERTICAL_WHEEL
        ));
    }

    #[test]
    fn connector_failures_are_categorized_without_internal_details() {
        let denied = ironrdp_connector::ConnectorError::new(
            "fixture authentication",
            ironrdp_connector::ConnectorErrorKind::AccessDenied,
        );
        assert_eq!(
            rdp_failure_message(&denied),
            "RDP authentication or access was rejected"
        );

        let custom = ironrdp_connector::ConnectorError::new(
            "fixture TLS",
            ironrdp_connector::ConnectorErrorKind::Custom,
        );
        assert_eq!(
            rdp_failure_message(&custom),
            "RDP TLS/certificate or transport validation failed"
        );

        let reason = ironrdp_connector::ConnectorError::new(
            "fixture internal detail",
            ironrdp_connector::ConnectorErrorKind::Reason(
                "secret server name and certificate detail".into(),
            ),
        );
        assert_eq!(rdp_failure_message(&reason), "RDP connection failed");
        assert!(!rdp_failure_message(&reason).contains("secret server name"));
    }

    #[test]
    fn reconnect_backoff_is_exponential_and_saturating() {
        assert_eq!(reconnect_delay(0), Duration::from_millis(250));
        assert_eq!(reconnect_delay(1), Duration::from_millis(500));
        assert_eq!(reconnect_delay(2), Duration::from_secs(1));
        assert!(reconnect_delay(10) >= reconnect_delay(2));
    }

    #[test]
    fn reconnect_budget_is_bounded_until_a_retry_reaches_active() {
        assert_eq!(next_reconnect_attempt(0, false), Some(1));
        assert_eq!(next_reconnect_attempt(1, false), Some(2));
        assert_eq!(next_reconnect_attempt(2, false), Some(3));
        assert_eq!(next_reconnect_attempt(3, false), None);
        assert_eq!(next_reconnect_attempt(3, true), Some(1));
    }

    #[tokio::test]
    async fn reconnect_backoff_honors_stop_without_waiting_for_the_delay() {
        let (sender, mut receiver) = mpsc::channel(1);
        sender
            .send(Incoming::Command(HelperCommand::Stop))
            .await
            .unwrap();
        let mut stdout = tokio::io::sink();
        let started = std::time::Instant::now();

        assert!(
            !wait_rdp_reconnect_backoff(Duration::from_secs(30), &mut stdout, &mut receiver,)
                .await
                .unwrap()
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn clipboard_command_is_rejected_without_forwarding_remote_text() {
        let (input_tx, mut input_rx) = mpsc::unbounded_channel();
        let (mut writer, mut reader) = tokio::io::duplex(4096);
        let mut current_display = DisplaySize {
            width: 640,
            height: 480,
        };
        let mut last_buttons = 0;
        let mut client_task = tokio::spawn(async {});

        assert!(
            !handle_command(
                HelperCommand::Clipboard {
                    text: "remote-secret-text".to_owned().into(),
                },
                &input_tx,
                &mut current_display,
                &mut last_buttons,
                &mut writer,
                &mut client_task,
            )
            .await
            .unwrap()
        );

        let frame = read_frame(&mut reader).await.unwrap().unwrap();
        let event = mobarust_remote_desktop::decode_event_frame(&frame).unwrap();
        assert!(matches!(
            event,
            HelperEvent::Diagnostic { message, .. }
                if message == "RDP clipboard redirection is not enabled in this helper build"
        ));
        assert!(input_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn closed_rdp_input_channel_is_categorized_without_event_details() {
        let (input_tx, input_rx) = mpsc::unbounded_channel();
        drop(input_rx);
        let mut current_display = DisplaySize {
            width: 640,
            height: 480,
        };
        let mut last_buttons = 0;
        let mut client_task = tokio::spawn(async {});
        let mut stdout = tokio::io::sink();

        let error = handle_command(
            HelperCommand::Key {
                scancode: 30,
                pressed: true,
            },
            &input_tx,
            &mut current_display,
            &mut last_buttons,
            &mut stdout,
            &mut client_task,
        )
        .await
        .unwrap_err();

        assert_eq!(error.to_string(), "RDP input channel is unavailable");
        assert!(!error.to_string().contains("FastPath"));
    }

    #[tokio::test]
    async fn invalid_rdp_resize_is_rejected_before_queueing_or_mutating_state() {
        let (input_tx, mut input_rx) = mpsc::unbounded_channel();
        let mut current_display = DisplaySize {
            width: 640,
            height: 480,
        };
        let mut last_buttons = 0;
        let mut client_task = tokio::spawn(async {});
        let mut stdout = tokio::io::sink();

        let error = handle_command(
            HelperCommand::Resize {
                display: DisplaySize {
                    width: 319,
                    height: 200,
                },
            },
            &input_tx,
            &mut current_display,
            &mut last_buttons,
            &mut stdout,
            &mut client_task,
        )
        .await
        .unwrap_err();

        assert!(matches!(
            error.downcast_ref::<HelperProtocolError>(),
            Some(HelperProtocolError::InvalidDisplaySize {
                width: 319,
                height: 200
            })
        ));
        assert_eq!(current_display.width, 640);
        assert_eq!(current_display.height, 480);
        assert!(input_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn stalled_loopback_handshake_is_aborted_by_the_startup_timeout() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let arguments = Arguments {
            host: "127.0.0.1".into(),
            port,
            username: "fixture-user".into(),
            domain: None,
            display: DisplaySize {
                width: 320,
                height: 200,
            },
            color_depth: 32,
            audio_requested: false,
        };
        let (_incoming_tx, mut incoming_rx) = mpsc::channel(1);
        let mut stdout = tokio::io::sink();
        let started = std::time::Instant::now();

        LocalSet::new()
            .run_until(run_rdp_session_with_policy(
                &arguments,
                arguments.display,
                HelperCredential::new("fixture-secret"),
                &mut stdout,
                &mut incoming_rx,
                Duration::from_millis(40),
                Duration::from_millis(40),
            ))
            .await
            .unwrap();

        assert!(started.elapsed() < Duration::from_secs(2));
        drop(listener);
    }
}
