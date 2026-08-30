use std::process::Stdio;
use std::time::Duration;

use des::Des;
use des::cipher::{Block, BlockCipherEncrypt, KeyInit};
use mobarust_remote_desktop::{
    DesktopProtocol, DisplaySize, HelperCommand, HelperCredential, HelperEvent, HelperState,
    decode_event_frame, encode_command_frame, read_frame, write_credential_frame,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::time::timeout;

const FIXTURE_SIZE: DisplaySize = DisplaySize {
    width: 320,
    height: 200,
};
const FIXTURE_PASSWORD: &str = "mobarust-vnc-fixture";
const FIXTURE_CHALLENGE: [u8; 16] = [
    0x6d, 0x6f, 0x62, 0x61, 0x72, 0x75, 0x73, 0x74, 0x2d, 0x76, 0x6e, 0x63, 0x2d, 0x31, 0x36, 0x21,
];

#[derive(Clone, Copy)]
enum FixtureAuth {
    None,
    VncPassword,
}

#[tokio::test]
async fn helper_controls_a_real_rfb_fixture_over_loopback() {
    exercise_fixture(FixtureAuth::None, "").await;
}

#[tokio::test]
async fn helper_authenticates_with_vnc_password_fixture_over_loopback() {
    exercise_fixture(FixtureAuth::VncPassword, FIXTURE_PASSWORD).await;
}

#[tokio::test]
async fn helper_reports_server_disconnect_during_negotiation() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        stream.write_all(b"RFB 003.008\n").await.unwrap();
        drop(stream);
    });
    let (mut child, mut stdin, mut stdout) = spawn_helper(port).await;
    send_command(
        &mut stdin,
        HelperCommand::Start {
            protocol: DesktopProtocol::Vnc,
            display: FIXTURE_SIZE,
        },
    )
    .await;
    write_credential_frame(&mut stdin, &HelperCredential::new("fixture-secret"))
        .await
        .unwrap();

    let mut saw_failed = false;
    for _ in 0..4 {
        let event = timeout(Duration::from_secs(3), next_event(&mut stdout))
            .await
            .unwrap();
        if matches!(
            event,
            HelperEvent::State {
                state: HelperState::Failed
            }
        ) {
            saw_failed = true;
            break;
        }
    }
    assert!(saw_failed, "the helper did not report the RFB disconnect");
    server_task.await.unwrap();
    drop(stdin);
    timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn helper_cancels_a_stalled_negotiation_without_waiting_for_timeout() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (accepted_tx, accepted_rx) = oneshot::channel();
    let server_task = tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.unwrap();
        let _ = accepted_tx.send(());
        tokio::time::sleep(Duration::from_secs(5)).await;
    });
    let (mut child, mut stdin, mut stdout) = spawn_helper(port).await;
    send_command(
        &mut stdin,
        HelperCommand::Start {
            protocol: DesktopProtocol::Vnc,
            display: FIXTURE_SIZE,
        },
    )
    .await;
    write_credential_frame(&mut stdin, &HelperCredential::new("fixture-secret"))
        .await
        .unwrap();
    timeout(Duration::from_secs(2), accepted_rx)
        .await
        .unwrap()
        .unwrap();
    send_command(&mut stdin, HelperCommand::Stop).await;

    let mut saw_stopped = false;
    for _ in 0..4 {
        let event = timeout(Duration::from_secs(2), next_event(&mut stdout))
            .await
            .unwrap();
        if matches!(
            event,
            HelperEvent::State {
                state: HelperState::Stopped
            }
        ) {
            saw_stopped = true;
            break;
        }
    }
    assert!(
        saw_stopped,
        "the helper did not cancel negotiation promptly"
    );
    server_task.await.unwrap();
    drop(stdin);
    timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
}

