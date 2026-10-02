//! Real SSH packets on loopback; no OS accounts, sshd, agent or credential files.

use std::borrow::Cow;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mobarust_core::ConnectionState;
use mobarust_ssh::{
    HostKeyPolicy, Secret, SshConnectOptions, SshConnection, SshCredentials, SshError, SshOutput,
};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::keys::{HashAlg, PrivateKey};
use russh::server::{self, Auth, Response};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use uuid::Uuid;
use zeroize::Zeroizing;

const DEADLINE: Duration = Duration::from_secs(10);

#[derive(Clone, Copy)]
enum Method {
    Password,
    Distinct {
        together: bool,
    },
    Interactive {
        prompts: usize,
        echo: bool,
        rounds: usize,
    },
}

#[derive(Default)]
struct Observations {
    accepted: AtomicUsize,
    forwarded: AtomicUsize,
    password_requests: AtomicUsize,
    challenges: AtomicUsize,
    responses: AtomicUsize,
    authenticated: AtomicUsize,
    entered: Notify,
}

struct Handler {
    method: Method,
    expected: Zeroizing<String>,
    otp: Zeroizing<String>,
    observations: Arc<Observations>,
    release: Option<oneshot::Receiver<()>>,
    native_echo: bool,
    forward_to: Option<SocketAddr>,
    forwarded: JoinSet<()>,
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
            prompts: Cow::Owned(if let Method::Distinct { together } = self.method {
                if together {
                    vec![
                        (Cow::Borrowed("Password: "), false),
                        (Cow::Borrowed("OTP: "), false),
                    ]
                } else if self.observations.responses.load(Ordering::SeqCst) == 0 {
                    vec![(Cow::Borrowed("Password: "), false)]
                } else {
                    vec![(Cow::Borrowed("OTP: "), false)]
                }
            } else {
                vec![(Cow::Borrowed("Fixture response: "), echo); prompts]
            }),
        }
    }
}

impl server::Handler for Handler {
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
        while self.forwarded.try_join_next().is_some() {}
        if self.forwarded.len() >= 8 {
            return Ok(());
        }
        let Some(target) = self.forward_to else {
            return Ok(());
        };
        // Never resolve or connect to caller-selected destinations in this lab.
        if host != "127.0.0.1" || !target.ip().is_loopback() || port != u32::from(target.port()) {
            return Ok(());
        }
        let Ok(Ok(mut upstream)) = tokio::time::timeout(DEADLINE, TcpStream::connect(target)).await
        else {
            return Ok(());
        };
        reply.accept().await;
        self.observations.forwarded.fetch_add(1, Ordering::SeqCst);
        self.forwarded.spawn(async move {
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
        });
        Ok(())
    }

    async fn channel_open_session(
        &mut self,
        _channel: russh::Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if self.native_echo {
            reply.accept().await;
        }
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: russh::ChannelId,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if self.native_echo {
            session.channel_success(channel)?;
            session.data(
                channel,
                b"Disposable SSH authentication echo fixture (no OS shell).\r\n".to_vec(),
            )?;
        }
        Ok(())
    }

    async fn data(
        &mut self,
        channel: russh::ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if self.native_echo {
            session.data(channel, data.to_vec())?;
        }
        Ok(())
    }

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
        let (prompts, echo, rounds) = match self.method {
            Method::Interactive {
                prompts,
                echo,
                rounds,
            } => (prompts, echo, rounds),
            Method::Distinct { together } => {
                if together {
                    (2, false, 1)
                } else {
                    (1, false, 2)
                }
            }
            Method::Password => return Ok(Auth::reject()),
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
            || responses.iter().enumerate().any(|(index, value)| {
                let otp = matches!(self.method, Method::Distinct { together: true }) && index == 1
                    || matches!(self.method, Method::Distinct { together: false }) && round == 2;
                value.as_ref()
                    != if otp {
                        self.otp.as_bytes()
                    } else {
                        self.expected.as_bytes()
                    }
            })
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
    otp: Zeroizing<String>,
    observations: Arc<Observations>,
    release: Option<oneshot::Sender<()>>,
    worker: JoinHandle<Result<(), russh::Error>>,
    server_control: Option<oneshot::Receiver<server::Handle>>,
}

impl Fixture {
    async fn start(method: Method, stall: bool) -> Self {
        Self::start_configured(method, stall, None, false).await
    }

