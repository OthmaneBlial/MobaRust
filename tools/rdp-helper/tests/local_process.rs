use std::process::Stdio;
use std::time::Duration;

use mobarust_remote_desktop::{
    DisplaySize, HelperCommand, HelperCredential, HelperEvent, HelperState, decode_event_frame,
    encode_command_frame, encode_credential_frame, read_frame, write_frame_with_timeout,
};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::process::{ChildStdout, Command};
use tokio::time::timeout;

const FIXTURE_SECRET: &str = "fixture-process-secret";

async fn next_event(stdout: &mut ChildStdout, phase: &str) -> HelperEvent {
    let frame = timeout(Duration::from_secs(5), read_frame(stdout))
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

#[tokio::test]
async fn real_helper_process_round_trips_native_start_and_exits_on_closed_loopback_fixture() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("could not reserve a disposable loopback port");
    let port = listener
        .local_addr()
        .expect("disposable loopback listener had no address")
        .port();
    let accept_task = tokio::spawn(async move {
        if let Ok((socket, _)) = listener.accept().await {
            drop(socket);
        }
    });

    let mut child = Command::new(env!("CARGO_BIN_EXE_mobarust-rdp-helper"))
        .args([
            "--mobarust-protocol",
            "rdp",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--username",
            "fixture-user",
            "--width",
            "640",
            "--height",
            "480",
            "--color-depth",
            "32",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("could not start the locally built RDP helper");
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

    let start = encode_command_frame(&HelperCommand::Start {
        protocol: mobarust_remote_desktop::DesktopProtocol::Rdp,
        display: DisplaySize {
            width: 640,
            height: 480,
        },
    })
    .expect("could not encode RDP start command");
    send_frame(&mut stdin, &start).await;

    let credential = encode_credential_frame(&HelperCredential::new(FIXTURE_SECRET))
        .expect("could not encode RDP credential frame");
    send_frame(&mut stdin, &credential).await;
    stdin
        .flush()
        .await
        .expect("could not flush RDP helper stdin");

    assert!(matches!(
        next_event(&mut stdout, "ready").await,
        HelperEvent::State {
            state: HelperState::Ready
        }
    ));

    let mut terminal_state = None;
    for _ in 0..4 {
        match next_event(&mut stdout, "failure").await {
            HelperEvent::Diagnostic { message, .. } => {
                assert!(!message.contains(FIXTURE_SECRET));
            }
            HelperEvent::State {
                state: state @ (HelperState::Failed | HelperState::Stopped | HelperState::Crashed),
            } => {
                terminal_state = Some(state);
                break;
            }
            _ => {}
        }
    }
    assert!(
        terminal_state.is_some(),
        "RDP helper did not report a terminal state"
    );

    let status = timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("RDP helper did not exit after a closed disposable loopback connection")
        .expect("could not wait for RDP helper");
    accept_task.abort();
    let _ = accept_task.await;
    assert!(
        status.success(),
        "RDP helper exited unsuccessfully: {status}"
    );
}
