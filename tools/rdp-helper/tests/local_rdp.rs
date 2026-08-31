#![cfg(feature = "local-rdp-fixture")]

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use bytes::Bytes;
use ironrdp_server::{
    BitmapUpdate, ConnectionHandler, Credentials, DesktopSize, DisplayUpdate, PostConnectionAction,
    RdpServer, RdpServerDisplay, RdpServerDisplayUpdates, RdpServerInputHandler, ServerEvent,
    TlsIdentityCtx,
};
use mobarust_remote_desktop::{
    DisplaySize, HelperCommand, HelperCredential, HelperEvent, HelperState, decode_event_frame,
    encode_command_frame, encode_credential_frame, read_frame, write_frame_with_timeout,
};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::process::{ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{sleep, timeout};

const WIDTH: u16 = 320;
const HEIGHT: u16 = 200;
const FIXTURE_USER: &str = "fixture-user";
const FIXTURE_PASSWORD: &str = "fixture-rdp-password";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputKind {
    Keyboard,
    Mouse,
}

struct RecordingInputHandler {
    events: Arc<StdMutex<Vec<InputKind>>>,
}

impl RdpServerInputHandler for RecordingInputHandler {
    fn keyboard(&mut self, _event: ironrdp_server::KeyboardEvent) {
        self.events
            .lock()
            .expect("fixture input mutex poisoned")
            .push(InputKind::Keyboard);
    }

    fn mouse(&mut self, _event: ironrdp_server::MouseEvent) {
        self.events
            .lock()
            .expect("fixture input mutex poisoned")
            .push(InputKind::Mouse);
    }
}

struct StopAfterDisconnect {
    stop_after: usize,
    disconnects: usize,
}

impl StopAfterDisconnect {
    fn new(stop_after: usize) -> Self {
        Self {
            stop_after,
            disconnects: 0,
        }
    }
}

impl ConnectionHandler for StopAfterDisconnect {
    fn on_disconnected(
        &mut self,
        _peer: SocketAddr,
        _duration: Duration,
        _error: Option<&anyhow::Error>,
    ) -> PostConnectionAction {
        self.disconnects += 1;
        if self.disconnects >= self.stop_after {
            PostConnectionAction::Stop
        } else {
            PostConnectionAction::Continue
        }
    }
}

struct FixtureDisplay {
    size: DesktopSize,
    updates: VecDeque<mpsc::Receiver<DisplayUpdate>>,
}

struct FixtureDisplayUpdates {
    updates: mpsc::Receiver<DisplayUpdate>,
}

#[async_trait::async_trait]
impl RdpServerDisplayUpdates for FixtureDisplayUpdates {
    async fn next_update(&mut self) -> anyhow::Result<Option<DisplayUpdate>> {
        Ok(self.updates.recv().await)
    }
}

#[async_trait::async_trait]
impl RdpServerDisplay for FixtureDisplay {
    async fn size(&mut self) -> DesktopSize {
        self.size
    }

    async fn updates(&mut self) -> anyhow::Result<Box<dyn RdpServerDisplayUpdates>> {
        let updates = self
            .updates
            .pop_front()
            .expect("fixture display has no stream for this connection");
        Ok(Box::new(FixtureDisplayUpdates { updates }))
    }
}

struct DisposableFixtureDirectory(PathBuf);

impl Drop for DisposableFixtureDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture_directory() -> DisposableFixtureDirectory {
    let directory = std::env::temp_dir().join(format!(
        "mobarust-rdp-server-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock is before the Unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir(&directory).expect("create disposable RDP fixture directory");
    DisposableFixtureDirectory(directory)
}

fn create_fixture_identity(directory: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let ca_certificate = directory.join("fixture-ca-cert.pem");
    let ca_private_key = directory.join("fixture-ca-key.pem");
    let certificate = directory.join("server-cert.pem");
    let private_key = directory.join("server-key.pem");
    let csr = directory.join("server.csr");
    let extensions = directory.join("server-ext.cnf");
    let generated_ca = std::process::Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-subj",
            "/CN=MobaRust local RDP fixture CA",
            "-addext",
            "basicConstraints=critical,CA:TRUE,pathlen:0",
            "-addext",
            "keyUsage=critical,keyCertSign,cRLSign",
            "-days",
            "1",
            "-keyout",
        ])
        .arg(&ca_private_key)
        .arg("-out")
        .arg(&ca_certificate)
        .output()
        .expect("openssl is required for the local RDP fixture");
    assert!(
        generated_ca.status.success(),
        "openssl could not create the local RDP fixture CA"
    );

    let generated_request = std::process::Command::new("openssl")
        .args([
            "req",
            "-new",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-subj",
            "/CN=127.0.0.1",
            "-keyout",
        ])
        .arg(&private_key)
        .arg("-out")
        .arg(&csr)
        .output()
        .expect("openssl is required for the local RDP fixture");
    assert!(
        generated_request.status.success(),
        "openssl could not create the local RDP fixture CSR"
    );

    std::fs::write(
        &extensions,
        "basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nsubjectAltName=IP:127.0.0.1\n",
    )
    .expect("write local RDP fixture certificate extensions");
    let generated_certificate = std::process::Command::new("openssl")
        .args(["x509", "-req", "-in"])
        .arg(&csr)
        .args(["-CA"])
        .arg(&ca_certificate)
        .args(["-CAkey"])
        .arg(&ca_private_key)
        .args(["-CAcreateserial", "-days", "1", "-out"])
        .arg(&certificate)
        .args(["-extfile"])
        .arg(&extensions)
        .output()
        .expect("openssl is required for the local RDP fixture");
    assert!(
        generated_certificate.status.success(),
        "openssl could not sign the local RDP fixture certificate"
    );
    (certificate, private_key, ca_certificate)
}

fn fixture_bitmap(pixel: [u8; 4]) -> BitmapUpdate {
    let width = std::num::NonZeroU16::new(WIDTH).expect("fixture width is non-zero");
    let height = std::num::NonZeroU16::new(HEIGHT).expect("fixture height is non-zero");
    let data = pixel.repeat(usize::from(WIDTH) * usize::from(HEIGHT));
    BitmapUpdate {
        x: 0,
        y: 0,
        width,
        height,
        format: ironrdp_server::PixelFormat::ARgb32,
        data: Bytes::from(data),
        stride: std::num::NonZeroUsize::new(usize::from(WIDTH) * pixel.len())
            .expect("fixture stride is non-zero"),
    }
}

async fn queued_bitmap(
    pixel: [u8; 4],
) -> (mpsc::Sender<DisplayUpdate>, mpsc::Receiver<DisplayUpdate>) {
    let (sender, receiver) = mpsc::channel(1);
    sender
        .send(DisplayUpdate::Bitmap(fixture_bitmap(pixel)))
        .await
        .expect("queue the disposable RDP framebuffer");
    (sender, receiver)
}

async fn next_event(stdout: &mut ChildStdout, phase: &str) -> HelperEvent {
    let frame = timeout(Duration::from_secs(8), read_frame(stdout))
        .await
        .unwrap_or_else(|_| panic!("RDP helper event timed out during {phase}"))
        .unwrap_or_else(|_| panic!("RDP helper event read failed during {phase}"))
        .unwrap_or_else(|| panic!("RDP helper closed its event pipe during {phase}"));
    decode_event_frame(&frame).expect("RDP helper emitted an invalid event frame")
}

async fn send_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: &[u8]) {
    write_frame_with_timeout(writer, frame)
        .await
        .expect("RDP helper native pipe write failed");
}

