//! Isolated VNC/RFB adapter for the native remote-desktop helper boundary.
//!
//! This process owns the VNC protocol engine. It accepts only non-secret
//! connection metadata in argv and receives the password through the
//! versioned native pipe after an explicit Start command. It never consults
//! the user's SSH files, SSH agent, clipboard, or host configuration.

use std::env;
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use mobarust_remote_desktop::{
    DEFAULT_REMOTE_DESKTOP_RECONNECT_ATTEMPTS, DEFAULT_REMOTE_DESKTOP_RECONNECT_ENABLED,
    DesktopProtocol, DisplaySize, HelperCapabilities, HelperCommand, HelperCredential,
    HelperCredentialKind, HelperEvent, HelperProtocolError, HelperState, MAX_CLIPBOARD_BYTES,
    MAX_FRAME_BYTES, MAX_HOST_BYTES, MAX_USERNAME_BYTES, ReconnectPolicy, decode_command_frame,
    decode_credential_frame, write_event_frame,
};
use tokio::io::AsyncWrite;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout};
use vnc::{
    ClientMouseEvent, PixelFormat, Rect, VncClient, VncConnector, VncEncoding, VncError, VncEvent,
    X11Event,
};
use zeroize::Zeroizing;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(12);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
const VNC_INPUT_TIMEOUT: Duration = Duration::from_secs(2);
const VNC_EVENT_POLL_TIMEOUT: Duration = Duration::from_millis(250);
const RECONNECT_INITIAL_BACKOFF: Duration = Duration::from_millis(250);
const VNC_TARGET_UNSUPPORTED: &str =
    "VNC experiment is restricted to a loopback IP until transport security is available";

