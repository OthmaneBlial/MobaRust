use std::process::Stdio;
use std::time::Duration;

use base64::Engine as _;
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
const RESIZED_FIXTURE_SIZE: DisplaySize = DisplaySize {
    width: 640,
    height: 400,
};
const FIXTURE_PASSWORD: &str = "mobarust-vnc-fixture";
const FIXTURE_KEYSYM: u32 = 0x0100_20ac;
const FIXTURE_CHALLENGE: [u8; 16] = [
    0x6d, 0x6f, 0x62, 0x61, 0x72, 0x75, 0x73, 0x74, 0x2d, 0x76, 0x6e, 0x63, 0x2d, 0x31, 0x36, 0x21,
];
const BALANCED_ENCODINGS: &[i32] = &[16, 1, 0, -239, -223];
const LOW_LATENCY_ENCODINGS: &[i32] = &[0, 1, 16, -239, -223];
const LOW_BANDWIDTH_ENCODINGS: &[i32] = &[7, 16, 1, 0, -239, -223];

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
async fn helper_decodes_tight_jpeg_framebuffer_fixture() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (release_tx, release_rx) = oneshot::channel();
    let server_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        serve_tight_framebuffer_connection(&mut stream).await?;
        let _ = release_rx.await;
        Ok::<(), String>(())
    });

    let (mut child, mut stdin, mut stdout) = spawn_helper_with_quality(port, "low-bandwidth").await;
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

    let mut saw_framebuffer = false;
    for _ in 0..8 {
        if let HelperEvent::Framebuffer {
            width: 320,
            height: 200,
            ref pixels,
        } = timeout(Duration::from_secs(3), next_event(&mut stdout))
            .await
            .unwrap()
        {
            assert!(pixels[..8].chunks_exact(4).all(|pixel| pixel[3] == 0xff));
            saw_framebuffer = true;
            break;
        }
    }
    assert!(
        saw_framebuffer,
        "the helper did not emit the Tight JPEG frame"
    );

    send_command(&mut stdin, HelperCommand::Stop).await;
    let mut saw_stopped = false;
    for _ in 0..6 {
        if matches!(
            timeout(Duration::from_secs(2), next_event(&mut stdout))
                .await
                .unwrap(),
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
        "the helper did not stop after the JPEG fixture"
    );
    let _ = release_tx.send(());
    server_task.await.unwrap().unwrap();
    timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn helper_rejects_out_of_bounds_framebuffer_fixture() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        prepare_vnc_connection(&mut stream).await?;
        let mut update = vec![0_u8, 0, 0, 1];
        update.extend_from_slice(&[0x01, 0x3f, 0, 0, 0, 2, 0, 1]);
        update.extend_from_slice(&0_i32.to_be_bytes());
        update.extend_from_slice(&[0x11, 0x22, 0x33, 0xff, 0x44, 0x55, 0x66, 0xff]);
        stream
            .write_all(&update)
            .await
            .map_err(|error| error.to_string())
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
    write_credential_frame(&mut stdin, &HelperCredential::new(""))
        .await
        .unwrap();

    let mut saw_diagnostic = false;
    let mut saw_failed = false;
    for _ in 0..8 {
        match timeout(Duration::from_secs(3), next_event(&mut stdout))
            .await
            .unwrap()
        {
            HelperEvent::Diagnostic { ref message, .. }
                if message == "VNC framebuffer update was invalid" =>
            {
                saw_diagnostic = true
            }
            HelperEvent::State {
                state: HelperState::Failed,
            } => {
                saw_failed = true;
                break;
            }
            _ => {}
        }
    }
    assert!(
        saw_diagnostic,
        "the helper did not report the invalid framebuffer"
    );
    assert!(
        saw_failed,
        "the helper did not fail closed on the invalid framebuffer"
    );
    server_task.await.unwrap().unwrap();
    timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn helper_rejects_invalid_tight_jpeg_fixture() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        prepare_vnc_connection(&mut stream).await?;
        let mut update = vec![0_u8, 0, 0, 1];
        update.extend_from_slice(&[0, 0, 0, 0, 0, 2, 0, 2]);
        update.extend_from_slice(&7_i32.to_be_bytes());
        update.push(0x90);
        update.extend_from_slice(&[3, 0xde, 0xad, 0xbe]);
        stream
            .write_all(&update)
            .await
            .map_err(|error| error.to_string())
    });

    let (mut child, mut stdin, mut stdout) = spawn_helper_with_quality(port, "low-bandwidth").await;
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

    let mut saw_diagnostic = false;
    let mut saw_failed = false;
    for _ in 0..8 {
        match timeout(Duration::from_secs(3), next_event(&mut stdout))
            .await
            .unwrap()
        {
            HelperEvent::Diagnostic { ref message, .. }
                if message == "VNC framebuffer update was invalid" =>
            {
                saw_diagnostic = true
            }
            HelperEvent::State {
                state: HelperState::Failed,
            } => {
                saw_failed = true;
                break;
            }
            _ => {}
        }
    }
    assert!(
        saw_diagnostic,
        "the helper did not report the invalid Tight JPEG"
    );
    assert!(
        saw_failed,
        "the helper did not fail closed on the invalid Tight JPEG"
    );
    server_task.await.unwrap().unwrap();
    timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn helper_sends_the_selected_quality_encodings_to_the_server() {
    for (quality, expected_encodings) in [
        ("balanced", BALANCED_ENCODINGS),
        ("low-latency", LOW_LATENCY_ENCODINGS),
        ("low-bandwidth", LOW_BANDWIDTH_ENCODINGS),
    ] {
        exercise_fixture_with_quality(FixtureAuth::None, "", quality, expected_encodings).await;
    }
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
    timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn helper_reconnects_after_a_connected_server_disconnects() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (release_tx, release_rx) = oneshot::channel();
    let server_task = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        serve_framebuffer_connection(&mut first, [0x11, 0x22, 0x33, 0xff])
            .await
            .unwrap();
        drop(first);

        let (mut second, _) = listener.accept().await.unwrap();
        serve_framebuffer_connection(&mut second, [0x44, 0x55, 0x66, 0xff])
            .await
            .unwrap();
        let _ = release_rx.await;
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
    write_credential_frame(&mut stdin, &HelperCredential::new(""))
        .await
        .unwrap();

    let mut saw_reconnecting = false;
    let mut saw_reconnected_frame = false;
    for _ in 0..20 {
        let event = match timeout(Duration::from_secs(3), next_event(&mut stdout)).await {
            Ok(event) => event,
            Err(_) => {
                eprintln!(
                    "reconnect fixture timed out: child={:?} server_finished={}",
                    child.try_wait().unwrap(),
                    server_task.is_finished()
                );
                panic!("reconnect fixture emitted no event");
            }
        };
        match event {
            HelperEvent::State {
                state: HelperState::Reconnecting,
            } => saw_reconnecting = true,
            HelperEvent::Framebuffer {
                ref pixels,
                width: 320,
                height: 200,
            } if saw_reconnecting && pixels[..4] == [0x44, 0x55, 0x66, 0xff] => {
                saw_reconnected_frame = true;
                break;
            }
            _ => {}
        }
    }
    assert!(
        saw_reconnecting,
        "the helper did not expose reconnecting state"
    );
    assert!(
        saw_reconnected_frame,
        "the helper did not emit the second real framebuffer"
    );

    send_command(&mut stdin, HelperCommand::Stop).await;
    let _ = release_tx.send(());
    let mut saw_stopped = false;
    for _ in 0..6 {
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
    assert!(saw_stopped, "the helper did not stop after reconnecting");
    server_task.await.unwrap();
    timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn helper_bounds_reconnect_attempts_after_the_fixture_goes_away() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        serve_framebuffer_connection(&mut stream, [0x11, 0x22, 0x33, 0xff])
            .await
            .unwrap();
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
    write_credential_frame(&mut stdin, &HelperCredential::new(""))
        .await
        .unwrap();

    timeout(Duration::from_secs(8), async {
        loop {
            if matches!(
                next_event(&mut stdout).await,
                HelperEvent::State {
                    state: HelperState::Failed
                }
            ) {
                break;
            }
        }
    })
    .await
    .unwrap();
    server_task.await.unwrap();
    timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn helper_fails_without_reconnect_when_the_policy_is_disabled() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        serve_framebuffer_connection(&mut stream, [0x11, 0x22, 0x33, 0xff])
            .await
            .unwrap();
    });

    let (mut child, mut stdin, mut stdout) = spawn_helper_with_policy(port, false, 0).await;
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

    let mut saw_active = false;
    let mut saw_reconnecting = false;
    let mut saw_failed = false;
    for _ in 0..8 {
        let event = timeout(Duration::from_secs(3), next_event(&mut stdout))
            .await
            .unwrap();
        match event {
            HelperEvent::State {
                state: HelperState::Active,
            } => saw_active = true,
            HelperEvent::State {
                state: HelperState::Reconnecting,
            } => saw_reconnecting = true,
            HelperEvent::State {
                state: HelperState::Failed,
            } => {
                saw_failed = true;
                break;
            }
            _ => {}
        }
    }

    assert!(
        saw_active,
        "the helper did not establish the fixture session"
    );
    assert!(
        saw_failed,
        "the helper did not fail after the disabled policy"
    );
    assert!(
        !saw_reconnecting,
        "disabled reconnect emitted a retry state"
    );
    server_task.await.unwrap();
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
    timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn helper_cancels_an_idle_connected_session_without_waiting_for_remote_data() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (release_tx, release_rx) = oneshot::channel();
    let server_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        serve_framebuffer_connection(&mut stream, [0x11, 0x22, 0x33, 0xff])
            .await
            .unwrap();
        let _ = release_rx.await;
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
    write_credential_frame(&mut stdin, &HelperCredential::new(""))
        .await
        .unwrap();

    let mut saw_capabilities = false;
    let mut saw_framebuffer = false;
    for _ in 0..8 {
        match timeout(Duration::from_secs(3), next_event(&mut stdout))
            .await
            .unwrap()
        {
            HelperEvent::Capabilities { capabilities }
                if capabilities.protocol == DesktopProtocol::Vnc
                    && !capabilities.clipboard
                    && !capabilities.server_resize
                    && capabilities.local_scaling
                    && !capabilities.gateway
                    && !capabilities.transport_encrypted =>
            {
                saw_capabilities = true;
            }
            HelperEvent::Framebuffer {
                width: 320,
                height: 200,
                ref pixels,
            } if pixels[..4] == [0x11, 0x22, 0x33, 0xff] => {
                saw_framebuffer = true;
                break;
            }
            _ => {}
        }
    }
    assert!(
        saw_capabilities,
        "the helper did not report VNC capabilities"
    );
    assert!(saw_framebuffer, "the helper did not become active");

    send_command(&mut stdin, HelperCommand::Stop).await;
    let mut saw_stopped = false;
    for _ in 0..6 {
        if matches!(
            timeout(Duration::from_millis(500), next_event(&mut stdout))
                .await
                .unwrap(),
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
        "the helper did not cancel an idle connected session promptly"
    );
    let _ = release_tx.send(());
    server_task.await.unwrap();
    timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn helper_rejects_clipboard_input_without_opt_in_at_the_socket_boundary() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (probe_tx, probe_rx) = oneshot::channel();
    let (checked_tx, checked_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let server_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        serve_framebuffer_connection(&mut stream, [0x11, 0x22, 0x33, 0xff])
            .await
            .unwrap();
        probe_rx.await.map_err(|error| error.to_string())?;

        let mut saw_clipboard = false;
        let mut read_error = None;
        for _ in 0..8 {
            match timeout(
                Duration::from_millis(150),
                read_client_message_kind(&mut stream),
            )
            .await
            {
                Ok(Ok(6)) => {
                    saw_clipboard = true;
                    break;
                }
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    read_error = Some(error);
                    break;
                }
                Err(_) => break,
            }
        }
        let _ = checked_tx.send(saw_clipboard);
        let _ = release_rx.await;
        if saw_clipboard {
            Err("the helper forwarded clipboard input without opt-in".into())
        } else if let Some(error) = read_error {
            Err(error)
        } else {
            Ok(())
        }
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
    write_credential_frame(&mut stdin, &HelperCredential::new(""))
        .await
        .unwrap();

    let mut saw_framebuffer = false;
    for _ in 0..8 {
        match timeout(Duration::from_secs(3), next_event(&mut stdout))
            .await
            .unwrap()
        {
            HelperEvent::Framebuffer { .. } => {
                saw_framebuffer = true;
                break;
            }
            HelperEvent::Capabilities { capabilities } => {
                assert!(!capabilities.clipboard);
            }
            _ => {}
        }
    }
    assert!(saw_framebuffer, "the helper did not become active");

    send_command(
        &mut stdin,
        HelperCommand::Clipboard {
            text: "must stay native".to_owned().into(),
        },
    )
    .await;
    assert!(matches!(
        timeout(Duration::from_secs(2), next_event(&mut stdout))
            .await
            .unwrap(),
        HelperEvent::Diagnostic { message, .. }
            if message == "VNC clipboard input is disabled without explicit opt-in"
    ));
    probe_tx.send(()).unwrap();
    assert!(!checked_rx.await.unwrap());

    send_command(&mut stdin, HelperCommand::Stop).await;
    let mut saw_stopped = false;
    for _ in 0..5 {
        if matches!(
            timeout(Duration::from_secs(2), next_event(&mut stdout))
                .await
                .unwrap(),
            HelperEvent::State {
                state: HelperState::Stopped
            }
        ) {
            saw_stopped = true;
            break;
        }
    }
    assert!(saw_stopped, "the helper did not stop cleanly");
    let _ = release_tx.send(());
    server_task.await.unwrap().unwrap();
    timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
}

