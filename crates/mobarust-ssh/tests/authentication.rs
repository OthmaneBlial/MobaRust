//! Real SSH packets on loopback; no OS accounts, sshd, agent or credential files.

use std::borrow::Cow;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mobarust_core::ConnectionState;
use mobarust_ssh::{
    HostKeyPolicy, Secret, SshConnectOptions, SshConnection, SshCredentials, SshError,
};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::keys::{HashAlg, PrivateKey};
use russh::server::{self, Auth, Response};
use tokio::net::TcpListener;
use tokio::sync::{Notify, oneshot};
use tokio::task::JoinHandle;
use uuid::Uuid;
use zeroize::Zeroizing;

const DEADLINE: Duration = Duration::from_secs(10);

#[derive(Clone, Copy)]
enum Method {
    Password,
    Interactive {
        prompts: usize,
        echo: bool,
        rounds: usize,
    },
}

#[derive(Default)]
struct Observations {
    password_requests: AtomicUsize,
    challenges: AtomicUsize,
    responses: AtomicUsize,
    authenticated: AtomicUsize,
    entered: Notify,
}

struct Handler {
    method: Method,
    expected: Zeroizing<String>,
    observations: Arc<Observations>,
    release: Option<oneshot::Receiver<()>>,
}

impl Handler {
    async fn stall_if_requested(&mut self) {
        self.observations.entered.notify_one();
        if let Some(release) = self.release.take() {
            tokio::time::timeout(DEADLINE, release)
                .await
                .expect("fixture authentication release deadline")
                .expect("release fixture authentication");
        }
    }

    fn challenge(&self, prompts: usize, echo: bool) -> Auth {
        self.observations.challenges.fetch_add(1, Ordering::SeqCst);
        Auth::Partial {
            name: Cow::Borrowed("Disposable authentication fixture"),
            instructions: Cow::Borrowed(""),
            prompts: Cow::Owned(vec![(Cow::Borrowed("Fixture response: "), echo); prompts]),
        }
    }
}

impl server::Handler for Handler {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        self.observations
            .password_requests
            .fetch_add(1, Ordering::SeqCst);
        self.stall_if_requested().await;
        Ok(
            if matches!(self.method, Method::Password)
                && user == "fixture"
                && password == self.expected.as_str()
            {
                Auth::Accept
            } else {
                Auth::reject()
            },
        )
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        user: &str,
        _submethods: &str,
        response: Option<Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        let Method::Interactive {
            prompts,
            echo,
            rounds,
        } = self.method
        else {
            return Ok(Auth::reject());
        };
        if user != "fixture" {
            return Ok(Auth::reject());
        }
        let Some(response) = response else {
            self.stall_if_requested().await;
            return Ok(self.challenge(prompts, echo));
        };
        let round = self.observations.responses.fetch_add(1, Ordering::SeqCst) + 1;
        let responses: Vec<_> = response.collect();
        if responses.len() != prompts
            || responses
                .iter()
                .any(|value| value.as_ref() != self.expected.as_bytes())
        {
            return Ok(Auth::reject());
        }
        Ok(if round < rounds {
            self.challenge(prompts, echo)
        } else {
            Auth::Accept
        })
    }

    async fn auth_succeeded(&mut self, _session: &mut server::Session) -> Result<(), Self::Error> {
        self.observations
            .authenticated
            .fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct Fixture {
    address: SocketAddr,
    fingerprint: String,
    secret: Zeroizing<String>,
    observations: Arc<Observations>,
    release: Option<oneshot::Sender<()>>,
    worker: JoinHandle<Result<(), russh::Error>>,
}

impl Fixture {
    async fn start(method: Method, stall: bool) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind loopback fixture");
        let address = listener.local_addr().expect("fixture address");
        assert!(address.ip().is_loopback());
        // UUID v4 uses OS randomness. Two UUIDs supply a disposable seed;
        // private material is memory-only and zeroized, never a production key.
        let mut seed = Zeroizing::new([0; 32]);
        seed[..16].copy_from_slice(Uuid::new_v4().as_bytes());
        seed[16..].copy_from_slice(Uuid::new_v4().as_bytes());
        let key = PrivateKey::new(KeypairData::Ed25519(Ed25519Keypair::from_seed(&seed)), "")
            .expect("create disposable host key");
        let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
        let config = Arc::new(server::Config {
            keys: vec![key],
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            // Cleanup must follow client closure, not an inactivity deadline.
            inactivity_timeout: None,
            nodelay: true,
            ..Default::default()
        });
        let secret = Zeroizing::new(Uuid::new_v4().to_string());
        let observations = Arc::new(Observations::default());
        let (release, receiver) = oneshot::channel();
        let handler = Handler {
            method,
            expected: secret.clone(),
            observations: observations.clone(),
            release: stall.then_some(receiver),
        };
        let worker = tokio::spawn(async move {
            let (stream, peer) = tokio::time::timeout(DEADLINE, listener.accept())
                .await
                .expect("fixture accept deadline")?;
            assert!(peer.ip().is_loopback());
            drop(listener);
            let session =
                tokio::time::timeout(DEADLINE, server::run_stream(config, stream, handler))
                    .await
                    .expect("fixture handshake deadline")?;
            session.await
        });
        Self {
            address,
            fingerprint,
            secret,
            observations,
            release: stall.then_some(release),
            worker,
        }
    }

    fn options(&self, method: Method, correct: bool) -> SshConnectOptions {
        let secret = Secret::from_zeroizing(if correct {
            self.secret.clone()
        } else {
            Zeroizing::new(Uuid::new_v4().to_string())
        });
        SshConnectOptions {
            host: self.address.ip().to_string(),
            port: self.address.port(),
            host_key_policy: HostKeyPolicy::PinnedFingerprint(self.fingerprint.clone()),
            timeout: DEADLINE,
            keepalive_interval: None,
            credentials: match method {
                Method::Password => SshCredentials::password_secret("fixture", secret),
                Method::Interactive { .. } => {
                    SshCredentials::keyboard_interactive_secret("fixture", secret)
                }
            },
            x11: None,
            environment: Vec::new(),
            startup_directory: None,
            startup_command: None,
        }
    }

    async fn finish(mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        let result = tokio::time::timeout(DEADLINE, &mut self.worker)
            .await
            .expect("client must close its socket and retire the server session")
            .expect("fixture worker must not panic");
        // EOF/reset during rejected authentication is an ordinary peer close.
        if let Err(error) = result {
            assert!(
                matches!(error, russh::Error::Disconnect)
                    || matches!(&error, russh::Error::IO(error) if matches!(error.kind(),
                        std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::BrokenPipe)),
                "unexpected fixture protocol error: {error}"
            );
        }
        let listener = TcpListener::bind(self.address)
            .await
            .expect("fixture listener must be released");
        drop(listener);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.worker.abort();
    }
}

