//! Production desktop I/O pump against encrypted, memory-only loopback SSH.
use super::{ShellRunResult, run_shell_operation};
use mobarust_ssh::{HostKeyPolicy, SshConnectOptions, SshConnection, SshCredentials};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::keys::{HashAlg, PrivateKey};
use russh::server::{self, Auth};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{Notify, oneshot, watch};
use uuid::Uuid;
use zeroize::Zeroizing;

struct Peer {
    password: Zeroizing<String>,
    received: Arc<AtomicUsize>,
    ready: Option<oneshot::Sender<(server::Handle, russh::ChannelId)>>,
}

impl server::Handler for Peer {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if user == "fixture" && password == self.password.as_str() {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        _channel: russh::Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: russh::ChannelId,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        let _ = self.ready.take().unwrap().send((session.handle(), channel));
        Ok(())
    }

    async fn data(
        &mut self,
        _channel: russh::ChannelId,
        data: &[u8],
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.received.fetch_add(data.len(), Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn blocked_shell_input_keeps_output_and_cancellation_live() {
    tokio::time::timeout(Duration::from_secs(10), async {
        for action in ["close", "exit", "disconnect"] {
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
            let (ready, channel) = oneshot::channel();
            let peer = Peer { password: password.clone(), received: received.clone(), ready: Some(ready) };
            let config = Arc::new(server::Config {
                keys: vec![key], window_size: 0, maximum_packet_size: 1024,
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
            let (mut reader, writer) = connection.open_shell(80, 24).await.unwrap().split();
            let (remote, channel) = channel.await.unwrap();
            let (close, mut closing) = watch::channel(false);
            let entered = Arc::new(Notify::new());
            let output_ready = Arc::new(Notify::new());
            let output = Arc::new(Mutex::new(Vec::new()));
            let stdout = "output while input blocked: été 🦀\r\n".as_bytes();
            let stderr = b"stderr while input blocked\r\n";
            let expected_len = stdout.len() + stderr.len();
            let started = entered.clone();
            let observed = output.clone();
            let notified = output_ready.clone();
            let task = tokio::spawn(async move {
                let operation = async { started.notify_one(); writer.write(b"must-not-be-replayed").await };
                let result = run_shell_operation(operation, &mut reader, &mut closing, |bytes| {
                    let mut output = observed.lock().unwrap();
                    output.extend_from_slice(bytes);
                    if output.len() == expected_len { notified.notify_one(); }
                }).await;
                (result, reader, writer)
            });
            entered.notified().await;
            assert!(!task.is_finished(), "zero receive window must stall the actual write");
            remote.data(channel, stdout.to_vec()).await.unwrap();
            remote.extended_data(channel, 1, stderr.to_vec()).await.unwrap();
            tokio::time::timeout(Duration::from_secs(1), output_ready.notified()).await
                .expect("desktop must deliver stdout/stderr while terminal input waits for window credit");
            assert_eq!(*output.lock().unwrap(), [stdout, stderr].concat());
            assert!(!task.is_finished(), "output must not release the peer's zero input window");
            assert_eq!(received.load(Ordering::SeqCst), 0);
            match action {
                "close" => { close.send_replace(true); }
                "exit" => {
                    remote.eof(channel).await.unwrap();
                    remote.exit_status_request(channel, 0).await.unwrap();
                    remote.close(channel).await.unwrap();
                }
                _ => { remote.disconnect(russh::Disconnect::ByApplication, String::new(), "en".into()).await.unwrap(); }
            }
            let (result, reader, writer) = tokio::time::timeout(Duration::from_secs(1), task).await
                .expect("close/exit/transport loss must retire a blocked terminal write").unwrap();
            assert!(matches!((&result, action), (Err(ShellRunResult::Closed), "close" | "exit") | (Err(ShellRunResult::Lost(_)), "disconnect")));
            drop(reader); drop(writer);
            let _ = connection.disconnect().await;
            drop(connection);
            let result = server.await.unwrap();
            assert!(result.is_ok() || matches!(result, Err(russh::Error::Disconnect)));
            assert_eq!(received.load(Ordering::SeqCst), 0, "discarded input never reaches the peer");
            drop(TcpListener::bind(address).await.expect("owned listener released"));
        }
    }).await.expect("loopback backpressure fixture cleanup deadline");
}
