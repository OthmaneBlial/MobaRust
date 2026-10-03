//! Production desktop I/O pump against encrypted, memory-only loopback SSH.
use super::{ShellRunResult, open_shell_at_current_size, retire_shell_output, run_shell_operation};
use mobarust_ssh::{HostKeyPolicy, SshConnectOptions, SshConnection, SshCredentials};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::keys::{HashAlg, PrivateKey};
use russh::server::{self, Auth};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{Notify, mpsc, oneshot, watch};
use uuid::Uuid;
use zeroize::Zeroizing;

struct Peer {
    password: Zeroizing<String>,
    received: Arc<AtomicUsize>,
    received_bytes: Arc<Mutex<Vec<u8>>>,
    delivered: Arc<Notify>,
    pause_first: Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>,
    ready: Option<oneshot::Sender<(server::Handle, russh::ChannelId)>>,
    geometry: Option<mpsc::Sender<(&'static str, u32, u32)>>,
    pause_replacement_pty: Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>,
    pause_operation: Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>,
    operation_channels:
        Option<std::collections::HashMap<russh::ChannelId, russh::Channel<server::Msg>>>,
    forward_to: Option<std::net::SocketAddr>,
    forwarded: tokio::task::JoinSet<()>,
}

struct DeniedSftp(Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>);

impl russh_sftp::server::Handler for DeniedSftp {
    type Error = russh_sftp::protocol::StatusCode;

    fn unimplemented(&self) -> Self::Error {
        russh_sftp::protocol::StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: std::collections::HashMap<String, String>,
    ) -> Result<russh_sftp::protocol::Version, Self::Error> {
        if let Some((entered, release)) = self.0.take() {
            entered.send(()).unwrap();
            release.await.unwrap();
        }
        Ok(russh_sftp::protocol::Version::new())
    }
}

impl server::Handler for Peer {
    type Error = russh::Error;

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: russh::Channel<server::Msg>,
        host: &str,
        port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: server::ChannelOpenHandle,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if let Some((entered, release)) = self.pause_operation.take() {
            entered.send(()).unwrap();
            release.await.unwrap();
        }
        while self.forwarded.try_join_next().is_some() {}
        if self.forwarded.len() >= 8 {
            return Ok(());
        }
        let Some(target) = self.forward_to else {
            return Ok(());
        };
        // This fixture can reach only its own preselected loopback endpoint.
        // Never resolve or connect to a caller-selected address.
        if host != "127.0.0.1" || !target.ip().is_loopback() || port != u32::from(target.port()) {
            return Ok(());
        }
        let Ok(Ok(mut upstream)) = tokio::time::timeout(
            Duration::from_secs(2),
            tokio::net::TcpStream::connect(target),
        )
        .await
        else {
            return Ok(());
        };
        reply.accept().await;
        self.forwarded.spawn(async move {
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
        });
        Ok(())
    }

    async fn tcpip_forward(
        &mut self,
        _address: &str,
        _port: &mut u32,
        _session: &mut server::Session,
    ) -> Result<bool, Self::Error> {
        if let Some((entered, release)) = self.pause_operation.take() {
            entered.send(()).unwrap();
            release.await.unwrap();
        }
        // This fixture never opens a remote listener. Pause/reject the request
        // to exercise cancellation while server acceptance is unresolved.
        Ok(false)
    }

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if user == "fixture" && password == self.password.as_str() {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        channel: russh::Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if let Some(channels) = &mut self.operation_channels {
            channels.insert(channel.id(), channel);
        }
        reply.accept().await;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: russh::ChannelId,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        if let Some(ready) = self.ready.take() {
            let _ = ready.send((session.handle(), channel));
        }
        Ok(())
    }