#[derive(Debug)]
struct Arguments {
    host: String,
    port: u16,
    display: DisplaySize,
    quality: String,
    clipboard_enabled: bool,
    reconnect_enabled: bool,
    reconnect_attempts: u8,
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
struct VncInputError(&'static str);

impl fmt::Display for VncInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for VncInputError {}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    run_main().await
}

async fn run_main() -> Result<(), Box<dyn Error>> {
    let arguments = parse_arguments(env::args().skip(1))?;
    let mut stdout = tokio::io::stdout();
    write_event_frame(&mut stdout, &HelperEvent::Hello { version: 1 }).await?;
    write_state(&mut stdout, HelperState::Starting).await?;
    if loopback_socket_address(&arguments.host, arguments.port).is_err() {
        send_error(&mut stdout, VNC_TARGET_UNSUPPORTED).await?;
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
                if protocol != DesktopProtocol::Vnc {
                    send_error(&mut stdout, "unsupported helper protocol").await?;
                    return Ok(());
                }
                start_display = Some(display);
            }
            Some(Incoming::Credential(value)) if value.kind() == HelperCredentialKind::Session => {
                credential = Some(value)
            }
            Some(Incoming::Credential(_)) => {
                send_error(&mut stdout, "unexpected VNC gateway credential").await?;
                write_state(&mut stdout, HelperState::Failed).await?;
                return Ok(());
            }
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
            return run_vnc_session(&arguments, display, secret, &mut stdout, &mut incoming_rx)
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

async fn run_vnc_session<W: AsyncWrite + Unpin>(
    arguments: &Arguments,
    display: DisplaySize,
    credential: HelperCredential,
    stdout: &mut W,
    incoming_rx: &mut mpsc::Receiver<Incoming>,
) -> Result<(), Box<dyn Error>> {
    display.validate()?;
    let reconnect_policy = ReconnectPolicy {
        enabled: arguments.reconnect_enabled,
        attempts: arguments.reconnect_attempts,
    };
    let client = match wait_for_vnc_connection(arguments, &credential, stdout, incoming_rx).await? {
        ConnectionOutcome::Connected(client) => client,
        ConnectionOutcome::Failed(message) => {
            send_error(stdout, message).await?;
            write_state(stdout, HelperState::Failed).await?;
            return Ok(());
        }
        ConnectionOutcome::Fatal | ConnectionOutcome::Stopped => return Ok(()),
    };
    let mut client = client;

    loop {
        let mut canvas = Canvas::new(display, arguments.clipboard_enabled)?;
        let mut capabilities = HelperCapabilities::vnc();
        capabilities.clipboard = arguments.clipboard_enabled;
        write_state(stdout, HelperState::Ready).await?;
        write_event_frame(stdout, &HelperEvent::Capabilities { capabilities }).await?;

        match run_connected_vnc_session(
            &client,
            &mut canvas,
            stdout,
            incoming_rx,
            quality_refresh_interval(&arguments.quality),
        )
        .await?
        {
            ConnectedOutcome::Stopped => return Ok(()),
            ConnectedOutcome::Fatal => return Ok(()),
            ConnectedOutcome::Lost(message) => {
                close_vnc_client(&client).await;
                if !reconnect_policy.enabled || reconnect_policy.attempts == 0 {
                    send_error(stdout, message).await?;
                    write_state(stdout, HelperState::Failed).await?;
                    return Ok(());
                }
                write_state(stdout, HelperState::Reconnecting).await?;
                client = match reconnect_vnc_client(
                    arguments,
                    &credential,
                    message,
                    reconnect_policy,
                    stdout,
                    incoming_rx,
                )
                .await?
                {
                    Some(client) => client,
                    None => return Ok(()),
                };
            }
        }
    }
}

async fn close_vnc_client(client: &VncClient) {
    let _ = timeout(CLOSE_TIMEOUT, client.close()).await;
}

enum ConnectionOutcome {
    Connected(VncClient),
    Failed(&'static str),
    Fatal,
    Stopped,
}

enum ConnectedOutcome {
    Lost(&'static str),
    Fatal,
    Stopped,
}

async fn wait_for_vnc_connection<W: AsyncWrite + Unpin>(
    arguments: &Arguments,
    credential: &HelperCredential,
    stdout: &mut W,
    incoming_rx: &mut mpsc::Receiver<Incoming>,
) -> Result<ConnectionOutcome, Box<dyn Error>> {
    let connect_future = connect_vnc_client(arguments, credential);
    tokio::pin!(connect_future);
    loop {
        tokio::select! {
            result = &mut connect_future => match result {
                Ok(client) => return Ok(ConnectionOutcome::Connected(client)),
                Err(message) => return Ok(ConnectionOutcome::Failed(message)),
            },
            incoming = incoming_rx.recv() => match incoming {
                Some(Incoming::Command(HelperCommand::Stop)) | Some(Incoming::End) | None => {
                    write_state(stdout, HelperState::Stopped).await?;
                    return Ok(ConnectionOutcome::Stopped);
                }
                Some(Incoming::Invalid) | Some(Incoming::Credential(_)) => {
                    send_error(stdout, "invalid helper input").await?;
                    write_state(stdout, HelperState::Failed).await?;
                    return Ok(ConnectionOutcome::Fatal);
                }
                Some(Incoming::Command(_)) => {}
            }
        }
    }
}

async fn reconnect_vnc_client<W: AsyncWrite + Unpin>(
    arguments: &Arguments,
    credential: &HelperCredential,
    initial_reason: &'static str,
    reconnect_policy: ReconnectPolicy,
    stdout: &mut W,
    incoming_rx: &mut mpsc::Receiver<Incoming>,
) -> Result<Option<VncClient>, Box<dyn Error>> {
    let mut last_reason = initial_reason;
    for attempt in 0..reconnect_policy.attempts {
        let backoff = RECONNECT_INITIAL_BACKOFF.saturating_mul(1_u32 << attempt);
        if !wait_reconnect_backoff(backoff, stdout, incoming_rx).await? {
            return Ok(None);
        }
        match wait_for_vnc_connection(arguments, credential, stdout, incoming_rx).await? {
            ConnectionOutcome::Connected(client) => return Ok(Some(client)),
            ConnectionOutcome::Failed(message) => {
                last_reason = message;
                if attempt + 1 < reconnect_policy.attempts {
                    send_error(stdout, last_reason).await?;
                }
            }
            ConnectionOutcome::Fatal | ConnectionOutcome::Stopped => return Ok(None),
        }
    }
    send_error(stdout, last_reason).await?;
    write_state(stdout, HelperState::Failed).await?;
    Ok(None)
}

async fn connect_vnc_client(
    arguments: &Arguments,
    credential: &HelperCredential,
) -> Result<VncClient, &'static str> {
    let address = loopback_socket_address(&arguments.host, arguments.port)
        .map_err(|_| VNC_TARGET_UNSUPPORTED)?;
    let stream = match timeout(CONNECT_TIMEOUT, TcpStream::connect(&address)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => return Err(connection_error_message(&error)),
        Err(_) => return Err("VNC connection timed out"),
    };

    // vnc-rs 0.5.3 currently requires its auth callback to return an owned
    // String. Move the helper-owned buffer into that required value instead
    // of creating a second `to_string()` copy. The upstream-owned String is
    // still not zeroizing, so this API remains a promotion gate until it can
    // accept a zeroizing/borrowed credential type directly.
    let mut password = Zeroizing::new(credential.password().to_owned());
    let encodings =
        quality_encodings(&arguments.quality).ok_or("VNC quality profile is invalid")?;
    let mut connector = VncConnector::new(stream)
        .set_auth_method(async move { Ok::<String, VncError>(std::mem::take(&mut *password)) })
        .allow_shared(true)
        .set_pixel_format(PixelFormat::rgba());
    for &encoding in encodings {
        connector = connector.add_encoding(encoding);
    }
    let connector = connector
        .build()
        .map_err(|_| "VNC client configuration failed")?;
    let state = match timeout(CONNECT_TIMEOUT, connector.try_start()).await {
        Ok(Ok(state)) => state,
        Ok(Err(error)) => return Err(negotiation_error_message(&error)),
        Err(_) => return Err("VNC negotiation timed out"),
    };
    state
        .finish()
        .map_err(|_| "VNC protocol negotiation failed")
}

async fn wait_reconnect_backoff<W: AsyncWrite + Unpin>(
    duration: Duration,
    stdout: &mut W,
    incoming_rx: &mut mpsc::Receiver<Incoming>,
) -> Result<bool, Box<dyn Error>> {
    let delay = sleep(duration);
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

async fn run_connected_vnc_session<W: AsyncWrite + Unpin>(
    client: &VncClient,
    canvas: &mut Canvas,
    stdout: &mut W,
    incoming_rx: &mut mpsc::Receiver<Incoming>,
    refresh_interval: Duration,
) -> Result<ConnectedOutcome, Box<dyn Error>> {
    let mut active_sent = false;
    let mut last_refresh = Instant::now();

    loop {
        tokio::select! {
            incoming = incoming_rx.recv() => {
                match incoming {
                    Some(Incoming::Command(command)) => {
                        match handle_command(client, command, stdout).await {
                            Ok(true) => {
                                close_vnc_client(client).await;
                                write_state(stdout, HelperState::Stopped).await?;
                                return Ok(ConnectedOutcome::Stopped);
                            }
                            Ok(false) => {}
                            Err(_) => {
                                send_error(stdout, "VNC input handling failed").await?;
                                return Ok(ConnectedOutcome::Lost("VNC input handling failed"));
                            }
                        }
                    }
                    Some(Incoming::End) | None => {
                        close_vnc_client(client).await;
                        write_state(stdout, HelperState::Stopped).await?;
                        return Ok(ConnectedOutcome::Stopped);
                    }
                    Some(Incoming::Invalid) | Some(Incoming::Credential(_)) => {
                        send_error(stdout, "invalid helper input").await?;
                        close_vnc_client(client).await;
                        write_state(stdout, HelperState::Failed).await?;
                        return Ok(ConnectedOutcome::Fatal);
                    }
                }
            }
            _ = sleep(Duration::from_millis(16)) => {
                let connection_loss = loop {
                    match timeout(VNC_EVENT_POLL_TIMEOUT, client.poll_event()).await {
                        Ok(Ok(Some(event))) => {
                            match canvas.apply(event) {
                                Ok(Some(event)) => {
                                    if !active_sent {
                                        active_sent = true;
                                        write_state(stdout, HelperState::Active).await?;
                                    }
                                    write_event_frame(stdout, &event).await?;
                                }
                                Ok(None) => {}
                                Err(_) => {
                                    send_error(stdout, "VNC framebuffer update was invalid").await?;
                                    close_vnc_client(client).await;
                                    write_state(stdout, HelperState::Failed).await?;
                                    return Ok(ConnectedOutcome::Fatal);
                                }
                            }
                        }
                        Ok(Ok(None)) => break None,
                        Ok(Err(_)) => break Some("VNC session ended unexpectedly"),
                        Err(_) => break None,
                    }
                };
                if let Some(reason) = connection_loss {
                    return Ok(ConnectedOutcome::Lost(reason));
                }
                if last_refresh.elapsed() >= refresh_interval {
                    if send_vnc_input(
                        client.input(X11Event::Refresh),
                        "VNC refresh failed",
                        "VNC refresh timed out",
                    )
                    .await
                    .is_err()
                    {
                        return Ok(ConnectedOutcome::Lost("VNC refresh failed"));
                    }
                    last_refresh = Instant::now();
                }
            }
        }
    }
}

async fn handle_command<W: AsyncWrite + Unpin>(
    client: &VncClient,
    command: HelperCommand,
    stdout: &mut W,
) -> Result<bool, Box<dyn Error>> {
    match command {
        HelperCommand::Stop => Ok(true),
        HelperCommand::Resize { .. } => {
            send_error(
                stdout,
                "VNC server-side resize is not supported; viewport scaling remains local",
            )
            .await?;
            Ok(false)
        }
        HelperCommand::Key { scancode, pressed } => {
            send_vnc_input(
                client.input(X11Event::KeyEvent((scancode, pressed).into())),
                "VNC keyboard input failed",
                "VNC keyboard input timed out",
            )
            .await?;
            Ok(false)
        }
        HelperCommand::Pointer { x, y, buttons } => {
            send_vnc_input(
                client.input(X11Event::PointerEvent(ClientMouseEvent::from((
                    x, y, buttons,
                )))),
                "VNC pointer input failed",
                "VNC pointer input timed out",
            )
            .await?;
            Ok(false)
        }
        HelperCommand::Wheel { x, y, delta } => {
            let button = if delta.is_negative() {
                0b0000_1000
            } else {
                0b0001_0000
            };
            for _ in 0..wheel_steps(delta) {
                send_vnc_input(
                    client.input(X11Event::PointerEvent(ClientMouseEvent::from((
                        x, y, button,
                    )))),
                    "VNC wheel input failed",
                    "VNC wheel input timed out",
                )
                .await?;
                send_vnc_input(
                    client.input(X11Event::PointerEvent(ClientMouseEvent::from((x, y, 0)))),
                    "VNC wheel input failed",
                    "VNC wheel input timed out",
                )
                .await?;
            }
            Ok(false)
        }
        HelperCommand::Clipboard { text } => {
            if text.len() > MAX_CLIPBOARD_BYTES || !text.chars().all(|value| value as u32 <= 0xff) {
                send_error(
                    stdout,
                    "VNC clipboard text is outside the Latin-1 safety limit",
                )
                .await?;
            } else {
                send_vnc_input(
                    client.input(X11Event::CopyText(text.to_string())),
                    "VNC clipboard input failed",
                    "VNC clipboard input timed out",
                )
                .await?;
            }
            Ok(false)
        }
        HelperCommand::Start { .. } => Ok(false),
    }
}

async fn send_vnc_input<F>(
    operation: F,
    failure_message: &'static str,
    timeout_message: &'static str,
) -> Result<(), VncInputError>
where
    F: Future<Output = Result<(), VncError>>,
{
    bounded_vnc_input(
        VNC_INPUT_TIMEOUT,
        operation,
        failure_message,
        timeout_message,
    )
    .await
}

async fn bounded_vnc_input<F>(
    limit: Duration,
    operation: F,
    failure_message: &'static str,
    timeout_message: &'static str,
) -> Result<(), VncInputError>
where
    F: Future<Output = Result<(), VncError>>,
{
    match timeout(limit, operation).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(VncInputError(failure_message)),
        Err(_) => Err(VncInputError(timeout_message)),
    }
}

fn wheel_steps(delta: i16) -> u32 {
    (i32::from(delta).unsigned_abs() + 59)
        .div_euclid(120)
        .clamp(1, 8)
}

struct Canvas {
    size: DisplaySize,
    pixels: Vec<u8>,
    clipboard_enabled: bool,
}

impl Canvas {
    fn new(size: DisplaySize, clipboard_enabled: bool) -> Result<Self, HelperProtocolError> {
        size.validate()?;
        let bytes = framebuffer_bytes(size)?;
        Ok(Self {
            size,
            pixels: vec![0; bytes],
            clipboard_enabled,
        })
    }

    fn apply(&mut self, event: VncEvent) -> Result<Option<HelperEvent>, HelperProtocolError> {
        match event {
            VncEvent::SetResolution(screen) => {
                let size = DisplaySize {
                    width: screen.width,
                    height: screen.height,
                };
                size.validate()?;
                self.size = size;
                self.pixels = vec![0; framebuffer_bytes(size)?];
                Ok(None)
            }
            VncEvent::RawImage(rect, data) => {
                self.copy_rect(rect, &data)?;
                Ok(Some(self.framebuffer_event()))
            }
            VncEvent::Copy(destination, source) => {
                self.copy_pixels(destination, source)?;
                Ok(Some(self.framebuffer_event()))
            }
            VncEvent::JpegImage(_, _) => Err(HelperProtocolError::Io(
                "VNC JPEG rectangles are not enabled by this adapter".into(),
            )),
            VncEvent::SetPixelFormat(_) | VncEvent::SetCursor(_, _) | VncEvent::Bell => Ok(None),
            VncEvent::Text(_) if !self.clipboard_enabled => Ok(None),
            VncEvent::Text(text) => {
                if text.len() > MAX_CLIPBOARD_BYTES {
                    return Err(HelperProtocolError::ClipboardTooLarge { bytes: text.len() });
                }
                Ok(Some(HelperEvent::Clipboard { text: text.into() }))
            }
            VncEvent::Error(_) => Err(HelperProtocolError::Io("VNC decoder error".into())),
            _ => Err(HelperProtocolError::Io("unsupported VNC event".into())),
        }
    }

    fn copy_rect(&mut self, rect: Rect, data: &[u8]) -> Result<(), HelperProtocolError> {
        let width = usize::from(rect.width);
        let height = usize::from(rect.height);
        let expected = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(HelperProtocolError::FrameTooLarge { bytes: usize::MAX })?;
        if data.len() != expected {
            return Err(HelperProtocolError::InvalidFramebuffer {
                expected,
                actual: data.len(),
            });
        }
        self.check_rect(rect)?;
        for row in 0..height {
            let source_start = row * width * 4;
            let destination_start = ((usize::from(rect.y) + row) * usize::from(self.size.width)
                + usize::from(rect.x))
                * 4;
            self.pixels[destination_start..destination_start + width * 4]
                .copy_from_slice(&data[source_start..source_start + width * 4]);
        }
        Ok(())
    }

    fn copy_pixels(&mut self, destination: Rect, source: Rect) -> Result<(), HelperProtocolError> {
        if destination.width != source.width || destination.height != source.height {
            return Err(HelperProtocolError::InvalidFramebuffer {
                expected: usize::from(destination.width)
                    .saturating_mul(usize::from(destination.height)),
                actual: usize::from(source.width).saturating_mul(usize::from(source.height)),
            });
        }
        self.check_rect(destination)?;
        self.check_rect(source)?;
        let width = usize::from(source.width);
        let height = usize::from(source.height);
        let mut copied = vec![0_u8; width * height * 4];
        for row in 0..height {
            let start = ((usize::from(source.y) + row) * usize::from(self.size.width)
                + usize::from(source.x))
                * 4;
            copied[row * width * 4..(row + 1) * width * 4]
                .copy_from_slice(&self.pixels[start..start + width * 4]);
        }
        self.copy_rect(destination, &copied)
    }

    fn check_rect(&self, rect: Rect) -> Result<(), HelperProtocolError> {
        let right = u32::from(rect.x) + u32::from(rect.width);
        let bottom = u32::from(rect.y) + u32::from(rect.height);
        if right > u32::from(self.size.width) || bottom > u32::from(self.size.height) {
            return Err(HelperProtocolError::InvalidFramebuffer {
                expected: usize::from(self.size.width)
                    .saturating_mul(usize::from(self.size.height))
                    .saturating_mul(4),
                actual: 0,
            });
        }
        Ok(())
    }

    fn framebuffer_event(&self) -> HelperEvent {
        HelperEvent::Framebuffer {
            width: self.size.width,
            height: self.size.height,
            pixels: self.pixels.clone(),
        }
    }
}

fn framebuffer_bytes(size: DisplaySize) -> Result<usize, HelperProtocolError> {
    let bytes = usize::from(size.width)
        .checked_mul(usize::from(size.height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(HelperProtocolError::FrameTooLarge { bytes: usize::MAX })?;
    if bytes > MAX_FRAME_BYTES {
        return Err(HelperProtocolError::FrameTooLarge { bytes });
    }
    Ok(bytes)
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

fn connection_error_message(error: &std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::NotFound => "VNC host could not be resolved",
        std::io::ErrorKind::ConnectionRefused => "VNC connection was refused",
        std::io::ErrorKind::TimedOut => "VNC connection timed out",
        std::io::ErrorKind::AddrNotAvailable => "VNC address is unavailable",
        _ => "VNC connection failed",
    }
}

fn negotiation_error_message(error: &VncError) -> &'static str {
    match error {
        VncError::NoPassword => "VNC server requires a password",
        VncError::WrongPassword => "VNC authentication was rejected",
        VncError::InvalidSecurityTyep(_) => "VNC server offered an unsupported security type",
        VncError::WrongPixelFormat | VncError::InvalidImageData => {
            "VNC server sent invalid framebuffer data"
        }
        VncError::WrongServerMessage => "VNC server sent an unsupported message",
        VncError::IoError(error) => connection_error_message(error),
        _ => "VNC protocol negotiation failed",
    }
}

fn parse_arguments<I>(arguments: I) -> Result<Arguments, ArgumentError>
where
    I: IntoIterator<Item = String>,
{
    let mut host = None;
    let mut port = None;
    let mut username = None;
    let mut width = 1280_u16;
    let mut height = 720_u16;
    let mut quality = "balanced".to_owned();
    let mut clipboard_enabled = false;
    let mut reconnect_enabled = DEFAULT_REMOTE_DESKTOP_RECONNECT_ENABLED;
    let mut reconnect_attempts = DEFAULT_REMOTE_DESKTOP_RECONNECT_ATTEMPTS;
    let mut iterator = arguments.into_iter();

    while let Some(argument) = iterator.next() {
        match argument.as_str() {
            "--mobarust-protocol" => {
                if next_argument(&mut iterator, "--mobarust-protocol")? != "vnc" {
                    return Err(ArgumentError(
                        "only the vnc helper protocol is supported".into(),
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
            "--width" => width = parse_u16("width", &next_argument(&mut iterator, "--width")?)?,
            "--height" => height = parse_u16("height", &next_argument(&mut iterator, "--height")?)?,
            "--quality" => quality = parse_quality(&next_argument(&mut iterator, "--quality")?)?,
            "--clipboard-enabled" => clipboard_enabled = true,
            "--reconnect-enabled" => reconnect_enabled = true,
            "--reconnect-disabled" => reconnect_enabled = false,
            "--reconnect-attempts" => {
                reconnect_attempts = parse_u8(
                    "reconnect attempts",
                    &next_argument(&mut iterator, "--reconnect-attempts")?,
                )?
            }
            _ => return Err(ArgumentError("unknown helper argument".into())),
        }
    }

    let _username = username;
    let arguments = Arguments {
        host: host.ok_or_else(|| ArgumentError("missing host".into()))?,
        port: port.ok_or_else(|| ArgumentError("missing port".into()))?,
        display: DisplaySize { width, height },
        quality,
        clipboard_enabled,
        reconnect_enabled,
        reconnect_attempts,
    };
    arguments
        .display
        .validate()
        .map_err(|error| ArgumentError(error.to_string()))?;
    ReconnectPolicy {
        enabled: arguments.reconnect_enabled,
        attempts: arguments.reconnect_attempts,
    }
    .validate()
    .map_err(|_| ArgumentError("invalid reconnect policy".into()))?;
    Ok(arguments)
}

fn parse_quality(value: &str) -> Result<String, ArgumentError> {
    match value {
        "balanced" | "low-latency" | "low-bandwidth" => Ok(value.to_owned()),
        _ => Err(ArgumentError("invalid VNC quality".into())),
    }
}

const LOW_LATENCY_ENCODINGS: &[VncEncoding] = &[
    VncEncoding::Raw,
    VncEncoding::CopyRect,
    VncEncoding::Zrle,
    VncEncoding::CursorPseudo,
    VncEncoding::DesktopSizePseudo,
];

const BALANCED_ENCODINGS: &[VncEncoding] = &[
    VncEncoding::Zrle,
    VncEncoding::CopyRect,
    VncEncoding::Raw,
    VncEncoding::CursorPseudo,
    VncEncoding::DesktopSizePseudo,
];

const LOW_BANDWIDTH_ENCODINGS: &[VncEncoding] = &[
    VncEncoding::Zrle,
    VncEncoding::CopyRect,
    VncEncoding::Raw,
    VncEncoding::CursorPseudo,
    VncEncoding::DesktopSizePseudo,
];

fn quality_encodings(value: &str) -> Option<&'static [VncEncoding]> {
    match value {
        "balanced" => Some(BALANCED_ENCODINGS),
        "low-latency" => Some(LOW_LATENCY_ENCODINGS),
        "low-bandwidth" => Some(LOW_BANDWIDTH_ENCODINGS),
        _ => None,
    }
}

fn quality_refresh_interval(value: &str) -> Duration {
    match value {
        "low-latency" => Duration::from_millis(50),
        "low-bandwidth" => Duration::from_millis(250),
        _ => Duration::from_millis(100),
    }
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

fn parse_u8(name: &str, value: &str) -> Result<u8, ArgumentError> {
    value
        .parse::<u8>()
        .map_err(|_| ArgumentError(format!("invalid {name}")))
}

fn validated_text(name: &str, value: &str, max_bytes: usize) -> Result<String, ArgumentError> {
    if value.trim().is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(ArgumentError(format!("invalid {name}")));
    }
    Ok(value.to_owned())
}

fn loopback_socket_address(host: &str, port: u16) -> Result<SocketAddr, &'static str> {
    let address = host.parse::<IpAddr>().map_err(|_| VNC_TARGET_UNSUPPORTED)?;
    if !address.is_loopback() || port == 0 {
        return Err(VNC_TARGET_UNSUPPORTED);
    }
    Ok(SocketAddr::new(address, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_accepts_loopback_metadata_without_secret_arguments() {
        let arguments = parse_arguments(
            [
                "--mobarust-protocol",
                "vnc",
                "--host",
                "127.0.0.1",
                "--port",
                "5900",
                "--username",
                "fixture-user",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        assert_eq!(arguments.host, "127.0.0.1");
        assert_eq!(arguments.port, 5900);
        assert_eq!(arguments.quality, "balanced");
        assert!(!arguments.clipboard_enabled);
        assert!(arguments.reconnect_enabled);
        assert_eq!(arguments.reconnect_attempts, 3);
    }

    #[test]
    fn parser_accepts_explicit_clipboard_opt_in() {
        let arguments = parse_arguments(
            [
                "--mobarust-protocol",
                "vnc",
                "--host",
                "127.0.0.1",
                "--port",
                "5900",
                "--clipboard-enabled",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        assert!(arguments.clipboard_enabled);
    }

    #[test]
    fn parser_accepts_explicit_bounded_reconnect_policy() {
        let arguments = parse_arguments(
            [
                "--mobarust-protocol",
                "vnc",
                "--host",
                "127.0.0.1",
                "--port",
                "5900",
                "--reconnect-disabled",
                "--reconnect-attempts",
                "10",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        assert!(!arguments.reconnect_enabled);
        assert_eq!(arguments.reconnect_attempts, 10);
    }

    #[test]
    fn parser_rejects_unbounded_reconnect_policy_without_echoing_value() {
        let error = parse_arguments(
            [
                "--mobarust-protocol",
                "vnc",
                "--host",
                "127.0.0.1",
                "--port",
                "5900",
                "--reconnect-attempts",
                "11",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "invalid reconnect policy");
        assert!(!error.to_string().contains("11"));
    }

    #[test]
    fn parser_accepts_bounded_quality_profiles() {
        let arguments = parse_arguments(
            [
                "--mobarust-protocol",
                "vnc",
                "--host",
                "127.0.0.1",
                "--port",
                "5900",
                "--quality",
                "low-bandwidth",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        assert_eq!(arguments.quality, "low-bandwidth");
        assert_eq!(
            quality_encodings(&arguments.quality).unwrap()[0],
            VncEncoding::Zrle
        );
        assert_eq!(
            quality_refresh_interval(&arguments.quality),
            Duration::from_millis(250)
        );
    }

    #[test]
    fn parser_rejects_unknown_quality_without_echoing_input() {
        let error = parse_arguments(
            [
                "--mobarust-protocol",
                "vnc",
                "--host",
                "127.0.0.1",
                "--port",
                "5900",
                "--quality",
                "secret-quality-value",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "invalid VNC quality");
        assert!(!error.to_string().contains("secret-quality-value"));
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
            "vnc".into(),
            "--host".into(),
            oversized_host.clone(),
            "--port".into(),
            "5900".into(),
        ])
        .unwrap_err();
        assert_eq!(error.to_string(), "invalid host");
        assert!(!error.to_string().contains(&oversized_host));
    }

    #[test]
    fn insecure_vnc_candidate_is_restricted_to_loopback_ip_literals() {
        assert!(loopback_socket_address("127.0.0.1", 5900).is_ok());
        assert!(loopback_socket_address("::1", 5900).is_ok());
        assert!(loopback_socket_address("localhost", 5900).is_err());
        assert!(loopback_socket_address("example.invalid", 5900).is_err());
        assert!(loopback_socket_address("192.0.2.10", 5900).is_err());
    }

    #[test]
    fn loopback_socket_address_formats_ipv4_and_ipv6_without_ambiguity() {
        assert_eq!(
            loopback_socket_address("127.0.0.1", 5900)
                .unwrap()
                .to_string(),
            "127.0.0.1:5900"
        );
        assert_eq!(
            loopback_socket_address("::1", 5900).unwrap().to_string(),
            "[::1]:5900"
        );
        assert!(loopback_socket_address("localhost", 5900).is_err());
    }

    #[test]
    fn connection_errors_are_actionable_without_raw_host_details() {
        assert_eq!(
            connection_error_message(&std::io::Error::from(std::io::ErrorKind::ConnectionRefused)),
            "VNC connection was refused"
        );
        assert_eq!(
            connection_error_message(&std::io::Error::from(std::io::ErrorKind::NotFound)),
            "VNC host could not be resolved"
        );
    }

    #[test]
    fn negotiation_errors_are_categorized_without_server_text() {
        assert_eq!(
            negotiation_error_message(&VncError::WrongPassword),
            "VNC authentication was rejected"
        );
        assert_eq!(
            negotiation_error_message(&VncError::General("secret server detail".into())),
            "VNC protocol negotiation failed"
        );
    }

    #[test]
    fn canvas_rejects_oversized_frames_and_out_of_bounds_rectangles() {
        let size = DisplaySize {
            width: 320,
            height: 200,
        };
        let mut canvas = Canvas::new(size, true).unwrap();
        let error = canvas
            .apply(VncEvent::RawImage(
                Rect {
                    x: 319,
                    y: 0,
                    width: 2,
                    height: 1,
                },
                vec![0; 8],
            ))
            .unwrap_err();
        assert!(matches!(
            error,
            HelperProtocolError::InvalidFramebuffer { .. }
        ));
    }

    #[test]
    fn canvas_copies_rgba_pixels_and_emits_full_bounded_frame() {
        let size = DisplaySize {
            width: 320,
            height: 200,
        };
        let mut canvas = Canvas::new(size, true).unwrap();
        let event = canvas
            .apply(VncEvent::RawImage(
                Rect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                vec![0x11, 0x22, 0x33, 0xff],
            ))
            .unwrap()
            .unwrap();
        assert!(
            matches!(event, HelperEvent::Framebuffer { width: 320, height: 200, ref pixels } if pixels[..4] == [0x11, 0x22, 0x33, 0xff])
        );
    }

    #[test]
    fn canvas_suppresses_server_clipboard_without_explicit_opt_in() {
        let size = DisplaySize {
            width: 320,
            height: 200,
        };
        let mut canvas = Canvas::new(size, false).unwrap();
        assert!(
            canvas
                .apply(VncEvent::Text("must stay native".into()))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn wheel_delta_is_quantized_to_bounded_vnc_button_pulses() {
        assert_eq!(wheel_steps(120), 1);
        assert_eq!(wheel_steps(-240), 2);
        assert_eq!(wheel_steps(16), 1);
        assert_eq!(wheel_steps(16 * 120), 8);
    }

    #[tokio::test]
    async fn vnc_input_operations_have_a_bounded_timeout() {
        let error = bounded_vnc_input(
            Duration::from_millis(10),
            std::future::pending::<Result<(), VncError>>(),
            "input failed",
            "input timed out",
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "input timed out");
    }
}