    async fn start_configured(
        method: Method,
        stall: bool,
        forward_to: Option<SocketAddr>,
        native_echo: bool,
    ) -> Self {
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
        let otp = Zeroizing::new(Uuid::new_v4().to_string());
        let observations = Arc::new(Observations::default());
        let (release, receiver) = oneshot::channel();
        let (server_control, server_control_receiver) = oneshot::channel();
        let handler = Handler {
            method,
            expected: secret.clone(),
            otp: otp.clone(),
            observations: observations.clone(),
            release: stall.then_some(receiver),
            native_echo,
            forward_to,
            forwarded: JoinSet::new(),
        };
        let accepted = observations.clone();
        let worker = tokio::spawn(async move {
            let (stream, peer) = tokio::time::timeout(DEADLINE, listener.accept())
                .await
                .expect("fixture accept deadline")?;
            assert!(peer.ip().is_loopback());
            accepted.accepted.fetch_add(1, Ordering::SeqCst);
            drop(listener);
            let session =
                tokio::time::timeout(DEADLINE, server::run_stream(config, stream, handler))
                    .await
                    .expect("fixture handshake deadline")?;
            let _ = server_control.send(session.handle());
            session.await
        });
        Self {
            address,
            fingerprint,
            secret,
            otp,
            observations,
            release: stall.then_some(release),
            worker,
            server_control: Some(server_control_receiver),
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
                Method::Interactive { .. } | Method::Distinct { .. } => {
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

    async fn finish_unconnected(mut self) {
        assert_eq!(self.observations.accepted.load(Ordering::SeqCst), 0);
        self.worker.abort();
        assert!((&mut self.worker).await.unwrap_err().is_cancelled());
        drop(TcpListener::bind(self.address).await.unwrap());
    }
}

/// Explicit manual GUI lab: only loopback, generated credentials and a bounded
/// echo channel. This is not an OpenSSH/PAM or remote OS-shell receipt.
#[cfg(unix)]
#[tokio::test]
#[ignore = "manual native desktop acceptance fixture; five-minute deadline"]
async fn native_authentication_lab() {
    native_lab(false).await;
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "manual native two-bastion challenge acceptance; five-minute deadline"]
async fn native_jump_authentication_lab() {
    native_lab(true).await;
}

#[cfg(unix)]
async fn native_lab(jumps: bool) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(if jumps {
            "target/jump-authentication-native-lab"
        } else {
            "target/authentication-native-lab"
        });
    std::fs::create_dir_all(&root).unwrap();
    let directory = native_lab_directory(&root);
    let mut endpoints = JoinSet::new();
    let target = native_endpoint(
        directory.path(),
        if jumps { "target" } else { "fixture" },
        None,
        &mut endpoints,
    )
    .await;
    if jumps {
        let second =
            native_endpoint(directory.path(), "bastion2", Some(target), &mut endpoints).await;
        native_endpoint(directory.path(), "bastion1", Some(second), &mut endpoints).await;
    }
    while let Some(result) = endpoints.join_next().await {
        result.unwrap();
    }
}

#[cfg(unix)]
fn native_lab_directory(root: &std::path::Path) -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(root)
        .unwrap()
}

#[cfg(unix)]
#[test]
fn native_lab_metadata_directory_is_private_and_removed() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let directory = native_lab_directory(root.path());
    let path = directory.path().to_owned();
    assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o700);
    drop(directory);
    assert!(!path.exists());
}

