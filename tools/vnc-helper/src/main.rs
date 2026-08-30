//! Isolated VNC/RFB adapter for the native remote-desktop helper boundary.
//!
//! This process owns the VNC protocol engine. It accepts only non-secret
//! connection metadata in argv and receives the password through the
//! versioned native pipe after an explicit Start command. It never consults
//! the user's SSH files, SSH agent, clipboard, or host configuration.

use std::env;
use std::error::Error;
use std::fmt;
use std::time::{Duration, Instant};

use mobarust_remote_desktop::{
    DesktopProtocol, DisplaySize, HelperCommand, HelperCredential, HelperEvent,
    HelperProtocolError, HelperState, MAX_CLIPBOARD_BYTES, MAX_FRAME_BYTES, decode_command_frame,
    decode_credential_frame, read_frame, write_event_frame,
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
const REFRESH_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug)]
struct Arguments {
    host: String,
    port: u16,
    display: DisplaySize,
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    run_main().await
}

async fn run_main() -> Result<(), Box<dyn Error>> {
    let arguments = parse_arguments(env::args().skip(1))?;
    let mut stdout = tokio::io::stdout();
    write_event_frame(&mut stdout, &HelperEvent::Hello { version: 1 }).await?;
    write_state(&mut stdout, HelperState::Starting).await?;

    let (incoming_tx, mut incoming_rx) = mpsc::channel(8);
    let command_task = tokio::spawn(read_commands(tokio::io::stdin(), incoming_tx));
    let mut start_display = None;
    let mut credential = None;

    loop {
        match incoming_rx.recv().await {
            Some(Incoming::Command(HelperCommand::Start { protocol, display })) => {
                if protocol != DesktopProtocol::Vnc {
                    send_error(&mut stdout, "unsupported helper protocol").await?;
                    command_task.abort();
                    return Ok(());
                }
                start_display = Some(display);
            }
            Some(Incoming::Credential(value)) => credential = Some(value),
            Some(Incoming::Command(HelperCommand::Stop)) | Some(Incoming::End) => {
                write_state(&mut stdout, HelperState::Stopped).await?;
                command_task.abort();
                return Ok(());
            }
            Some(Incoming::Invalid) | None => {
                send_error(&mut stdout, "invalid helper input").await?;
                write_state(&mut stdout, HelperState::Failed).await?;
                command_task.abort();
                return Ok(());
            }
            Some(Incoming::Command(_)) => {}
        }

        if let (Some(display), Some(secret)) = (start_display, credential.take()) {
            let result =
                run_vnc_session(&arguments, display, secret, &mut stdout, &mut incoming_rx).await;
            command_task.abort();
            return result;
        }
    }
}

async fn read_commands(mut reader: tokio::io::Stdin, sender: mpsc::Sender<Incoming>) {
    loop {
        let frame = match read_frame(&mut reader).await {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                let _ = sender.send(Incoming::End).await;
                return;
            }
            Err(_) => {
                let _ = sender.send(Incoming::Invalid).await;
                return;
            }
        };

        if let Ok(command) = decode_command_frame(&frame) {
            if sender.send(Incoming::Command(command)).await.is_err() {
                return;
            }
            continue;
        }
        if let Ok(credential) = decode_credential_frame(&frame) {
            if sender.send(Incoming::Credential(credential)).await.is_err() {
                return;
            }
            continue;
        }
        let _ = sender.send(Incoming::Invalid).await;
        return;
    }
}