async fn exercise_fixture(auth: FixtureAuth, password: &str) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server_task = tokio::spawn(run_fixture(listener, auth));

    let mut child = Command::new(env!("CARGO_BIN_EXE_mobarust-vnc-helper"))
        .args([
            "--mobarust-protocol",
            "vnc",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--width",
            "320",
            "--height",
            "200",
        ])
        .env_remove("SSH_AUTH_SOCK")
        .env_remove("SSH_AGENT_PID")
        .env_remove("GIT_SSH_COMMAND")
        .env_remove("GIT_CONFIG_GLOBAL")
        .env_remove("GIT_CONFIG_SYSTEM")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    assert!(matches!(
        next_event(&mut stdout).await,
        HelperEvent::Hello { version: 1 }
    ));
    assert!(matches!(
        next_event(&mut stdout).await,
        HelperEvent::State {
            state: HelperState::Starting
        }
    ));

    send_command(
        &mut stdin,
        HelperCommand::Start {
            protocol: DesktopProtocol::Vnc,
            display: FIXTURE_SIZE,
        },
    )
    .await;
    write_credential_frame(&mut stdin, &HelperCredential::new(password))
        .await
        .unwrap();

    let mut saw_active_frame = false;
    for _ in 0..8 {
        let event = match timeout(Duration::from_secs(3), next_event(&mut stdout)).await {
            Ok(event) => event,
            Err(_) => {
                let status = child.try_wait().unwrap();
                panic!("helper emitted no framebuffer event; child={status:?}");
            }
        };
        if matches!(
            event,
            HelperEvent::State {
                state: HelperState::Active
            }
        ) {
            saw_active_frame = true;
        }
        if matches!(event, HelperEvent::Framebuffer { width: 320, height: 200, ref pixels } if pixels[..4] == [0x11, 0x22, 0x33, 0xff])
        {
            saw_active_frame = true;
            break;
        }
    }
    assert!(
        saw_active_frame,
        "the helper did not emit a real framebuffer event"
    );

    send_command(
        &mut stdin,
        HelperCommand::Key {
            scancode: 0x41,
            pressed: true,
        },
    )
    .await;
    send_command(
        &mut stdin,
        HelperCommand::Pointer {
            x: 12,
            y: 18,
            buttons: 1,
        },
    )
    .await;

    timeout(Duration::from_secs(3), server_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    send_command(&mut stdin, HelperCommand::Stop).await;
    let mut saw_stopped = false;
    for _ in 0..5 {
        let event = timeout(Duration::from_secs(2), next_event(&mut stdout))
            .await
            .unwrap();
        if matches!(
            event,
            HelperEvent::State {
                state: HelperState::Stopped
            }
        ) {
            saw_stopped = true;
            break;
        }
    }
    assert!(saw_stopped, "the helper did not stop cleanly");
    drop(stdin);
    timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
}

async fn spawn_helper(
    port: u16,
) -> (
    tokio::process::Child,
    tokio::process::ChildStdin,
    tokio::process::ChildStdout,
) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mobarust-vnc-helper"))
        .args([
            "--mobarust-protocol",
            "vnc",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--width",
            "320",
            "--height",
            "200",
        ])
        .env_remove("SSH_AUTH_SOCK")
        .env_remove("SSH_AGENT_PID")
        .env_remove("GIT_SSH_COMMAND")
        .env_remove("GIT_CONFIG_GLOBAL")
        .env_remove("GIT_CONFIG_SYSTEM")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    assert!(matches!(
        next_event(&mut stdout).await,
        HelperEvent::Hello { version: 1 }
    ));
    assert!(matches!(
        next_event(&mut stdout).await,
        HelperEvent::State {
            state: HelperState::Starting
        }
    ));
    (child, stdin, stdout)
}

async fn send_command(stdin: &mut tokio::process::ChildStdin, command: HelperCommand) {
    let frame = encode_command_frame(&command).unwrap();
    stdin.write_all(&frame[..]).await.unwrap();
    stdin.flush().await.unwrap();
}

async fn next_event(stdout: &mut tokio::process::ChildStdout) -> HelperEvent {
    let frame = read_frame(stdout).await.unwrap().unwrap();
    decode_event_frame(&frame).unwrap()
}