#[tokio::test]
async fn password_success_and_rejection_close_the_session() {
    for correct in [true, false] {
        let fixture = Fixture::start(Method::Password, false).await;
        let result = SshConnection::connect(fixture.options(Method::Password, correct)).await;
        if correct {
            let connection = result.expect("accept generated password");
            assert_eq!(connection.state(), ConnectionState::Connected);
            connection
                .disconnect()
                .await
                .expect("disconnect authenticated client");
            drop(connection);
        } else {
            assert!(matches!(result, Err(SshError::AuthenticationRejected)));
        }
        assert_eq!(
            fixture
                .observations
                .password_requests
                .load(Ordering::SeqCst),
            1
        );
        assert_eq!(
            fixture.observations.authenticated.load(Ordering::SeqCst),
            usize::from(correct)
        );
        fixture.finish().await;
    }
}

#[tokio::test]
async fn interactive_success_and_rejection_cross_real_ssh_packets() {
    for (prompts, rounds, correct) in [(0, 1, true), (1, 1, true), (8, 2, true), (1, 1, false)] {
        let method = Method::Interactive {
            prompts,
            echo: false,
            rounds,
        };
        let fixture = Fixture::start(method, false).await;
        let result = SshConnection::connect(fixture.options(method, correct)).await;
        if correct {
            let connection = result.expect("accept generated interactive response");
            assert_eq!(connection.state(), ConnectionState::Connected);
            connection
                .disconnect()
                .await
                .expect("disconnect authenticated client");
            drop(connection);
        } else {
            assert!(matches!(result, Err(SshError::AuthenticationRejected)));
        }
        assert_eq!(
            fixture.observations.challenges.load(Ordering::SeqCst),
            rounds
        );
        assert_eq!(
            fixture.observations.responses.load(Ordering::SeqCst),
            rounds
        );
        assert_eq!(
            fixture.observations.authenticated.load(Ordering::SeqCst),
            usize::from(correct)
        );
        fixture.finish().await;
    }
}

#[tokio::test]
async fn hostile_interactive_prompts_receive_no_secret_response() {
    for (prompts, echo) in [(1, true), (9, false)] {
        let method = Method::Interactive {
            prompts,
            echo,
            rounds: 1,
        };
        let fixture = Fixture::start(method, false).await;
        let error = SshConnection::connect(fixture.options(method, true))
            .await
            .err()
            .expect("refuse hostile challenge");
        assert!(if echo {
            matches!(error, SshError::KeyboardInteractiveEchoPrompt)
        } else {
            matches!(error, SshError::KeyboardInteractiveTooManyPrompts)
        });
        assert_eq!(fixture.observations.challenges.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.observations.responses.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.observations.authenticated.load(Ordering::SeqCst), 0);
        fixture.finish().await;
    }
}

#[tokio::test]
async fn host_key_rejection_precedes_password_or_interactive_authentication() {
    for method in [
        Method::Password,
        Method::Interactive {
            prompts: 1,
            echo: false,
            rounds: 1,
        },
    ] {
        let fixture = Fixture::start(method, false).await;
        let mut options = fixture.options(method, true);
        options.host_key_policy = HostKeyPolicy::RejectUnknown;
        assert!(matches!(
            SshConnection::connect(options).await,
            Err(SshError::HostKeyRejected { .. })
        ));
        assert_eq!(
            fixture
                .observations
                .password_requests
                .load(Ordering::SeqCst),
            0
        );
        assert_eq!(fixture.observations.challenges.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.observations.responses.load(Ordering::SeqCst), 0);
        fixture.finish().await;
    }
}

#[tokio::test]
async fn timeout_and_cancellation_during_authentication_close_the_client_socket() {
    for method in [
        Method::Password,
        Method::Interactive {
            prompts: 1,
            echo: false,
            rounds: 1,
        },
    ] {
        for cancel in [false, true] {
            let fixture = Fixture::start(method, true).await;
            let mut options = fixture.options(method, true);
            if !cancel {
                options.timeout = Duration::from_secs(1);
            }
            let attempt = tokio::spawn(SshConnection::connect(options));
            tokio::time::timeout(DEADLINE, fixture.observations.entered.notified())
                .await
                .expect("must reach authentication before cancellation/timeout");
            if cancel {
                attempt.abort();
                assert!(
                    attempt
                        .await
                        .err()
                        .expect("cancel connect task")
                        .is_cancelled()
                );
            } else {
                let result = tokio::time::timeout(DEADLINE, attempt)
                    .await
                    .expect("authentication timeout deadline")
                    .expect("connect task");
                assert!(matches!(result, Err(SshError::Timeout)));
            }
            fixture.finish().await;
        }
    }
}