async fn run_vnc_session<W: AsyncWrite + Unpin>(
    arguments: &Arguments,
    display: DisplaySize,
    credential: HelperCredential,
    stdout: &mut W,
    incoming_rx: &mut mpsc::Receiver<Incoming>,
) -> Result<(), Box<dyn Error>> {
    display.validate()?;
    let address = format!("{}:{}", arguments.host, arguments.port);
    let connect_future = async move {
        let stream = match timeout(CONNECT_TIMEOUT, TcpStream::connect(&address)).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) => return Err(connection_error_message(&error)),
            Err(_) => return Err("VNC connection timed out"),
        };

        // vnc-rs 0.5.3 currently requires its auth callback to return an owned
        // String. Keep the helper-owned source copy zeroizing for the whole
        // callback lifetime; the upstream API remains a promotion gate until
        // it can accept a zeroizing/borrowed credential type directly.
        let password = Zeroizing::new(credential.password().to_owned());
        let connector = VncConnector::new(stream)
            .set_auth_method(async move { Ok::<String, VncError>(password.to_string()) })
            .add_encoding(VncEncoding::Zrle)
            .add_encoding(VncEncoding::CopyRect)
            .add_encoding(VncEncoding::Raw)
            .add_encoding(VncEncoding::CursorPseudo)
            .add_encoding(VncEncoding::DesktopSizePseudo)
            .allow_shared(true)
            .set_pixel_format(PixelFormat::rgba())
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
    };
    tokio::pin!(connect_future);
    let client = loop {
        tokio::select! {
            result = &mut connect_future => match result {
                Ok(client) => break client,
                Err(message) => {
                    send_error(stdout, message).await?;
                    write_state(stdout, HelperState::Failed).await?;
                    return Ok(());
                }
            },
            incoming = incoming_rx.recv() => match incoming {
                Some(Incoming::Command(HelperCommand::Stop)) | Some(Incoming::End) | None => {
                    write_state(stdout, HelperState::Stopped).await?;
                    return Ok(());
                }
                Some(Incoming::Invalid) | Some(Incoming::Credential(_)) => {
                    send_error(stdout, "invalid helper input").await?;
                    write_state(stdout, HelperState::Failed).await?;
                    return Ok(());
                }
                Some(Incoming::Command(_)) => {}
            }
        }
    };
    let mut canvas = Canvas::new(display)?;
    write_state(stdout, HelperState::Ready).await?;
    let mut active_sent = false;
    let mut last_refresh = Instant::now();

    loop {
        tokio::select! {
            incoming = incoming_rx.recv() => {
                match incoming {
                    Some(Incoming::Command(command)) => {
                        match handle_command(&client, command, stdout).await {
                            Ok(true) => {
                                let _ = client.close().await;
                                write_state(stdout, HelperState::Stopped).await?;
                                return Ok(());
                            }
                            Ok(false) => {}
                            Err(_) => {
                                send_error(stdout, "VNC input handling failed").await?;
                                let _ = client.close().await;
                                write_state(stdout, HelperState::Failed).await?;
                                return Ok(());
                            }
                        }
                    }
                    Some(Incoming::End) | None => {
                        let _ = client.close().await;
                        write_state(stdout, HelperState::Stopped).await?;
                        return Ok(());
                    }
                    Some(Incoming::Invalid) | Some(Incoming::Credential(_)) => {
                        send_error(stdout, "invalid helper input").await?;
                        let _ = client.close().await;
                        write_state(stdout, HelperState::Failed).await?;
                        return Ok(());
                    }
                }
            }
            _ = sleep(Duration::from_millis(16)) => {
                loop {
                    match client.poll_event().await {
                        Ok(Some(event)) => {
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
                                    let _ = client.close().await;
                                    write_state(stdout, HelperState::Failed).await?;
                                    return Ok(());
                                }
                            }
                        }
                        Ok(None) => break,
                        Err(_) => {
                            send_error(stdout, "VNC session ended unexpectedly").await?;
                            write_state(stdout, HelperState::Failed).await?;
                            return Ok(());
                        }
                    }
                }
                if last_refresh.elapsed() >= REFRESH_INTERVAL {
                    if client.input(X11Event::Refresh).await.is_err() {
                        send_error(stdout, "VNC refresh failed").await?;
                        write_state(stdout, HelperState::Failed).await?;
                        return Ok(());
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
                "VNC server-side resize is not supported by this adapter",
            )
            .await?;
            Ok(false)
        }
        HelperCommand::Key { scancode, pressed } => {
            client
                .input(X11Event::KeyEvent((scancode, pressed).into()))
                .await
                .map_err(|_| "VNC keyboard input failed")?;
            Ok(false)
        }
        HelperCommand::Pointer { x, y, buttons } => {
            client
                .input(X11Event::PointerEvent(ClientMouseEvent::from((
                    x, y, buttons,
                ))))
                .await
                .map_err(|_| "VNC pointer input failed")?;
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
                client
                    .input(X11Event::CopyText(text.to_string()))
                    .await
                    .map_err(|_| "VNC clipboard input failed")?;
            }
            Ok(false)
        }
        HelperCommand::Start { .. } => Ok(false),
    }
}

struct Canvas {
    size: DisplaySize,
    pixels: Vec<u8>,
}

impl Canvas {
    fn new(size: DisplaySize) -> Result<Self, HelperProtocolError> {
        size.validate()?;
        let bytes = framebuffer_bytes(size)?;
        Ok(Self {
            size,
            pixels: vec![0; bytes],
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
                )?)
            }
            "--port" => port = Some(parse_u16("port", &next_argument(&mut iterator, "--port")?)?),
            "--username" => {
                username = Some(validated_text(
                    "username",
                    &next_argument(&mut iterator, "--username")?,
                )?)
            }
            "--width" => width = parse_u16("width", &next_argument(&mut iterator, "--width")?)?,
            "--height" => height = parse_u16("height", &next_argument(&mut iterator, "--height")?)?,
            _ => return Err(ArgumentError("unknown helper argument".into())),
        }
    }

    let _username = username;
    let arguments = Arguments {
        host: host.ok_or_else(|| ArgumentError("missing host".into()))?,
        port: port.ok_or_else(|| ArgumentError("missing port".into()))?,
        display: DisplaySize { width, height },
    };
    arguments
        .display
        .validate()
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

fn validated_text(name: &str, value: &str) -> Result<String, ArgumentError> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return Err(ArgumentError(format!("invalid {name}")));
    }
    Ok(value.to_owned())
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
    }

    #[test]
    fn parser_does_not_echo_secret_bearing_unknown_arguments() {
        let error = parse_arguments(["--password=fixture-secret".to_owned()]).unwrap_err();
        assert_eq!(error.to_string(), "unknown helper argument");
        assert!(!error.to_string().contains("fixture-secret"));
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
        let mut canvas = Canvas::new(size).unwrap();
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
        let mut canvas = Canvas::new(size).unwrap();
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
}