    async fn pty_request(
        &mut self,
        _channel: russh::ChannelId,
        _term: &str,
        cols: u32,
        rows: u32,
        _pixel_width: u32,
        _pixel_height: u32,
        _modes: &[(russh::Pty, u32)],
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if let Some(geometry) = &self.geometry {
            geometry.try_send(("pty", cols, rows)).unwrap();
        }
        if self.ready.is_none()
            && let Some((entered, release)) = self.pause_replacement_pty.take()
        {
            let _ = entered.send(());
            release.await.unwrap();
        }
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        _channel: russh::ChannelId,
        cols: u32,
        rows: u32,
        _pixel_width: u32,
        _pixel_height: u32,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if let Some(geometry) = &self.geometry {
            geometry.try_send(("resize", cols, rows)).unwrap();
        }
        Ok(())
    }

    async fn data(
        &mut self,
        _channel: russh::ChannelId,
        data: &[u8],
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.received.fetch_add(data.len(), Ordering::SeqCst);
        self.received_bytes.lock().unwrap().extend_from_slice(data);
        self.delivered.notify_one();
        if let Some((entered, release)) = self.pause_first.take() {
            let _ = entered.send(());
            release.await.unwrap();
        }
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel: russh::ChannelId,
        name: &str,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        assert_eq!(name, "sftp");
        session.channel_success(channel)?;
        russh_sftp::server::run(
            self.operation_channels
                .as_mut()
                .unwrap()
                .remove(&channel)
                .unwrap()
                .into_stream(),
            DeniedSftp(self.pause_operation.take()),
        )
        .await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: russh::ChannelId,
        _command: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if let Some((entered, release)) = self.pause_operation.take() {
            entered.send(()).unwrap();
            release.await.unwrap();
        }
        session.eof(channel)?;
        session.exit_status_request(channel, 1)?;
        session.close(channel)?;
        Ok(())
    }
}

