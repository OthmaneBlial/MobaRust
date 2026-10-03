//! Remote listener ownership over real SSH packets, restricted to loopback.
use mobarust_ssh::{
    HostKeyPolicy, Secret, SshConnectOptions, SshConnection, SshCredentials, SshError,
};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::keys::{HashAlg, PrivateKey};
use russh::server::{self, Auth};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;
use uuid::Uuid;
use zeroize::Zeroizing;

struct Peer {
    password: Zeroizing<String>,
    requested: u32,
    listener: Option<TcpListener>,
    requests: Arc<AtomicUsize>,
    cancellations: Arc<AtomicUsize>,
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

    async fn tcpip_forward(
        &mut self,
        address: &str,
        port: &mut u32,
        _session: &mut server::Session,
    ) -> Result<bool, Self::Error> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        // No DNS or caller-selected endpoints: only this test's reserved port.
        if address != "127.0.0.1" || *port != self.requested || self.listener.is_some() {
            return Ok(false);
        }
        let listener = TcpListener::bind(("127.0.0.1", u16::try_from(*port).unwrap())).await?;
        if *port == 0 {
            *port = u32::from(listener.local_addr()?.port());
        }
        self.listener = Some(listener);
        Ok(true)
    }

    async fn cancel_tcpip_forward(
        &mut self,
        address: &str,
        port: u32,
        _session: &mut server::Session,
    ) -> Result<bool, Self::Error> {
        let matches = address == "127.0.0.1"
            && self
                .listener
                .as_ref()
                .is_some_and(|listener| u32::from(listener.local_addr().unwrap().port()) == port);
        if matches {
            drop(self.listener.take());
            self.cancellations.fetch_add(1, Ordering::SeqCst);
        }
        Ok(matches)
    }
}

#[tokio::test]
async fn remote_forward_returns_the_bound_port_and_cancels_without_disconnect() {
    tokio::time::timeout(Duration::from_secs(10), async {
        for allocated in [false, true] {
            let reservation = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let requested = if allocated {
                0
            } else {
                u32::from(reservation.local_addr().unwrap().port())
            };
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let address = listener.local_addr().unwrap();
            let mut seed = Zeroizing::new([0; 32]);
            seed[..16].copy_from_slice(Uuid::new_v4().as_bytes());
            seed[16..].copy_from_slice(Uuid::new_v4().as_bytes());
            let key = PrivateKey::new(KeypairData::Ed25519(Ed25519Keypair::from_seed(&seed)), "")
                .unwrap();
            let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
            let password = Zeroizing::new(Uuid::new_v4().to_string());
            let requests = Arc::new(AtomicUsize::new(0));
            let cancellations = Arc::new(AtomicUsize::new(0));
            let peer = Peer {
                password: password.clone(),
                requested,
                listener: None,
                requests: requests.clone(),
                cancellations: cancellations.clone(),
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
            let connection = SshConnection::connect(SshConnectOptions {
                host: "127.0.0.1".into(),
                port: address.port(),
                host_key_policy: HostKeyPolicy::PinnedFingerprint(fingerprint),
                timeout: Duration::from_secs(5),
                credentials: SshCredentials::password_secret(
                    "fixture",
                    Secret::from_zeroizing(password),
                ),
                keepalive_interval: None,
                x11: None,
                environment: Vec::new(),
                startup_directory: None,
                startup_command: None,
            })
            .await
            .unwrap();
            drop(reservation);
            let bound = connection
                .request_remote_forward("127.0.0.1", requested)
                .await
                .unwrap();
            assert_ne!(
                bound, 0,
                "successful forwarding must identify the actual listener"
            );
            if !allocated {
                assert_eq!(u32::from(bound), requested);
            }
            assert!(
                TcpListener::bind(("127.0.0.1", bound)).await.is_err(),
                "server owns the listener"
            );
            connection
                .cancel_remote_forward("127.0.0.1", u32::from(bound))
                .await
                .unwrap();
            assert_eq!(cancellations.load(Ordering::SeqCst), 1);
            drop(
                TcpListener::bind(("127.0.0.1", bound))
                    .await
                    .expect("Cancel releases the listener before disconnect"),
            );
            assert!(!server.is_finished(), "Cancel preserves the SSH transport");
            assert!(matches!(
                connection.request_remote_forward("127.0.0.1", 65536).await,
                Err(SshError::InvalidOptions)
            ));
            assert_eq!(
                requests.load(Ordering::SeqCst),
                1,
                "invalid ports never reach the server"
            );
            let second = connection
                .request_remote_forward("127.0.0.1", requested)
                .await
                .expect("the same SSH transport can start another forward after Cancel");
            assert_ne!(second, 0);
            if !allocated {
                assert_eq!(u32::from(second), requested);
            }
            connection
                .cancel_remote_forward("127.0.0.1", u32::from(second))
                .await
                .unwrap();
            assert_eq!(requests.load(Ordering::SeqCst), 2);
            assert_eq!(cancellations.load(Ordering::SeqCst), 2);
            drop(TcpListener::bind(("127.0.0.1", second)).await.unwrap());
            connection.disconnect().await.unwrap();
            drop(connection);
            if let Err(error) = server.await.unwrap() {
                assert!(
                    matches!(error, russh::Error::Disconnect)
                        || matches!(&error, russh::Error::IO(error) if matches!(error.kind(),
                        std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::BrokenPipe)),
                    "unexpected fixture error: {error}"
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
    .expect("remote forward fixture cleanup deadline");
}