async fn server_port(sender: &tokio::sync::mpsc::UnboundedSender<ServerEvent>) -> u16 {
    let (reply_tx, reply_rx) = oneshot::channel();
    sender
        .send(ServerEvent::GetLocalAddr(reply_tx))
        .expect("RDP fixture server event channel closed before bind");
    let address = timeout(Duration::from_secs(5), reply_rx)
        .await
        .expect("RDP fixture server did not report its bound address")
        .expect("RDP fixture server address response was dropped")
        .expect("RDP fixture server did not bind an address");
    address.port()
}

#[tokio::test(flavor = "current_thread")]
async fn real_helper_controls_a_real_loopback_rdp_server() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let _ = ironrdp_server::tokio_rustls::rustls::crypto::ring::default_provider()
                .install_default();
            let fixture = fixture_directory();
            let (certificate, private_key, ca_certificate) = create_fixture_identity(&fixture.0);
            let identity = TlsIdentityCtx::init_from_paths(&certificate, &private_key)
                .expect("load the disposable RDP fixture identity");
            let input_events = Arc::new(StdMutex::new(Vec::new()));
            let (_display_tx, display_rx) = queued_bitmap([0xff, 0x19, 0x4d, 0x74]).await;

            let mut server = RdpServer::builder()
                .with_addr(([127, 0, 0, 1], 0))
                .with_hybrid(
                    identity
                        .make_acceptor()
                        .expect("build fixture TLS acceptor"),
                    identity.pub_key.clone(),
                )
                .with_input_handler(RecordingInputHandler {
                    events: Arc::clone(&input_events),
                })
                .with_display_handler(FixtureDisplay {
                    size: DesktopSize {
                        width: WIDTH,
                        height: HEIGHT,
                    },
                    updates: VecDeque::from([display_rx]),
                })
                .with_connection_handler(Some(Box::new(StopAfterDisconnect::new(1))))
                .build();
            server.set_credentials(Some(Credentials {
                username: FIXTURE_USER.to_owned(),
                password: FIXTURE_PASSWORD.to_owned(),
                domain: None,
            }));
            let server_events = server.event_sender().clone();
            let server_task = tokio::task::spawn_local(async move { server.run().await });
            let port = server_port(&server_events).await;

            let mut child = Command::new(env!("CARGO_BIN_EXE_mobarust-rdp-helper"))
                .args([
                    "--mobarust-protocol",
                    "rdp",
                    "--host",
                    "127.0.0.1",
                    "--port",
                    &port.to_string(),
                    "--username",
                    FIXTURE_USER,
                    "--reconnect-attempts",
                    "0",
                    "--width",
                    &WIDTH.to_string(),
                    "--height",
                    &HEIGHT.to_string(),
                    "--color-depth",
                    "32",
                ])
                .env("MOBARUST_RDP_FIXTURE_CA", &ca_certificate)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .expect("start the locally built RDP helper");
            let mut stdin = child.stdin.take().expect("RDP helper stdin unavailable");
            let mut stdout = child.stdout.take().expect("RDP helper stdout unavailable");

            assert!(matches!(
                next_event(&mut stdout, "hello").await,
                HelperEvent::Hello { version: 1 }
            ));
            assert!(matches!(
                next_event(&mut stdout, "starting").await,
                HelperEvent::State {
                    state: HelperState::Starting
                }
            ));
            send_frame(
                &mut stdin,
                &encode_command_frame(&HelperCommand::Start {
                    protocol: mobarust_remote_desktop::DesktopProtocol::Rdp,
                    display: DisplaySize {
                        width: WIDTH,
                        height: HEIGHT,
                    },
                })
                .expect("encode RDP start command"),
            )
            .await;
            send_frame(
                &mut stdin,
                &encode_credential_frame(&HelperCredential::new(FIXTURE_PASSWORD))
                    .expect("encode RDP fixture credential"),
            )
            .await;
            stdin.flush().await.expect("flush RDP start frames");

            assert!(matches!(
                next_event(&mut stdout, "ready").await,
                HelperEvent::State {
                    state: HelperState::Ready
                }
            ));
            assert!(matches!(
                next_event(&mut stdout, "capabilities").await,
                HelperEvent::Capabilities { capabilities }
                    if capabilities.protocol == mobarust_remote_desktop::DesktopProtocol::Rdp
                        && capabilities.transport_encrypted
                        && capabilities.color_depths == vec![16, 32]
            ));

            let mut saw_active = false;
            let mut saw_frame = false;
            for _ in 0..8 {
                match next_event(&mut stdout, "real server session").await {
                    HelperEvent::State {
                        state: HelperState::Active,
                    } => saw_active = true,
                    HelperEvent::Framebuffer {
                        width,
                        height,
                        pixels,
                    } => {
                        assert_eq!((width, height), (WIDTH, HEIGHT));
                        assert_eq!(pixels.len(), usize::from(WIDTH) * usize::from(HEIGHT) * 4);
                        assert!(
                            pixels
                                .chunks_exact(4)
                                .any(|pixel| pixel[0] != 0 || pixel[1] != 0 || pixel[2] != 0),
                            "real RDP fixture returned only black pixels; first pixel: {:?}",
                            pixels.get(..4)
                        );
                        saw_frame = true;
                    }
                    HelperEvent::Diagnostic { message, .. } => {
                        assert!(!message.contains(FIXTURE_PASSWORD));
                        panic!("real RDP fixture emitted a diagnostic: {message}");
                    }
                    _ => {}
                }
                if saw_active && saw_frame {
                    break;
                }
            }
            assert!(saw_active, "real RDP helper never reported Active");
            assert!(
                saw_frame,
                "real RDP helper never forwarded the server framebuffer"
            );

            send_frame(
                &mut stdin,
                &encode_command_frame(&HelperCommand::Key {
                    scancode: 0x1e,
                    pressed: true,
                })
                .expect("encode RDP keyboard input"),
            )
            .await;
            send_frame(
                &mut stdin,
                &encode_command_frame(&HelperCommand::Pointer {
                    x: 17,
                    y: 23,
                    buttons: 0,
                })
                .expect("encode RDP pointer input"),
            )
            .await;
            stdin.flush().await.expect("flush RDP input frames");

            timeout(Duration::from_secs(3), async {
                loop {
                    let events = input_events
                        .lock()
                        .expect("fixture input mutex poisoned")
                        .clone();
                    if events.contains(&InputKind::Keyboard) && events.contains(&InputKind::Mouse) {
                        break;
                    }
                    sleep(Duration::from_millis(25)).await;
                }
            })
            .await
            .expect("real RDP server did not receive helper keyboard and pointer input");

            send_frame(
                &mut stdin,
                &encode_command_frame(&HelperCommand::Stop).expect("encode RDP stop command"),
            )
            .await;
            assert!(matches!(
                next_event(&mut stdout, "stopping").await,
                HelperEvent::State {
                    state: HelperState::Stopping
                }
            ));
            assert!(matches!(
                next_event(&mut stdout, "stopped").await,
                HelperEvent::State {
                    state: HelperState::Stopped
                }
            ));

            let status = timeout(Duration::from_secs(5), child.wait())
                .await
                .expect("RDP helper did not exit after Stop")
                .expect("could not wait for RDP helper");
            assert!(
                status.success(),
                "RDP helper exited unsuccessfully: {status}"
            );

            let server_result = timeout(Duration::from_secs(5), server_task)
                .await
                .expect("RDP fixture server did not stop after the helper disconnected")
                .expect("RDP fixture server task panicked");
            assert!(
                server_result.is_ok(),
                "RDP fixture server failed: {server_result:?}"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn real_helper_reconnects_after_real_loopback_server_loss() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let _ = ironrdp_server::tokio_rustls::rustls::crypto::ring::default_provider()
                .install_default();
            let fixture = fixture_directory();
            let (certificate, private_key, ca_certificate) = create_fixture_identity(&fixture.0);
            let identity = TlsIdentityCtx::init_from_paths(&certificate, &private_key)
                .expect("load the disposable RDP fixture identity");
            let (_first_sender, first_receiver) = queued_bitmap([0xff, 0x19, 0x4d, 0x74]).await;
            let (second_sender, second_receiver) = queued_bitmap([0xff, 0xa4, 0x3f, 0x7a]).await;
            drop(_first_sender);

            let mut server = RdpServer::builder()
                .with_addr(([127, 0, 0, 1], 0))
                .with_hybrid(
                    identity
                        .make_acceptor()
                        .expect("build fixture TLS acceptor"),
                    identity.pub_key.clone(),
                )
                .with_no_input()
                .with_display_handler(FixtureDisplay {
                    size: DesktopSize {
                        width: WIDTH,
                        height: HEIGHT,
                    },
                    updates: VecDeque::from([first_receiver, second_receiver]),
                })
                .with_connection_handler(Some(Box::new(StopAfterDisconnect::new(2))))
                .build();
            server.set_credentials(Some(Credentials {
                username: FIXTURE_USER.to_owned(),
                password: FIXTURE_PASSWORD.to_owned(),
                domain: None,
            }));
            let server_events = server.event_sender().clone();
            let server_task = tokio::task::spawn_local(async move { server.run().await });
            let port = server_port(&server_events).await;

            let mut child = Command::new(env!("CARGO_BIN_EXE_mobarust-rdp-helper"))
                .args([
                    "--mobarust-protocol",
                    "rdp",
                    "--host",
                    "127.0.0.1",
                    "--port",
                    &port.to_string(),
                    "--username",
                    FIXTURE_USER,
                    "--reconnect-enabled",
                    "--reconnect-attempts",
                    "1",
                    "--width",
                    &WIDTH.to_string(),
                    "--height",
                    &HEIGHT.to_string(),
                    "--color-depth",
                    "32",
                ])
                .env("MOBARUST_RDP_FIXTURE_CA", &ca_certificate)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .expect("start the locally built RDP helper");
            let mut stdin = child.stdin.take().expect("RDP helper stdin unavailable");
            let mut stdout = child.stdout.take().expect("RDP helper stdout unavailable");

            assert!(matches!(
                next_event(&mut stdout, "hello").await,
                HelperEvent::Hello { version: 1 }
            ));
            assert!(matches!(
                next_event(&mut stdout, "starting").await,
                HelperEvent::State {
                    state: HelperState::Starting
                }
            ));
            send_frame(
                &mut stdin,
                &encode_command_frame(&HelperCommand::Start {
                    protocol: mobarust_remote_desktop::DesktopProtocol::Rdp,
                    display: DisplaySize {
                        width: WIDTH,
                        height: HEIGHT,
                    },
                })
                .expect("encode RDP start command"),
            )
            .await;
            send_frame(
                &mut stdin,
                &encode_credential_frame(&HelperCredential::new(FIXTURE_PASSWORD))
                    .expect("encode RDP fixture credential"),
            )
            .await;
            stdin.flush().await.expect("flush RDP start frames");

            assert!(matches!(
                next_event(&mut stdout, "ready").await,
                HelperEvent::State {
                    state: HelperState::Ready
                }
            ));
            assert!(matches!(
                next_event(&mut stdout, "capabilities").await,
                HelperEvent::Capabilities { capabilities }
                    if capabilities.protocol == mobarust_remote_desktop::DesktopProtocol::Rdp
                        && capabilities.transport_encrypted
            ));

            let mut first_pixels = None;
            let mut first_active = false;
            for _ in 0..8 {
                match next_event(&mut stdout, "first real server session").await {
                    HelperEvent::State {
                        state: HelperState::Active,
                    } => first_active = true,
                    HelperEvent::Framebuffer {
                        width,
                        height,
                        pixels,
                    } => {
                        assert_eq!((width, height), (WIDTH, HEIGHT));
                        assert_eq!(pixels.len(), usize::from(WIDTH) * usize::from(HEIGHT) * 4);
                        first_pixels = Some(pixels);
                    }
                    HelperEvent::Diagnostic { message, .. } => {
                        assert!(!message.contains(FIXTURE_PASSWORD));
                        panic!("real RDP reconnect fixture emitted a diagnostic: {message}");
                    }
                    _ => {}
                }
                if first_active && first_pixels.is_some() {
                    break;
                }
            }
            let first_pixels =
                first_pixels.expect("first RDP fixture framebuffer was not received");
            assert!(
                first_active,
                "first RDP fixture session never became Active"
            );

            let mut saw_reconnecting = false;
            let mut saw_fresh_starting = false;
            let mut second_active = false;
            let mut second_pixels = None;
            for _ in 0..20 {
                match next_event(&mut stdout, "RDP reconnect").await {
                    HelperEvent::State {
                        state: HelperState::Reconnecting,
                    } => saw_reconnecting = true,
                    HelperEvent::State {
                        state: HelperState::Starting,
                    } if saw_reconnecting => saw_fresh_starting = true,
                    HelperEvent::State {
                        state: HelperState::Active,
                    } if saw_fresh_starting => second_active = true,
                    HelperEvent::Framebuffer {
                        width,
                        height,
                        pixels,
                    } if saw_fresh_starting => {
                        assert_eq!((width, height), (WIDTH, HEIGHT));
                        assert_eq!(pixels.len(), usize::from(WIDTH) * usize::from(HEIGHT) * 4);
                        second_pixels = Some(pixels);
                    }
                    HelperEvent::State {
                        state: HelperState::Failed,
                    } => panic!("RDP helper failed instead of reconnecting"),
                    HelperEvent::Diagnostic { message, .. } => {
                        assert!(!message.contains(FIXTURE_PASSWORD));
                        panic!("real RDP reconnect fixture emitted a diagnostic: {message}");
                    }
                    _ => {}
                }
                if second_active && second_pixels.is_some() {
                    break;
                }
            }
            assert!(
                saw_reconnecting,
                "RDP helper did not report the real connection loss"
            );
            assert!(
                saw_fresh_starting,
                "RDP helper did not start a fresh attempt after connection loss"
            );
            assert!(second_active, "reconnected RDP session never became Active");
            let second_pixels =
                second_pixels.expect("reconnected RDP framebuffer was not received");
            assert_ne!(
                first_pixels, second_pixels,
                "reconnected fixture framebuffer was stale"
            );

            send_frame(
                &mut stdin,
                &encode_command_frame(&HelperCommand::Stop).expect("encode RDP stop command"),
            )
            .await;
            stdin.flush().await.expect("flush RDP stop frame");
            assert!(matches!(
                next_event(&mut stdout, "stopping").await,
                HelperEvent::State {
                    state: HelperState::Stopping
                }
            ));
            assert!(matches!(
                next_event(&mut stdout, "stopped").await,
                HelperEvent::State {
                    state: HelperState::Stopped
                }
            ));
            drop(second_sender);

            let status = timeout(Duration::from_secs(5), child.wait())
                .await
                .expect("RDP helper did not exit after Stop")
                .expect("could not wait for RDP helper");
            assert!(
                status.success(),
                "RDP helper exited unsuccessfully after reconnect: {status}"
            );

            let server_result = timeout(Duration::from_secs(5), server_task)
                .await
                .expect("RDP fixture server did not stop after the second disconnect")
                .expect("RDP fixture server task panicked");
            assert!(
                server_result.is_ok(),
                "RDP reconnect fixture server failed: {server_result:?}"
            );
        })
        .await;
}