#[tokio::test]
async fn blocked_shell_input_keeps_output_and_cancellation_live() {
    tokio::time::timeout(Duration::from_secs(10), async {
        for action in ["close", "exit", "disconnect", "resume", "flood-close"] {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let address = listener.local_addr().unwrap();
            assert!(address.ip().is_loopback());
            let mut seed = Zeroizing::new([0; 32]);
            seed[..16].copy_from_slice(Uuid::new_v4().as_bytes());
            seed[16..].copy_from_slice(Uuid::new_v4().as_bytes());
            let key = PrivateKey::new(KeypairData::Ed25519(Ed25519Keypair::from_seed(&seed)), "").unwrap();
            let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
            let password = Zeroizing::new(Uuid::new_v4().to_string());
            let received = Arc::new(AtomicUsize::new(0));
            let received_bytes = Arc::new(Mutex::new(Vec::new()));
            let delivered = Arc::new(Notify::new());
            let (first, first_received) = oneshot::channel();
            let (release, resumed) = oneshot::channel();
            let (ready, channel) = oneshot::channel();
            let (geometry, mut sizes) = mpsc::channel(8);
            let (pty_entered, pty_pending) = oneshot::channel();
            let (pty_release, pty_resumed) = oneshot::channel();
            let peer = Peer { password: password.clone(), received: received.clone(),
                received_bytes: received_bytes.clone(), delivered: delivered.clone(),
                pause_first: (action == "resume").then_some((first, resumed)), ready: Some(ready),
                geometry: (action == "resume").then_some(geometry),
                pause_replacement_pty: (action == "resume").then_some((pty_entered, pty_resumed)),
                pause_operation: None, operation_channels: None,
                forward_to: None, forwarded: tokio::task::JoinSet::new() };
            let config = Arc::new(server::Config {
                keys: vec![key], window_size: if action == "resume" { 1024 } else { 0 }, maximum_packet_size: 1024,
                auth_rejection_time: Duration::ZERO, auth_rejection_time_initial: Some(Duration::ZERO),
                inactivity_timeout: None, nodelay: true, ..Default::default()
            });
            let server = tokio::spawn(async move {
                let (stream, remote) = listener.accept().await.unwrap();
                assert!(remote.ip().is_loopback());
                drop(listener);
                server::run_stream(config, stream, peer).await.unwrap().await
            });
            let connection = SshConnection::connect(SshConnectOptions {
                host: "127.0.0.1".into(), port: address.port(),
                host_key_policy: HostKeyPolicy::PinnedFingerprint(fingerprint), timeout: Duration::from_secs(5),
                credentials: SshCredentials::password_secret("fixture", mobarust_ssh::Secret::from_zeroizing(password)),
                keepalive_interval: None, x11: None, environment: Vec::new(), startup_directory: None, startup_command: None,
            }).await.unwrap();
            let (size, mut viewport) = watch::channel((80, 24));
            let (mut reader, writer) = open_shell_at_current_size(&connection, &viewport).await.unwrap().split();
            let (remote, channel) = channel.await.unwrap();
            if action == "resume" { assert_eq!(sizes.recv().await.unwrap(), ("pty", 80, 24)); }
            let (close, mut closing) = watch::channel(false);
            let entered = Arc::new(Notify::new());
            let output_ready = Arc::new(Notify::new());
            let output = Arc::new(Mutex::new(Vec::new()));
            let stdout = "output while input blocked: été 🦀\r\n".as_bytes();
            let stderr = b"stderr while input blocked\r\n";
            let expected_len = stdout.len() + stderr.len();
            let started = entered.clone();
            let starts = Arc::new(AtomicUsize::new(0));
            let operation_starts = starts.clone();
            let input = if action == "resume" {
                (0..65536).map(|index| (index % 251) as u8).collect::<Vec<_>>()
            } else { b"must-not-be-replayed".to_vec() };
            let sent = input.clone();
            let observed = output.clone();
            let notified = output_ready.clone();
            let task = tokio::spawn(async move {
                let operation = async {
                    operation_starts.fetch_add(1, Ordering::SeqCst);
                    started.notify_one();
                    writer.write(&sent).await
                };
                let result = run_shell_operation(operation, &mut reader, &mut closing, |bytes| {
                    let mut output = observed.lock().unwrap();
                    output.extend_from_slice(bytes);
                    if output.len() == expected_len { notified.notify_one(); }
                }).await;
                (result, reader, writer)
            });
            entered.notified().await;
            if action == "resume" {
                first_received.await.unwrap();
                assert_eq!(*received_bytes.lock().unwrap(), input[..1024]);
            }
            assert!(!task.is_finished(), "receive-window backpressure must stall the actual write");
            remote.data(channel, stdout.to_vec()).await.unwrap();
            remote.extended_data(channel, 1, stderr.to_vec()).await.unwrap();
            if action == "resume" { release.send(()).unwrap(); }
            tokio::time::timeout(Duration::from_secs(1), output_ready.notified()).await
                .expect("desktop must deliver stdout/stderr while terminal input waits for window credit");
            assert_eq!(*output.lock().unwrap(), [stdout, stderr].concat());
            if action != "resume" {
                assert!(!task.is_finished(), "output must not release the peer's zero input window");
                assert_eq!(received.load(Ordering::SeqCst), 0);
            }
            match action {
                "close" | "flood-close" => { close.send_replace(true); }
                "exit" => {
                    remote.eof(channel).await.unwrap();
                    remote.exit_status_request(channel, 0).await.unwrap();
                    remote.close(channel).await.unwrap();
                }
                "disconnect" => { remote.disconnect(russh::Disconnect::ByApplication, String::new(), "en".into()).await.unwrap(); }
                _ => {}
            }
            let (result, reader, writer) = tokio::time::timeout(Duration::from_secs(1), task).await
                .expect("close/exit/transport loss must retire a blocked terminal write").unwrap();
            assert!(matches!((&result, action), (Err(ShellRunResult::Closed), "close" | "exit" | "flood-close") | (Err(ShellRunResult::Lost(_)), "disconnect") | (Ok(()), "resume")));
            assert_eq!(starts.load(Ordering::SeqCst), 1, "output never restarts a partial write");
            if action == "resume" {
                tokio::time::timeout(Duration::from_secs(1), async {
                    while received.load(Ordering::SeqCst) < input.len() { delivered.notified().await; }
                }).await.expect("peer receives the completed write");
                assert_eq!(*received_bytes.lock().unwrap(), input, "input is delivered exactly once");
            }
            if action == "flood-close" {
                // More packets than the reader's bounded queue. Keep the same transport alive.
                for _ in 0..256 { remote.data(channel, vec![b'x'; 1024]).await.unwrap(); }
                let writer = Arc::new(writer);
                let fill_writer = writer.clone();
                let accepted = Arc::new(AtomicUsize::new(0));
                let sent = accepted.clone();
                let mut fill = tokio::spawn(async move {
                    for _ in 0..64 {
                        fill_writer.resize(80, 24).await.unwrap();
                        sent.fetch_add(1, Ordering::SeqCst);
                    }
                });
                assert!(tokio::time::timeout(Duration::from_millis(100), &mut fill).await.is_err(),
                    "unread shell output must backpressure the transport actor");
                assert!(accepted.load(Ordering::SeqCst) >= 10, "fill the outbound actor queue too");
                let stalled = accepted.load(Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                assert_eq!(accepted.load(Ordering::SeqCst), stalled, "actor remains blocked by unread output");
                fill.abort();
                assert!(fill.await.unwrap_err().is_cancelled());
                tokio::time::timeout(Duration::from_secs(1), retire_shell_output(reader, &writer)).await
                    .expect("retired output must not deadlock EOF behind the full actor queue");
                let shell = tokio::time::timeout(Duration::from_secs(1), connection.open_shell(80, 24)).await
                    .expect("the same transport must still process other channels").unwrap();
                let (reader, next_writer) = shell.split();
                retire_shell_output(reader, &next_writer).await;
                drop(next_writer); drop(writer);
            } else if action == "resume" {
                retire_shell_output(reader, &writer).await;
                drop(writer);
                size.send((132, 41)).unwrap();
                let (mut reader, writer) = {
                    let reopening = open_shell_at_current_size(&connection, &viewport);
                    tokio::pin!(reopening);
                    tokio::select! {
                        _ = &mut reopening => panic!("replacement shell needs server acceptance"),
                        entered = pty_pending => entered.unwrap(),
                    }
                    assert_eq!(sizes.recv().await.unwrap(), ("pty", 132, 41),
                        "replacement PTY uses current geometry rather than connect defaults");
                    size.send((155, 53)).unwrap();
                    pty_release.send(()).unwrap();
                    reopening.await.unwrap().split()
                };
                viewport.changed().await.unwrap();
                let (cols, rows) = *viewport.borrow_and_update();
                let mut closing = close.subscribe();
                assert!(run_shell_operation(writer.resize(cols, rows), &mut reader, &mut closing,
                    |_| panic!("retired output must not reach the replacement shell")).await.is_ok());
                assert_eq!(sizes.recv().await.unwrap(), ("resize", 155, 53),
                    "a resize arriving during shell setup is still delivered");
                retire_shell_output(reader, &writer).await;
                drop(writer);
            } else { drop(reader); drop(writer); }
            let _ = connection.disconnect().await;
            drop(connection);
            let result = server.await.unwrap();
            if let Err(error) = result {
                assert!(matches!(error, russh::Error::Disconnect)
                    || matches!(&error, russh::Error::IO(error) if matches!(error.kind(),
                        std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::BrokenPipe)),
                    "unexpected {action} fixture protocol error: {error}");
            }
            assert_eq!(received.load(Ordering::SeqCst), if action == "resume" { input.len() } else { 0 }, "no replay or delivery of discarded input");
            drop(TcpListener::bind(address).await.expect("owned listener released"));
        }
    }).await.expect("loopback backpressure fixture cleanup deadline");
}

#[tokio::test]
async fn tunnel_workers_are_owned_and_joined_before_transport_cleanup() {
    use super::{
        DynamicForwardJob, LocalForwardJob, RemoteForwardJob, SshCommand, SshManager,
        TunnelControl, TunnelState, spawn_session_tunnel,
    };
    use tokio::task::JoinSet;

    tokio::time::timeout(Duration::from_secs(10), async {
        for kind in ["local", "socks", "remote", "local-traffic", "socks-traffic", "socks-handshake", "local-open-pending"] {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let traffic = kind.ends_with("-traffic");
            let target = if traffic { Some(TcpListener::bind(("127.0.0.1", 0)).await.unwrap()) } else { None };
            let target_address = target.as_ref().map(|listener| listener.local_addr().unwrap());
            let payload = (0..32768).map(|index| (index % 251) as u8).collect::<Vec<_>>();
            let expected = payload.clone();
            let target_task = target.map(|listener| tokio::spawn(async move {
                let (mut stream, peer) = listener.accept().await.unwrap();
                assert!(peer.ip().is_loopback());
                let mut received = vec![0; expected.len()];
                stream.read_exact(&mut received).await.unwrap();
                assert_eq!(received, expected);
                stream.write_all(&received).await.unwrap();
                assert_eq!(stream.read(&mut [0]).await.unwrap(), 0,
                    "tunnel cancellation must close the upstream TCP socket before transport cleanup");
                listener
            }));
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let address = listener.local_addr().unwrap();
            let mut seed = Zeroizing::new([0; 32]);
            seed[..16].copy_from_slice(Uuid::new_v4().as_bytes());
            seed[16..].copy_from_slice(Uuid::new_v4().as_bytes());
            let key = PrivateKey::new(KeypairData::Ed25519(Ed25519Keypair::from_seed(&seed)), "")
                .unwrap();
            let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
            let password = Zeroizing::new(Uuid::new_v4().to_string());
            let (entered, operation_entered) = oneshot::channel();
            let (release, operation_release) = oneshot::channel();
            let peer = Peer {
                password: password.clone(),
                received: Arc::new(AtomicUsize::new(0)),
                received_bytes: Arc::new(Mutex::new(Vec::new())),
                delivered: Arc::new(Notify::new()),
                pause_first: None,
                ready: None,
                geometry: None,
                pause_replacement_pty: None,
                pause_operation: (kind == "remote" || kind == "local-open-pending").then_some((entered, operation_release)),
                operation_channels: None,
                forward_to: target_address,
                forwarded: JoinSet::new(),
            };
            let config = Arc::new(server::Config {
                keys: vec![key],
                auth_rejection_time: Duration::ZERO,
                auth_rejection_time_initial: Some(Duration::ZERO),
                inactivity_timeout: None,
                ..Default::default()
            });
            let server = tokio::spawn(async move {
                let (stream, remote) = listener.accept().await.unwrap();
                assert!(remote.ip().is_loopback());
                drop(listener);
                server::run_stream(config, stream, peer)
                    .await
                    .unwrap()
                    .await
            });
            let connection = Arc::new(
                SshConnection::connect(SshConnectOptions {
                    host: "127.0.0.1".into(),
                    port: address.port(),
                    host_key_policy: HostKeyPolicy::PinnedFingerprint(fingerprint),
                    timeout: Duration::from_secs(5),
                    credentials: SshCredentials::password_secret(
                        "fixture",
                        mobarust_ssh::Secret::from_zeroizing(password),
                    ),
                    keepalive_interval: None,
                    x11: None,
                    environment: Vec::new(),
                    startup_directory: None,
                    startup_command: None,
                })
                .await
                .unwrap(),
            );
            let manager = SshManager::default();
            let (cancel, cancellation) = watch::channel(false);
            manager.tunnels.lock().unwrap().insert(
                kind.into(),
                TunnelControl {
                    terminal_id: "fixture".into(),
                    cancel,
                },
            );
            let (reply, response) = oneshot::channel();
            let mut port = None;
            let command = if kind == "remote" {
                manager
                    .remote_forwards
                    .lock()
                    .unwrap()
                    .insert("fixture".into(), kind.into());
                SshCommand::StartRemoteForward {
                    job: RemoteForwardJob {
                        tunnel_id: kind.into(),
                        terminal_id: "fixture".into(),
                        bind_host: "127.0.0.1".into(),
                        bind_port: 1,
                        target_host: "127.0.0.1".into(),
                        target_port: 1,
                        cancel: cancellation,
                    },
                    reply,
                }
            } else {
                let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
                let bind_port = listener.local_addr().unwrap().port();
                port = Some(bind_port);
                if kind.starts_with("local") {
                    SshCommand::StartLocalForward {
                        job: LocalForwardJob {
                            tunnel_id: kind.into(),
                            terminal_id: "fixture".into(),
                            bind_host: "127.0.0.1".into(),
                            bind_port,
                            target_host: "127.0.0.1".into(),
                            target_port: target_address.map_or(1, |target| target.port()),
                            listener,
                            cancel: cancellation,
                        },
                    }
                } else {
                    SshCommand::StartDynamicForward {
                        job: DynamicForwardJob {
                            tunnel_id: kind.into(),
                            terminal_id: "fixture".into(),
                            bind_host: "127.0.0.1".into(),
                            bind_port,
                            listener,
                            cancel: cancellation,
                        },
                    }
                }
            };
            let mut workers = JoinSet::new();
            let command = manager
                .admit_session_command(
                    command,
                    &mut workers,
                    |_| panic!("tunnels do not emit transfer events"),
                    |_| panic!("idle session must admit tunnel"),
                )
                .unwrap();
            let (started, ready) = oneshot::channel();
            let events = Arc::new(Mutex::new(Vec::new()));
            let observed = events.clone();
            let mut started = Some(started);
            let (accepted, accepted_connection) = oneshot::channel();
            let mut accepted = Some(accepted);
            assert!(
                spawn_session_tunnel(
                    command,
                    manager.clone(),
                    connection.clone(),
                    &mut workers,
                    move |event| {
                        if matches!(event.state, TunnelState::Running)
                            && let Some(started) = started.take()
                        {
                            let _ = started.send(());
                        }
                        if event.connections > 0 && let Some(accepted) = accepted.take() {
                            let _ = accepted.send(());
                        }
                        observed.lock().unwrap().push(event);
                    }
                )
                .is_none()
            );
            assert_eq!(
                workers.len(),
                1,
                "{kind} tunnel must belong to the session worker set"
            );
            if kind != "remote" {
                ready.await.unwrap();
            }
            let mut client = if traffic || kind == "socks-handshake" || kind == "local-open-pending" {
                Some(tokio::net::TcpStream::connect(("127.0.0.1", port.unwrap())).await.unwrap())
            } else { None };
            if kind == "remote" || kind == "local-open-pending" {
                operation_entered.await.unwrap();
            }
            if let Some(client) = &mut client {
                accepted_connection.await.unwrap();
                if traffic {
                    if kind.starts_with("socks") {
                        client.write_all(&[5, 1, 0]).await.unwrap();
                        let mut greeting = [0; 2];
                        client.read_exact(&mut greeting).await.unwrap();
                        assert_eq!(greeting, [5, 0]);
                        let target = target_address.unwrap().port().to_be_bytes();
                        client.write_all(&[5, 1, 0, 1, 127, 0, 0, 1, target[0], target[1]]).await.unwrap();
                        let mut reply = [0; 10];
                        client.read_exact(&mut reply).await.unwrap();
                        assert_eq!(&reply[..2], &[5, 0]);
                    }
                    client.write_all(&payload).await.unwrap();
                    let mut echoed = vec![0; payload.len()];
                    client.read_exact(&mut echoed).await.unwrap();
                    assert_eq!(echoed, payload, "encrypted forwarding preserves both directions' bytes");
                    assert!(!target_task.as_ref().unwrap().is_finished());
                }
            }
            assert!(
                !server.is_finished(),
                "transport is still owned while draining its tunnel"
            );
            manager
                .finish_session_operations("fixture", &mut workers)
                .await;
            assert!(workers.is_empty());
            assert!(manager.tunnels.lock().unwrap().is_empty());
            assert!(manager.remote_forwards.lock().unwrap().is_empty());
            if let Some(client) = &mut client {
                let mut closed = Vec::new();
                tokio::time::timeout(Duration::from_secs(2), client.take(11).read_to_end(&mut closed)).await
                    .unwrap_or_else(|_| panic!("joined {kind} tunnel must close its client before transport cleanup or peer release"))
                    .unwrap();
                if traffic { assert!(closed.is_empty(), "no extra payload is forwarded after cleanup"); }
                else if kind == "socks-handshake" { assert!(closed.is_empty() || closed == [5, 1, 0, 1, 0, 0, 0, 0, 0, 0],
                    "a stopped SOCKS handshake may report failure before EOF"); }
                else { assert!(closed.is_empty(), "pending direct-channel open must drop its local client on cancellation"); }
            }
            drop(client);
            if let Some(target) = target_task {
                // Retain this port until the connection task has actually
                // completed; parallel ephemeral fixtures may otherwise take it.
                drop(target.await.unwrap());
                drop(TcpListener::bind(target_address.unwrap()).await.expect("owned TCP target listener released"));
            }
            if kind == "remote" {
                assert!(
                    response
                        .await
                        .unwrap()
                        .unwrap_err()
                        .contains("cancelled before")
                );
                release.send(()).unwrap();
            } else {
                if kind == "local-open-pending" { release.send(()).unwrap(); }
                assert!(matches!(
                    events.lock().unwrap().last().unwrap().state,
                    TunnelState::Stopped
                ));
                drop(
                    TcpListener::bind(("127.0.0.1", port.unwrap()))
                        .await
                        .expect("joined tunnel must release its listener before transport cleanup"),
                );
            }
            if kind != "remote" {
                assert!(!server.is_finished());
            } else {
                assert_eq!(connection.state(), mobarust_core::ConnectionState::Disconnected);
            }
            connection.disconnect().await.unwrap();
            drop(connection);
            let result = server.await.unwrap();
            if let Err(error) = result {
                assert!(
                    matches!(error, russh::Error::Disconnect)
                        || matches!(&error, russh::Error::IO(error) if matches!(error.kind(),
                        std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::BrokenPipe)),
                    "unexpected tunnel fixture protocol error: {error}"
                );
            }
            drop(
                TcpListener::bind(address)
                    .await
                    .expect("owned SSH listener released"),
            );
        }
    })
    .await
    .expect("owned loopback tunnel cleanup deadline");
}

#[tokio::test]
async fn finite_session_operations_settle_before_transport_cleanup() {
    use super::{SshCommand, SshFileOperation, SshManager, spawn_session_operation};
    use mobarust_ssh::RemoteTextEncoding;
    use std::future::{Future, poll_fn};
    use std::task::Poll;
    use tokio::task::{JoinHandle, JoinSet};

    fn rejected<T: Send + 'static>(
        response: oneshot::Receiver<Result<T, String>>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            assert!(response.await.unwrap().is_err());
        })
    }

    tokio::time::timeout(Duration::from_secs(10), async {
        for action in [
            "save",
            "save-as",
            "file-operation",
            "list",
            "open",
            "monitor",
        ] {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let address = listener.local_addr().unwrap();
            let mut seed = Zeroizing::new([0; 32]);
            seed[..16].copy_from_slice(Uuid::new_v4().as_bytes());
            seed[16..].copy_from_slice(Uuid::new_v4().as_bytes());
            let key = PrivateKey::new(KeypairData::Ed25519(Ed25519Keypair::from_seed(&seed)), "")
                .unwrap();
            let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
            let password = Zeroizing::new(Uuid::new_v4().to_string());
            let (entered, operation_entered) = oneshot::channel();
            let (release, operation_release) = oneshot::channel();
            let peer = Peer {
                password: password.clone(),
                received: Arc::new(AtomicUsize::new(0)),
                received_bytes: Arc::new(Mutex::new(Vec::new())),
                delivered: Arc::new(Notify::new()),
                pause_first: None,
                ready: None,
                geometry: None,
                pause_replacement_pty: None,
                pause_operation: Some((entered, operation_release)),
                operation_channels: Some(std::collections::HashMap::new()),
                forward_to: None,
                forwarded: JoinSet::new(),
            };
            let config = Arc::new(server::Config {
                keys: vec![key],
                auth_rejection_time: Duration::ZERO,
                auth_rejection_time_initial: Some(Duration::ZERO),
                inactivity_timeout: None,
                ..Default::default()
            });
            let server = tokio::spawn(async move {
                let (stream, remote) = listener.accept().await.unwrap();
                assert!(remote.ip().is_loopback());
                drop(listener);
                server::run_stream(config, stream, peer)
                    .await
                    .unwrap()
                    .await
            });
            let connection = Arc::new(
                SshConnection::connect(SshConnectOptions {
                    host: "127.0.0.1".into(),
                    port: address.port(),
                    host_key_policy: HostKeyPolicy::PinnedFingerprint(fingerprint),
                    timeout: Duration::from_secs(5),
                    credentials: SshCredentials::password_secret(
                        "fixture",
                        mobarust_ssh::Secret::from_zeroizing(password),
                    ),
                    keepalive_interval: None,
                    x11: None,
                    environment: Vec::new(),
                    startup_directory: None,
                    startup_command: None,
                })
                .await
                .unwrap(),
            );
            let (command, response) = match action {
                "save" => {
                    let (reply, response) = oneshot::channel();
                    (
                        SshCommand::SaveTextFile {
                            path: "/fixture.txt".into(),
                            expected_revision: "fixture-revision".into(),
                            content: "replacement".into(),
                            encoding: RemoteTextEncoding::Utf8,
                            reply,
                        },
                        rejected(response),
                    )
                }
                "save-as" => {
                    let (reply, response) = oneshot::channel();
                    (
                        SshCommand::SaveTextFileAs {
                            path: "/fixture-copy.txt".into(),
                            content: "replacement".into(),
                            encoding: RemoteTextEncoding::Utf8,
                            overwrite: true,
                            reply,
                        },
                        rejected(response),
                    )
                }
                "file-operation" => {
                    let (reply, response) = oneshot::channel();
                    (
                        SshCommand::FileOperation {
                            operation: SshFileOperation::Rename {
                                from: "/before".into(),
                                to: "/after".into(),
                            },
                            reply,
                        },
                        rejected(response),
                    )
                }
                "list" => {
                    let (reply, response) = oneshot::channel();
                    (
                        SshCommand::ListDirectory {
                            path: "/".into(),
                            reply,
                        },
                        rejected(response),
                    )
                }
                "open" => {
                    let (reply, response) = oneshot::channel();
                    (
                        SshCommand::OpenTextFile {
                            path: "/fixture.txt".into(),
                            encoding: RemoteTextEncoding::Utf8,
                            reply,
                        },
                        rejected(response),
                    )
                }
                "monitor" => {
                    let (reply, response) = oneshot::channel();
                    (SshCommand::CollectMonitor { reply }, rejected(response))
                }
                _ => unreachable!(),
            };
            let manager = SshManager::default();
            let mut workers = JoinSet::new();
            let command = manager
                .admit_session_command(
                    command,
                    &mut workers,
                    |_| panic!("finite action must not emit a transfer event"),
                    |_| panic!("finite action must not emit a tunnel event"),
                )
                .expect("an idle session must admit the operation");
            assert!(spawn_session_operation(command, connection.clone(), &mut workers).is_none());
            operation_entered.await.unwrap();
            assert!(!response.is_finished());
            let mut drain = Box::pin(manager.finish_session_operations("fixture", &mut workers));
            poll_fn(|cx| {
                assert!(
                    drain.as_mut().poll(cx).is_pending(),
                    "{action} was detached from session cleanup"
                );
                Poll::Ready(())
            })
            .await;
            release.send(()).unwrap();
            drain.await;
            assert!(workers.is_empty());
            response.await.unwrap();
            connection.disconnect().await.unwrap();
            drop(connection);
            let result = server.await.unwrap();
            assert!(
                result.is_ok()
                    || matches!(result, Err(russh::Error::Disconnect))
                    || matches!(&result, Err(russh::Error::IO(error))
                        if matches!(error.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof)),
                "unexpected SSH fixture error: {result:?}"
            );
            drop(
                TcpListener::bind(address)
                    .await
                    .expect("owned listener released"),
            );
        }
    })
    .await
    .expect("finite SSH operations and cleanup deadline");
}
