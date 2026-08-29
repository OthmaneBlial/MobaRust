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
    HelperProtocolError, HelperState, MAX_FRAME_BYTES, decode_command_frame,
    decode_credential_frame, read_frame, write_event_frame,
};
use smallvec::SmallVec;
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;
use tokio::task::LocalSet;
use tokio::time::timeout;

const STOP_GRACE_PERIOD: Duration = Duration::from_secs(5);

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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    LocalSet::new().run_until(run_main()).await
}

async fn run_main() -> Result<(), Box<dyn Error>> {
    let arguments = parse_arguments(env::args().skip(1))?;
    let mut stdout = tokio::io::stdout();
    write_event_frame(&mut stdout, &HelperEvent::Hello { version: 1 }).await?;
    write_state(&mut stdout, HelperState::Starting).await?;

    let (incoming_tx, mut incoming_rx) = mpsc::channel(8);
    let command_task = tokio::task::spawn_local(read_commands(tokio::io::stdin(), incoming_tx));

    let mut start_display = None;
    let mut credential = None;
    loop {
        match incoming_rx.recv().await {
            Some(Incoming::Command(HelperCommand::Start { protocol, display })) => {
                if protocol != DesktopProtocol::Rdp {
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
                run_rdp_session(&arguments, display, secret, &mut stdout, &mut incoming_rx).await;
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

async fn run_rdp_session<W: AsyncWrite + Unpin>(
    arguments: &Arguments,
    display: DisplaySize,
    credential: HelperCredential,
    stdout: &mut W,
    incoming_rx: &mut mpsc::Receiver<Incoming>,
) -> Result<(), Box<dyn Error>> {
    let config = match build_config(arguments, display, &credential) {
        Ok(config) => config,
        Err(_) => {
            send_error(stdout, "invalid RDP configuration").await?;
            write_state(stdout, HelperState::Failed).await?;
            return Ok(());
        }
    };

    // The third-party RDP config owns its password for the connection
    // lifetime. The helper never logs or serializes that config; the pending
    // native credential wrapper is dropped as soon as the config is built.
    drop(credential);

    let (output_tx, mut output_rx) = mpsc::channel(2);
    let client = RdpClient::new(config, output_tx);
    let input_tx = client.input_sender();
    let mut client_task = tokio::task::spawn_local(client.run());
    let mut last_buttons = 0u8;
    let mut current_display = display;
    let mut active_sent = false;

    write_state(stdout, HelperState::Ready).await?;

    loop {
        tokio::select! {
            incoming = incoming_rx.recv() => {
                match incoming {
                    Some(Incoming::Command(command)) => {
                        if handle_command(
                            command,
                            &input_tx,
                            &mut current_display,
                            &mut last_buttons,
                            stdout,
                            &mut client_task,
                        ).await? {
                            return Ok(());
                        }
                    }
                    Some(Incoming::End) | None => {
                        stop_client(&input_tx, &mut client_task).await;
                        write_state(stdout, HelperState::Stopped).await?;
                        return Ok(());
                    }
                    Some(Incoming::Invalid) | Some(Incoming::Credential(_)) => {
                        send_error(stdout, "invalid helper input").await?;
                        let _ = input_tx.send(RdpInputEvent::Close);
                        stop_client(&input_tx, &mut client_task).await;
                        write_state(stdout, HelperState::Failed).await?;
                        return Ok(());
                    }
                }
            }
            output = output_rx.recv() => {
                match output {
                    Some(RdpOutputEvent::Image { buffer, width, height }) => {
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
                                return Ok(());
                            }
                        }
                    }
                    Some(RdpOutputEvent::ConnectionFailure(_)) => {
                        send_error(stdout, "RDP connection or authentication failed").await?;
                        write_state(stdout, HelperState::Failed).await?;
                        return Ok(());
                    }
                    Some(RdpOutputEvent::Terminated(Ok(_))) => {
                        write_state(stdout, HelperState::Stopped).await?;
                        return Ok(());
                    }
                    Some(RdpOutputEvent::Terminated(Err(_))) => {
                        send_error(stdout, "RDP session terminated unexpectedly").await?;
                        write_state(stdout, HelperState::Failed).await?;
                        return Ok(());
                    }
                    Some(RdpOutputEvent::PointerDefault)
                    | Some(RdpOutputEvent::PointerHidden)
                    | Some(RdpOutputEvent::PointerPosition { .. })
                    | Some(RdpOutputEvent::PointerBitmap(_)) => {}
                    None => {
                        write_state(stdout, HelperState::Crashed).await?;
                        return Ok(());
                    }
                }
            }
            result = &mut client_task => {
                if result.is_err() {
                    send_error(stdout, "RDP helper engine stopped unexpectedly").await?;
                    write_state(stdout, HelperState::Crashed).await?;
                } else {
                    write_state(stdout, HelperState::Stopped).await?;
                }
                return Ok(());
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
            *current_display = display;
            input_tx.send(RdpInputEvent::Resize {
                width: display.width,
                height: display.height,
                scale_factor: 100,
                physical_size: None,
            })?;
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
            input_tx.send(RdpInputEvent::FastPath(events))?;
            Ok(false)
        }
        HelperCommand::Pointer { x, y, buttons } => {
            let events = pointer_events(x, y, buttons, *last_buttons);
            *last_buttons = buttons;
            input_tx.send(RdpInputEvent::FastPath(events))?;
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

async fn stop_client(
    input_tx: &mpsc::UnboundedSender<RdpInputEvent>,
    client_task: &mut tokio::task::JoinHandle<()>,
) {
    let _ = input_tx.send(RdpInputEvent::Close);
    if timeout(STOP_GRACE_PERIOD, &mut *client_task).await.is_err() {
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
    // Audio is intentionally not enabled until a platform-native playback
    // policy exists. Keeping the flag parsed prevents accidental argv drift.
    let _ = arguments.audio_requested;
    Ok(builder.build()?)
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
                )?)
            }
            "--port" => port = Some(parse_u16("port", &next_argument(&mut iterator, "--port")?)?),
            "--username" => {
                username = Some(validated_text(
                    "username",
                    &next_argument(&mut iterator, "--username")?,
                )?)
            }
            "--domain" => {
                domain = Some(validated_text(
                    "domain",
                    &next_argument(&mut iterator, "--domain")?,
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

    let arguments = Arguments {
        host: host.ok_or_else(|| ArgumentError("missing host".into()))?,
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
    if arguments.color_depth != 16 && arguments.color_depth != 32 {
        return Err(ArgumentError("color depth must be 16 or 32".into()));
    }
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
}
