use std::process::Stdio;
use std::time::Duration;

use mobarust_remote_desktop::{
    DesktopProtocol, DisplaySize, HelperCommand, HelperCredential, HelperEvent, HelperState,
    decode_event_frame, encode_command_frame, read_frame, write_credential_frame,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::time::timeout;

const FIXTURE_SIZE: DisplaySize = DisplaySize {
    width: 320,
    height: 200,
};

#[tokio::test]
async fn helper_controls_a_real_rfb_fixture_over_loopback() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server_task = tokio::spawn(run_fixture(listener));

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
    write_credential_frame(&mut stdin, &HelperCredential::new(""))
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

async fn send_command(stdin: &mut tokio::process::ChildStdin, command: HelperCommand) {
    let frame = encode_command_frame(&command).unwrap();
    stdin.write_all(&frame[..]).await.unwrap();
    stdin.flush().await.unwrap();
}

async fn next_event(stdout: &mut tokio::process::ChildStdout) -> HelperEvent {
    let frame = read_frame(stdout).await.unwrap().unwrap();
    decode_event_frame(&frame).unwrap()
}

async fn run_fixture(listener: TcpListener) -> Result<(), String> {
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

    stream
        .write_all(&[1, 1])
        .await
        .map_err(|error| error.to_string())?;
    let mut selected = [0_u8; 1];
    stream
        .read_exact(&mut selected)
        .await
        .map_err(|error| error.to_string())?;
    if selected[0] != 1 {
        return Err("helper did not select no-auth fixture mode".into());
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