#[cfg(unix)]
async fn native_endpoint(
    directory: &std::path::Path,
    name: &str,
    forward_to: Option<SocketAddr>,
    endpoints: &mut JoinSet<()>,
) -> SocketAddr {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut seed = Zeroizing::new([0; 32]);
    seed[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    seed[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    let key = PrivateKey::new(KeypairData::Ed25519(Ed25519Keypair::from_seed(&seed)), "").unwrap();
    let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
    let password = Zeroizing::new(Uuid::new_v4().to_string());
    let otp = Zeroizing::new(Uuid::new_v4().to_string());
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let metadata = Zeroizing::new(format!(
        "{{\"host\":\"127.0.0.1\",\"port\":{},\"fingerprint\":\"{}\",\"password\":\"{}\",\"otp\":\"{}\"}}",
        address.port(),
        fingerprint,
        password.as_str(),
        otp.as_str()
    ));
    let metadata_path = directory.join(format!("{name}.json"));
    std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&metadata_path)
        .unwrap()
        .write_all(metadata.as_bytes())
        .unwrap();
    eprintln!(
        "Native authentication lab metadata: {}",
        metadata_path.display()
    );
    let config = Arc::new(server::Config {
        keys: vec![key],
        auth_rejection_time: Duration::ZERO,
        inactivity_timeout: Some(Duration::from_secs(180)),
        nodelay: true,
        ..Default::default()
    });
    endpoints.spawn(async move {
        let mut sessions = JoinSet::new();
        let lifetime = tokio::time::sleep(Duration::from_secs(300));
        tokio::pin!(lifetime);
        loop {
            tokio::select! {
                _ = &mut lifetime => break,
                completed = sessions.join_next(), if !sessions.is_empty() => {
                    let _ = completed.unwrap().unwrap();
                },
                peer = listener.accept() => {
                    let (stream, peer) = peer.unwrap();
                    assert!(peer.ip().is_loopback());
                    if sessions.len() >= 8 { drop(stream); continue; }
                    let handler = Handler {
                        method: Method::Distinct { together: false },
                        expected: password.clone(), otp: otp.clone(),
                        observations: Arc::new(Observations::default()), release: None,
                        native_echo: forward_to.is_none(), forward_to,
                        forwarded: JoinSet::new(),
                    };
                    let config = config.clone();
                    sessions.spawn(async move { server::run_stream(config, stream, handler).await?.await });
                }
            }
        }
        sessions.abort_all();
        while sessions.join_next().await.is_some() {}
    });
    address
}

#[tokio::test]
async fn distinct_challenges_are_owned_by_each_hop_and_fail_closed() {
    let method = Method::Distinct { together: false };
    for (failure, hop) in [
        ("none", 3),
        ("trust", 0),
        ("trust", 1),
        ("trust", 2),
        ("otp", 0),
        ("otp", 1),
        ("otp", 2),
        ("cancel", 0),
        ("cancel", 1),
        ("cancel", 2),
        ("drop", 0),
        ("drop", 1),
        ("drop", 2),
        ("forward-host", 2),
        ("forward-port", 2),
    ] {
        let target = Fixture::start_configured(method, false, None, true).await;
        let second = Fixture::start_configured(method, false, Some(target.address), false).await;
        let first = Fixture::start_configured(method, false, Some(second.address), false).await;
        let fixtures = [first, second, target];
        let trace = Arc::new(std::sync::Mutex::new(Vec::new()));
        let entered = Arc::new(Notify::new());
        let mut options = Vec::new();
        for (index, fixture) in fixtures.iter().enumerate() {
            let mut option = fixture.options(method, true);
            if failure == "trust" && index == hop {
                option.host_key_policy = HostKeyPolicy::RejectUnknown;
            }
            let password = fixture.secret.clone();
            let otp = if failure == "otp" && index == hop {
                Zeroizing::new(Uuid::new_v4().to_string())
            } else {
                fixture.otp.clone()
            };
            let trace = trace.clone();
            let entered = entered.clone();
            option.credentials =
                SshCredentials::keyboard_interactive_prompt("fixture", move |challenge| {
                    assert_eq!(challenge.prompts.len(), 1);
                    let prompt = challenge.prompts[0].as_str();
                    assert!(matches!(prompt, "Password: " | "OTP: "));
                    trace.lock().unwrap().push((index, prompt.to_owned()));
                    if failure == "drop" && index == hop {
                        entered.notify_one();
                    }
                    let response = if prompt == "OTP: " {
                        otp.clone()
                    } else {
                        password.clone()
                    };
                    async move {
                        if failure == "drop" && index == hop {
                            std::future::pending().await
                        } else if failure == "cancel" && index == hop {
                            Err(SshError::AuthenticationCancelled)
                        } else {
                            Ok(vec![Secret::from_zeroizing(response)])
                        }
                    }
                });
            options.push(option);
        }
        let mut target = options.pop().unwrap();
        if failure == "forward-host" {
            target.host = "192.0.2.1".into(); // Refused before any outbound TCP or DNS.
        } else if failure == "forward-port" {
            target.port = if target.port == u16::MAX {
                1
            } else {
                target.port + 1
            };
        }
        let connect = SshConnection::connect_with_jump_chain(target, options);
        let result = if failure == "drop" {
            let attempt = tokio::spawn(connect);
            tokio::time::timeout(DEADLINE, entered.notified())
                .await
                .expect("enter selected hop's responder");
            attempt.abort();
            assert!(
                attempt
                    .await
                    .err()
                    .expect("cancel pending hop authentication")
                    .is_cancelled()
            );
            None
        } else {
            Some(
                tokio::time::timeout(DEADLINE, connect)
                    .await
                    .expect("bounded two-bastion authentication"),
            )
        };
        match (failure, result) {
            ("drop", None) => {}
            ("none", Some(result)) => {
                let connection = result.expect("accept distinct responses at all three endpoints");
                assert_eq!(connection.state(), ConnectionState::Connected);
                tokio::time::timeout(DEADLINE, async {
                    let mut shell = connection.open_shell(100, 30).await.unwrap();
                    let marker = "jump authentication: été 🦀\r\n".as_bytes();
                    shell.write(marker).await.unwrap();
                    let expected = [
                        b"Disposable SSH authentication echo fixture (no OS shell).\r\n".as_slice(),
                        marker,
                    ]
                    .concat();
                    let mut output = Vec::new();
                    while output.len() < expected.len() {
                        if let SshOutput::Stdout(data) = shell.next_output().await.unwrap().unwrap()
                        {
                            output.extend(data);
                        }
                    }
                    assert_eq!(output, expected);
                })
                .await
                .expect("echo channel round trip through both bastions");
                connection.disconnect().await.unwrap();
                drop(connection);
            }
            ("trust", Some(result)) => {
                assert!(matches!(result, Err(SshError::HostKeyRejected { .. })))
            }
            ("otp", Some(result)) => {
                assert!(matches!(result, Err(SshError::AuthenticationRejected)))
            }
            ("cancel", Some(result)) => {
                assert!(matches!(result, Err(SshError::AuthenticationCancelled)))
            }
            (_, Some(result)) => assert!(matches!(result, Err(SshError::Channel(_)))),
            _ => panic!("missing authentication result"),
        }
        let mut expected = Vec::new();
        for (index, fixture) in fixtures.iter().enumerate() {
            if index < hop || failure == "otp" && index == hop {
                expected.extend([(index, "Password: ".into()), (index, "OTP: ".into())]);
            } else if matches!(failure, "cancel" | "drop") && index == hop {
                expected.push((index, "Password: ".into()));
            }
            let observation = &fixture.observations;
            assert_eq!(
                observation.authenticated.load(Ordering::SeqCst),
                usize::from(index < hop)
            );
            assert_eq!(
                observation.challenges.load(Ordering::SeqCst),
                expected.iter().filter(|(owner, _)| *owner == index).count()
            );
            assert_eq!(
                observation.responses.load(Ordering::SeqCst),
                expected.iter().filter(|(owner, _)| *owner == index).count()
                    - usize::from(matches!(failure, "cancel" | "drop") && index == hop)
            );
            assert_eq!(
                observation.forwarded.load(Ordering::SeqCst),
                usize::from(
                    index < 2 && index < hop && !(failure.starts_with("forward-") && index == 1)
                )
            );
        }
        assert_eq!(*trace.lock().unwrap(), expected, "{failure} at hop {hop}");
        for fixture in fixtures {
            if fixture.observations.accepted.load(Ordering::SeqCst) == 0 {
                fixture.finish_unconnected().await;
            } else {
                fixture.finish().await;
            }
        }
    }
}

#[tokio::test]
async fn distinct_password_and_otp_work_in_one_or_multiple_rounds() {
    for together in [true, false] {
        let method = Method::Distinct { together };
        // Receipt of the old behavior: one static secret cannot satisfy OTP.
        let fixture = Fixture::start(method, false).await;
        assert!(matches!(
            SshConnection::connect(fixture.options(method, true)).await,
            Err(SshError::AuthenticationRejected)
        ));
        fixture.finish().await;
        for correct in [true, false] {
            let fixture = Fixture::start(method, false).await;
            let mut options = fixture.options(method, true);
            let password = fixture.secret.clone();
            let otp = if correct {
                fixture.otp.clone()
            } else {
                Zeroizing::new(Uuid::new_v4().to_string())
            };
            options.credentials =
                SshCredentials::keyboard_interactive_prompt("fixture", move |challenge| {
                    let responses = challenge
                        .prompts
                        .iter()
                        .map(|prompt| {
                            Secret::from_zeroizing(if prompt == "OTP: " {
                                otp.clone()
                            } else {
                                password.clone()
                            })
                        })
                        .collect();
                    async move { Ok(responses) }
                });
            let result = SshConnection::connect(options).await;
            if correct {
                let connection = result.expect("accept distinct generated factors");
                assert_eq!(connection.state(), ConnectionState::Connected);
                connection
                    .disconnect()
                    .await
                    .expect("disconnect interactive client");
                drop(connection);
            } else {
                assert!(matches!(result, Err(SshError::AuthenticationRejected)));
            }
            assert_eq!(
                fixture.observations.authenticated.load(Ordering::SeqCst),
                usize::from(correct)
            );
            fixture.finish().await;
        }
    }
}

#[tokio::test]
async fn interactive_responder_is_bounded_and_cancelled_without_sending_secrets() {
    for case in [
        "echo",
        "fanout",
        "rounds",
        "count",
        "size",
        "cancel",
        "drop",
        "trust",
        "disconnect",
    ] {
        let method = Method::Interactive {
            prompts: if case == "fanout" { 9 } else { 1 },
            echo: case == "echo",
            rounds: if case == "rounds" { 17 } else { 1 },
        };
        let mut fixture = Fixture::start(method, false).await;
        let mut options = fixture.options(method, true);
        let response = fixture.secret.clone();
        let entered = Arc::new(Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_callback = calls.clone();
        let entered_for_callback = entered.clone();
        options.credentials = SshCredentials::keyboard_interactive_prompt("fixture", move |_| {
            calls_for_callback.fetch_add(1, Ordering::SeqCst);
            entered_for_callback.notify_one();
            let response = response.clone();
            async move {
                match case {
                    "cancel" => Err(SshError::AuthenticationCancelled),
                    "drop" | "disconnect" => std::future::pending().await,
                    "count" => Ok(Vec::new()),
                    "size" => Ok(vec![Secret::new(
                        "s".repeat(mobarust_ssh::MAX_KEYBOARD_INTERACTIVE_RESPONSE_BYTES + 1),
                    )]),
                    _ => Ok(vec![Secret::from_zeroizing(response)]),
                }
            }
        });
        if case == "trust" {
            options.host_key_policy = HostKeyPolicy::RejectUnknown;
        }
        if case == "drop" || case == "disconnect" {
            let mut attempt = tokio::spawn(SshConnection::connect(options));
            tokio::time::timeout(DEADLINE, entered.notified())
                .await
                .expect("enter responder");
            if case == "drop" {
                attempt.abort();
                assert!(
                    attempt
                        .await
                        .err()
                        .expect("cancel response wait")
                        .is_cancelled()
                );
            } else {
                fixture
                    .server_control
                    .take()
                    .unwrap()
                    .await
                    .unwrap()
                    .disconnect(russh::Disconnect::ByApplication, String::new(), "en".into())
                    .await
                    .unwrap();
                let result = tokio::time::timeout(Duration::from_secs(1), &mut attempt)
                    .await
                    .expect(
                        "server loss must retire a pending response without waiting 120 seconds",
                    )
                    .unwrap();
                assert!(matches!(result, Err(SshError::Transport(_))));
            }
        } else {
            let error = SshConnection::connect(options)
                .await
                .err()
                .expect("refuse challenge or responses");
            assert!(match case {
                "echo" => matches!(error, SshError::KeyboardInteractiveEchoPrompt),
                "fanout" => matches!(error, SshError::KeyboardInteractiveTooManyPrompts),
                "rounds" => matches!(error, SshError::KeyboardInteractiveChallengeLimit),
                "trust" => matches!(error, SshError::HostKeyRejected { .. }),
                "cancel" => matches!(error, SshError::AuthenticationCancelled),
                _ => matches!(error, SshError::KeyboardInteractiveInvalidResponses),
            });
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            match case {
                "echo" | "fanout" | "trust" => 0,
                "rounds" => 16,
                _ => 1,
            }
        );
        assert_eq!(
            fixture.observations.responses.load(Ordering::SeqCst),
            if case == "rounds" { 16 } else { 0 }
        );
        fixture.finish().await;
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