async fn exercise_fixture(auth: FixtureAuth, password: &str) {
    exercise_fixture_with_quality(auth, password, "balanced", BALANCED_ENCODINGS).await;
}

async fn exercise_fixture_with_quality(
    auth: FixtureAuth,
    password: &str,
    quality: &str,
    expected_encodings: &[i32],
) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server_task = tokio::spawn(run_fixture(listener, auth, expected_encodings.to_vec()));
    let (mut child, mut stdin, mut stdout) = spawn_helper_with_quality(port, quality).await;

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

    let mut saw_active = false;
    let mut saw_framebuffer = false;
    let mut saw_remote_clipboard = false;
    let mut saw_resized_framebuffer = false;
    let mut saw_clipboard_capability = false;
    for _ in 0..12 {
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
            saw_active = true;
        }
        if matches!(event, HelperEvent::Framebuffer { width: 320, height: 200, ref pixels } if pixels[..4] == [0x11, 0x22, 0x33, 0xff])
        {
            saw_framebuffer = true;
        }
        if matches!(event, HelperEvent::Clipboard { ref text } if text.as_str() == "remote fixture clipboard")
        {
            saw_remote_clipboard = true;
        }
        if matches!(event, HelperEvent::Capabilities { ref capabilities } if capabilities.clipboard)
        {
            saw_clipboard_capability = true;
        }
        if matches!(
            event,
            HelperEvent::Framebuffer {
                width: 640,
                height: 400,
                ref pixels,
            } if pixels[..4] == [0x44, 0x55, 0x66, 0xff]
        ) {
            saw_resized_framebuffer = true;
        }
        if saw_active
            && saw_framebuffer
            && saw_remote_clipboard
            && saw_resized_framebuffer
            && saw_clipboard_capability
        {
            break;
        }
    }
    assert!(
        saw_active && saw_framebuffer,
        "the helper did not emit a real framebuffer event"
    );
    assert!(
        saw_remote_clipboard,
        "the helper did not forward the server clipboard event"
    );
    assert!(
        saw_clipboard_capability,
        "the helper did not report opted-in clipboard capability"
    );
    assert!(
        saw_resized_framebuffer,
        "the helper did not apply the server-announced framebuffer resize"
    );

    send_command(
        &mut stdin,
        HelperCommand::Resize {
            display: DisplaySize {
                width: 640,
                height: 400,
            },
        },
    )
    .await;
    assert!(matches!(
        timeout(Duration::from_secs(2), next_event(&mut stdout))
            .await
            .unwrap(),
        HelperEvent::Diagnostic { message, .. }
            if message == "VNC server-side resize is not supported; viewport scaling remains local"
    ));

    send_command(
        &mut stdin,
        HelperCommand::Key {
            scancode: FIXTURE_KEYSYM,
            pressed: true,
        },
    )
    .await;
    send_command(
        &mut stdin,
        HelperCommand::Pointer {
            x: u16::MAX,
            y: u16::MAX,
            buttons: 1,
        },
    )
    .await;
    send_command(
        &mut stdin,
        HelperCommand::Wheel {
            x: u16::MAX,
            y: u16::MAX,
            delta: 120,
        },
    )
    .await;
    send_command(
        &mut stdin,
        HelperCommand::Clipboard {
            text: "fixture clipboard".to_owned().into(),
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
    spawn_helper_with_policy(port, true, 3).await
}

async fn spawn_helper_with_policy(
    port: u16,
    reconnect_enabled: bool,
    reconnect_attempts: u8,
) -> (
    tokio::process::Child,
    tokio::process::ChildStdin,
    tokio::process::ChildStdout,
) {
    spawn_helper_with_quality_and_policy_and_clipboard(
        port,
        reconnect_enabled,
        reconnect_attempts,
        "balanced",
        false,
    )
    .await
}

async fn spawn_helper_with_quality(
    port: u16,
    quality: &str,
) -> (
    tokio::process::Child,
    tokio::process::ChildStdin,
    tokio::process::ChildStdout,
) {
    spawn_helper_with_quality_and_policy_and_clipboard(port, true, 3, quality, true).await
}

async fn spawn_helper_with_quality_and_policy_and_clipboard(
    port: u16,
    reconnect_enabled: bool,
    reconnect_attempts: u8,
    quality: &str,
    clipboard_enabled: bool,
) -> (
    tokio::process::Child,
    tokio::process::ChildStdin,
    tokio::process::ChildStdout,
) {
    let reconnect_flag = if reconnect_enabled {
        "--reconnect-enabled"
    } else {
        "--reconnect-disabled"
    };
    let reconnect_attempts = reconnect_attempts.to_string();
    let port = port.to_string();
    let mut helper_arguments = vec![
        "--mobarust-protocol".to_owned(),
        "vnc".to_owned(),
        "--host".to_owned(),
        "127.0.0.1".to_owned(),
        "--port".to_owned(),
        port,
        "--width".to_owned(),
        "320".to_owned(),
        "--height".to_owned(),
        "200".to_owned(),
        "--quality".to_owned(),
        quality.to_owned(),
    ];
    if clipboard_enabled {
        helper_arguments.push("--clipboard-enabled".to_owned());
    }
    helper_arguments.extend([
        reconnect_flag.to_owned(),
        "--reconnect-attempts".to_owned(),
        reconnect_attempts,
    ]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_mobarust-vnc-helper"))
        .args(helper_arguments)
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

async fn run_fixture(
    listener: TcpListener,
    auth: FixtureAuth,
    expected_encodings: Vec<i32>,
) -> Result<(), String> {
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
    let actual_encodings = read_set_encodings(&mut stream).await?;
    if actual_encodings != expected_encodings {
        return Err(format!(
            "helper sent unexpected VNC encodings: {actual_encodings:?}"
        ));
    }
    read_update_request(&mut stream).await?;

    let mut update = vec![0_u8, 0, 0, 1];
    update.extend_from_slice(&[0, 0, 0, 0, 0, 1, 0, 1]);
    update.extend_from_slice(&[0, 0, 0, 0]);
    update.extend_from_slice(&[0x11, 0x22, 0x33, 0xff]);
    stream
        .write_all(&update)
        .await
        .map_err(|error| error.to_string())?;

    let remote_clipboard = b"remote fixture clipboard";
    let mut cut_text = vec![3_u8, 0, 0, 0];
    cut_text.extend_from_slice(&(remote_clipboard.len() as u32).to_be_bytes());
    cut_text.extend_from_slice(remote_clipboard);
    stream
        .write_all(&cut_text)
        .await
        .map_err(|error| error.to_string())?;

    let mut resize_update = vec![0_u8, 0, 0, 1];
    resize_update.extend_from_slice(&[0, 0, 0, 0]);
    resize_update.extend_from_slice(&RESIZED_FIXTURE_SIZE.width.to_be_bytes());
    resize_update.extend_from_slice(&RESIZED_FIXTURE_SIZE.height.to_be_bytes());
    resize_update.extend_from_slice(&(-223_i32).to_be_bytes());
    stream
        .write_all(&resize_update)
        .await
        .map_err(|error| error.to_string())?;

    let mut resized_update = vec![0_u8, 0, 0, 1];
    resized_update.extend_from_slice(&[0, 0, 0, 0, 0, 1, 0, 1]);
    resized_update.extend_from_slice(&[0, 0, 0, 0]);
    resized_update.extend_from_slice(&[0x44, 0x55, 0x66, 0xff]);
    stream
        .write_all(&resized_update)
        .await
        .map_err(|error| error.to_string())?;

    let mut saw_key = false;
    let mut saw_pointer = false;
    let mut saw_wheel = false;
    let mut saw_clipboard = false;
    while !(saw_key && saw_pointer && saw_wheel && saw_clipboard) {
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
                let keysym = u32::from_be_bytes(payload[3..7].try_into().unwrap());
                if keysym != FIXTURE_KEYSYM {
                    return Err(format!(
                        "fixture received unexpected VNC keysym: 0x{keysym:08x}"
                    ));
                }
                saw_key = true;
            }
            5 => {
                let mut payload = [0_u8; 5];
                stream
                    .read_exact(&mut payload)
                    .await
                    .map_err(|error| error.to_string())?;
                let x = u16::from_be_bytes([payload[1], payload[2]]);
                let y = u16::from_be_bytes([payload[3], payload[4]]);
                if (x, y)
                    != (
                        RESIZED_FIXTURE_SIZE.width - 1,
                        RESIZED_FIXTURE_SIZE.height - 1,
                    )
                {
                    return Err(format!(
                        "fixture received out-of-bounds pointer coordinates: ({x},{y})"
                    ));
                }
                if payload[0] == 0b0000_0001 {
                    saw_pointer = true;
                }
                if payload[0] == 0b0000_1000 || payload[0] == 0b0001_0000 {
                    saw_wheel = true;
                }
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
                if text != b"fixture clipboard" {
                    return Err("fixture received unexpected clipboard text".into());
                }
                saw_clipboard = true;
            }
            _ => return Err("fixture saw an unexpected client message".into()),
        }
    }
    Ok(())
}

async fn serve_framebuffer_connection(
    stream: &mut TcpStream,
    pixel: [u8; 4],
) -> Result<(), String> {
    prepare_vnc_connection(stream).await?;

    let mut update = vec![0_u8, 0, 0, 1];
    update.extend_from_slice(&[0, 0, 0, 0, 0, 1, 0, 1]);
    update.extend_from_slice(&[0, 0, 0, 0]);
    update.extend_from_slice(&pixel);
    stream
        .write_all(&update)
        .await
        .map_err(|error| error.to_string())
}

async fn serve_tight_framebuffer_connection(stream: &mut TcpStream) -> Result<(), String> {
    prepare_vnc_connection(stream).await?;
    let jpeg = base64::engine::general_purpose::STANDARD
        .decode("/9j/4AAQSkZJRgABAgAAAQABAAD//gAPTGF2YzYzLjEuMTAxAP/bAEMACAQEBAQEBQUFBQUFBgYGBgYGBgYGBgYGBgcHBwgICAcHBwYGBwcICAgICQkJCAgICAkJCgoKDAwLCw4ODhERFP/EAE0AAQEAAAAAAAAAAAAAAAAAAAAGAQEBAQAAAAAAAAAAAAAAAAAABgcQAQAAAAAAAAAAAAAAAAAAAAARAQAAAAAAAAAAAAAAAAAAAAD/wAARCAACAAIDARIAAhIAAxIA/9oADAMBAAIRAxEAPwCLEmN/H//Z")
        .map_err(|error| error.to_string())?;
    let mut update = vec![0_u8, 0, 0, 1];
    update.extend_from_slice(&[0, 0, 0, 0, 0, 2, 0, 2]);
    update.extend_from_slice(&7_i32.to_be_bytes());
    update.push(0x90);
    let mut length = jpeg.len();
    loop {
        let mut byte = u8::try_from(length & 0x7f).map_err(|_| "fixture JPEG length overflow")?;
        length >>= 7;
        if length != 0 {
            byte |= 0x80;
        }
        update.push(byte);
        if length == 0 {
            break;
        }
    }
    update.extend_from_slice(&jpeg);
    stream
        .write_all(&update)
        .await
        .map_err(|error| error.to_string())
}

async fn prepare_vnc_connection(stream: &mut TcpStream) -> Result<(), String> {
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
    read_set_pixel_format(stream).await?;
    read_set_encodings(stream).await?;
    read_update_request(stream).await?;
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

async fn read_set_encodings(stream: &mut TcpStream) -> Result<Vec<i32>, String> {
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
    Ok(encodings
        .chunks_exact(4)
        .map(|encoding| i32::from_be_bytes(encoding.try_into().unwrap()))
        .collect())
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

async fn read_client_message_kind(stream: &mut TcpStream) -> Result<u8, String> {
    let mut kind = [0_u8; 1];
    stream
        .read_exact(&mut kind)
        .await
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
        }
        5 => {
            let mut payload = [0_u8; 5];
            stream
                .read_exact(&mut payload)
                .await
                .map_err(|error| error.to_string())?;
        }
        6 => {}
        value => return Err(format!("fixture saw an unexpected client message: {value}")),
    }
    Ok(kind[0])
}