async fn run_fixture(listener: TcpListener, auth: FixtureAuth) -> Result<(), String> {
    let (mut stream, _) = listener.accept().await.map_err(|error| error.to_string())?;
    stream
        .write_all(b"RFB 003.008\n")
        .await
        .map_err(|error| error.to_string())?;
    let mut version = [0_u8; 12];
    stream
        .read_exact(&mut version)
        .await
        .map_err(|error| error.to_string())?;
    if &version != b"RFB 003.008\n" {
        return Err("unexpected RFB version".into());
    }

    match auth {
        FixtureAuth::None => {
            stream
                .write_all(&[1, 1])
                .await
                .map_err(|error| error.to_string())?;
        }
        FixtureAuth::VncPassword => {
            stream
                .write_all(&[1, 2])
                .await
                .map_err(|error| error.to_string())?;
        }
    }
    let mut selected = [0_u8; 1];
    stream
        .read_exact(&mut selected)
        .await
        .map_err(|error| error.to_string())?;
    match auth {
        FixtureAuth::None if selected[0] != 1 => {
            return Err("helper did not select no-auth fixture mode".into());
        }
        FixtureAuth::VncPassword if selected[0] != 2 => {
            return Err("helper did not select VNC password fixture mode".into());
        }
        _ => {}
    }

    if matches!(auth, FixtureAuth::VncPassword) {
        stream
            .write_all(&FIXTURE_CHALLENGE)
            .await
            .map_err(|error| error.to_string())?;
        let mut response = [0_u8; 16];
        stream
            .read_exact(&mut response)
            .await
            .map_err(|error| error.to_string())?;
        if response != vnc_auth_response(&FIXTURE_CHALLENGE, FIXTURE_PASSWORD) {
            return Err("helper produced an invalid VNC password response".into());
        }
    }

    stream
        .write_all(&[0, 0, 0, 0])
        .await
        .map_err(|error| error.to_string())?;

    let mut shared = [0_u8; 1];
    stream
        .read_exact(&mut shared)
        .await
        .map_err(|error| error.to_string())?;
    let mut server_init = Vec::with_capacity(64);
    server_init.extend_from_slice(&FIXTURE_SIZE.width.to_be_bytes());
    server_init.extend_from_slice(&FIXTURE_SIZE.height.to_be_bytes());
    server_init.extend_from_slice(&[32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 0, 0, 0, 8, 0, 16]);
    server_init.extend_from_slice(&7_u32.to_be_bytes());
    server_init.extend_from_slice(b"fixture");
    stream
        .write_all(&server_init)
        .await
        .map_err(|error| error.to_string())?;

    read_set_pixel_format(&mut stream).await?;
    read_set_encodings(&mut stream).await?;
    read_update_request(&mut stream).await?;

    let mut update = vec![0_u8, 0, 0, 1];
    update.extend_from_slice(&[0, 0, 0, 0, 0, 1, 0, 1]);
    update.extend_from_slice(&[0, 0, 0, 0]);
    update.extend_from_slice(&[0x11, 0x22, 0x33, 0xff]);
    stream
        .write_all(&update)
        .await
        .map_err(|error| error.to_string())?;

    let mut saw_key = false;
    let mut saw_pointer = false;
    while !(saw_key && saw_pointer) {
        let mut kind = [0_u8; 1];
        timeout(Duration::from_secs(3), stream.read_exact(&mut kind))
            .await
            .map_err(|_| "fixture timed out waiting for helper input".to_owned())?
            .map_err(|error| error.to_string())?;
        match kind[0] {
            3 => {
                let mut payload = [0_u8; 9];
                stream
                    .read_exact(&mut payload)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            4 => {
                let mut payload = [0_u8; 7];
                stream
                    .read_exact(&mut payload)
                    .await
                    .map_err(|error| error.to_string())?;
                saw_key = true;
            }
            5 => {
                let mut payload = [0_u8; 5];
                stream
                    .read_exact(&mut payload)
                    .await
                    .map_err(|error| error.to_string())?;
                saw_pointer = true;
            }
            6 => {
                let mut header = [0_u8; 7];
                stream
                    .read_exact(&mut header)
                    .await
                    .map_err(|error| error.to_string())?;
                let length = u32::from_be_bytes(header[3..7].try_into().unwrap()) as usize;
                if length > 1024 * 1024 {
                    return Err("fixture saw an oversized clipboard payload".into());
                }
                let mut text = vec![0_u8; length];
                stream
                    .read_exact(&mut text)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            _ => return Err("fixture saw an unexpected client message".into()),
        }
    }
    Ok(())
}

fn vnc_auth_response(challenge: &[u8; 16], password: &str) -> [u8; 16] {
    let mut key = [0_u8; 8];
    for (index, value) in key.iter_mut().enumerate() {
        *value = password
            .as_bytes()
            .get(index)
            .copied()
            .unwrap_or_default()
            .reverse_bits();
    }
    let cipher = Des::new_from_slice(&key).expect("DES key has the required length");
    let mut response = [0_u8; 16];
    for (source, destination) in challenge.chunks_exact(8).zip(response.chunks_exact_mut(8)) {
        let mut block =
            Block::<Des>::try_from(source).expect("RFB challenge is an exact DES block");
        cipher.encrypt_block(&mut block);
        destination.copy_from_slice(&block);
    }
    response
}

async fn read_set_pixel_format(stream: &mut TcpStream) -> Result<(), String> {
    let mut message = [0_u8; 20];
    stream
        .read_exact(&mut message)
        .await
        .map_err(|error| error.to_string())?;
    if message[0] != 0 {
        return Err("expected SetPixelFormat".into());
    }
    Ok(())
}

async fn read_set_encodings(stream: &mut TcpStream) -> Result<(), String> {
    let mut header = [0_u8; 4];
    stream
        .read_exact(&mut header)
        .await
        .map_err(|error| error.to_string())?;
    if header[0] != 2 {
        return Err("expected SetEncodings".into());
    }
    let count = u16::from_be_bytes([header[2], header[3]]) as usize;
    let mut encodings = vec![0_u8; count * 4];
    stream
        .read_exact(&mut encodings)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

async fn read_update_request(stream: &mut TcpStream) -> Result<(), String> {
    let mut request = [0_u8; 10];
    stream
        .read_exact(&mut request)
        .await
        .map_err(|error| error.to_string())?;
    if request[0] != 3 {
        return Err("expected FramebufferUpdateRequest".into());
    }
    Ok(())
}
