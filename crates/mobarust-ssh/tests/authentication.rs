//! Real SSH packets on loopback; no OS accounts, sshd, agent or credential files.

use std::borrow::Cow;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
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
const SHELL_BANNER: &[u8] = b"Disposable SSH authentication echo fixture (no OS shell).\r\n";
const SHELL_PRELUDE: &[u8] = "prelude: été 🦀\r\n".as_bytes();
const SHELL_STDERR: &[u8] = b"setup stderr\r\n";
const SFTP_VERSION: &[u8] = &[0, 0, 0, 5, 2, 0, 0, 0, 3];
const SFTP_UNSUPPORTED: &[u8] = &[
    0, 0, 0, 17, 101, 0, 0, 0, 1, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0,
];

#[derive(Clone, Copy, Debug)]
enum SftpReply {
    Accept,
    Reject,
    Close,
    Silent,
    Cancel,
    Prelude,
    Flood,
    OversizedPacket,
}

#[derive(Clone, Copy, Debug)]
enum ShellReply {
    Accept,
    Reject,
    Silent,
    Close,
    Prelude,
    Flood,
    X11Reject,
    StartupFlood,
    StartupStall,
    StartupExit,
    StartupPartialExit,
    #[cfg(unix)]
    StartupReconnectStall,
    StartupOverflow,
    ExitBeforeOutput,
    EofBeforeExit,
}

#[cfg(unix)]
const NATIVE_SHELL_CASES: [(&str, ShellReply); 3] = [
    ("startup", ShellReply::StartupFlood),
    ("rejected", ShellReply::Reject),
    ("stalled", ShellReply::StartupStall),
];

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
    shell_requests: AtomicUsize,
    shell_input_bytes: AtomicUsize,
    x11_requests: AtomicUsize,
    closed_channels: AtomicUsize,
    channel_closed: Notify,
    startup_input: Mutex<Vec<u8>>,
    sftp_requests: AtomicUsize,
    sftp_input_bytes: AtomicUsize,
    sftp_entered: Notify,
}

struct Handler {
    method: Method,
    expected: Zeroizing<String>,
    otp: Zeroizing<String>,
    observations: Arc<Observations>,
    release: Option<oneshot::Receiver<()>>,
    native_echo: bool,
    shell_reply: ShellReply,
    forward_to: Option<SocketAddr>,
    forwarded: JoinSet<()>,
    sftp_reply: Option<(SftpReply, bool)>,
    sftp_channels: std::collections::HashSet<russh::ChannelId>,
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
            self.observations
                .shell_requests
                .fetch_add(1, Ordering::SeqCst);
            match self.shell_reply {
                ShellReply::Reject => {
                    session.channel_failure(channel)?;
                    return Ok(());
                }
                ShellReply::Silent => return Ok(()),
                ShellReply::Close => {
                    session.close(channel)?;
                    return Ok(());
                }
                ShellReply::Prelude => {
                    let split = SHELL_PRELUDE.len() - 4;
                    session.data(channel, SHELL_PRELUDE[..split].to_vec())?;
                    session.data(channel, SHELL_PRELUDE[split..].to_vec())?;
                    session.extended_data(channel, 1, SHELL_STDERR.to_vec())?;
                }
                ShellReply::Flood => {
                    for _ in 0..33 {
                        session.data(channel, vec![b'x'; 32 * 1024])?;
                    }
                }
                _ => {}
            }
            session.channel_success(channel)?;
            if matches!(self.shell_reply, ShellReply::StartupFlood) {
                for _ in 0..256 {
                    session.data(channel, vec![b'x'; 1024])?;
                }
            }
            if matches!(self.shell_reply, ShellReply::StartupOverflow) {
                for _ in 0..33 {
                    session.data(channel, vec![b'x'; 32 * 1024])?;
                }
            }
            session.data(channel, SHELL_BANNER.to_vec())?;
            if matches!(self.shell_reply, ShellReply::ExitBeforeOutput) {
                session.exit_status_request(channel, 23)?;
                session.data(channel, b"output after exit status\r\n".to_vec())?;
                session.extended_data(channel, 1, b"stderr after exit status\r\n".to_vec())?;
                session.eof(channel)?;
                session.close(channel)?;
            }
            if matches!(self.shell_reply, ShellReply::EofBeforeExit) {
                session.eof(channel)?;
                session.exit_status_request(channel, 23)?;
                session.close(channel)?;
            }
            if matches!(self.shell_reply, ShellReply::StartupStall) {
                self.observations.entered.notify_one();
            }
            if matches!(self.shell_reply, ShellReply::StartupExit) {
                session.eof(channel)?;
                session.exit_status_request(channel, 0)?;
                session.close(channel)?;
            }
        }
        Ok(())
    }

    async fn data(
        &mut self,
        channel: russh::ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if self.sftp_channels.contains(&channel) {
            self.observations
                .sftp_input_bytes
                .fetch_add(data.len(), Ordering::SeqCst);
            let (reply, listing) = self.sftp_reply.unwrap();
            let reply = if listing && self.observations.sftp_requests.load(Ordering::SeqCst) == 1 {
                SftpReply::Accept
            } else {
                reply
            };
            if data == [0, 0, 0, 5, 1, 0, 0, 0, 3] {
                if matches!(reply, SftpReply::Accept) {
                    session.data(channel, SFTP_VERSION.to_vec())?;
                } else if matches!(reply, SftpReply::OversizedPacket) {
                    // Only a header: never allocate or send a giant payload.
                    let length = russh_sftp::client::Config::default().max_packet_len + 1;
                    session.data(channel, length.to_be_bytes().to_vec())?;
                }
            } else {
                // The fixture has no filesystem: refuse its one OPENDIR request.
                assert_eq!(&data[4..9], &[11, 0, 0, 0, 1]);
                session.data(channel, SFTP_UNSUPPORTED.to_vec())?;
            }
            return Ok(());
        }
        if self.native_echo {
            self.observations
                .shell_input_bytes
                .fetch_add(data.len(), Ordering::SeqCst);
            if matches!(
                self.shell_reply,
                ShellReply::StartupFlood | ShellReply::StartupPartialExit
            ) {
                let mut input = self.observations.startup_input.lock().unwrap();
                assert!(
                    input.len() + data.len() <= 16 * 1024,
                    "bounded fixture input receipt"
                );
                input.extend_from_slice(data);
            }
            if matches!(self.shell_reply, ShellReply::StartupPartialExit) {
                session.exit_status_request(channel, 23)?;
                session.close(channel)?;
                return Ok(());
            }
            session.data(channel, data.to_vec())?;
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
        let request = self
            .observations
            .sftp_requests
            .fetch_add(1, Ordering::SeqCst);
        self.observations.sftp_entered.notify_one();
        let Some((reply, listing)) = self.sftp_reply else {
            session.channel_failure(channel)?;
            return Ok(());
        };
        self.sftp_channels.insert(channel);
        let reply = if listing && request == 0 {
            SftpReply::Accept
        } else {
            reply
        };
        match reply {
            SftpReply::Reject => session.channel_failure(channel)?,
            SftpReply::Close => session.close(channel)?,
            SftpReply::Silent | SftpReply::Cancel => {}
            SftpReply::Flood => {
                for _ in 0..33 {
                    session.data(channel, vec![b'x'; 32 * 1024])?;
                }
            }
            SftpReply::Accept | SftpReply::Prelude | SftpReply::OversizedPacket => {
                if matches!(reply, SftpReply::Prelude) {
                    session.data(channel, SFTP_VERSION[..6].to_vec())?;
                    session.extended_data(
                        channel,
                        1,
                        b"fixture diagnostic must stay private".to_vec(),
                    )?;
                    session.data(channel, SFTP_VERSION[6..].to_vec())?;
                }
                session.channel_success(channel)?;
            }
        }
        Ok(())
    }

    async fn x11_request(
        &mut self,
        channel: russh::ChannelId,
        _single_connection: bool,
        _protocol: &str,
        _cookie: &str,
        _screen: u32,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.observations
            .x11_requests
            .fetch_add(1, Ordering::SeqCst);
        if matches!(self.shell_reply, ShellReply::X11Reject) {
            session.channel_failure(channel)?;
        } else {
            session.channel_success(channel)?;
        }
        Ok(())
    }

    async fn channel_close(
        &mut self,
        _channel: russh::ChannelId,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.observations
            .closed_channels
            .fetch_add(1, Ordering::SeqCst);
        self.observations.channel_closed.notify_one();
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
        Self::start_with_shell(method, stall, forward_to, native_echo, ShellReply::Accept).await
    }

    async fn start_with_shell(
        method: Method,
        stall: bool,
        forward_to: Option<SocketAddr>,
        native_echo: bool,
        shell_reply: ShellReply,
    ) -> Self {
        Self::start_with_replies(method, stall, forward_to, native_echo, shell_reply, None).await
    }

    async fn start_with_replies(
        method: Method,
        stall: bool,
        forward_to: Option<SocketAddr>,
        native_echo: bool,
        shell_reply: ShellReply,
        sftp_reply: Option<(SftpReply, bool)>,
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
        let mut config = server::Config {
            keys: vec![key],
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            // Cleanup must follow client closure, not an inactivity deadline.
            inactivity_timeout: None,
            nodelay: true,
            ..Default::default()
        };
        if matches!(
            shell_reply,
            ShellReply::StartupFlood | ShellReply::StartupPartialExit
        ) {
            config.window_size = 1024;
        }
        if matches!(
            shell_reply,
            ShellReply::StartupStall | ShellReply::StartupExit | ShellReply::StartupOverflow
        ) {
            config.window_size = 0;
        }
        let config = Arc::new(config);
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
            shell_reply,
            forward_to,
            forwarded: JoinSet::new(),
            sftp_reply,
            sftp_channels: std::collections::HashSet::new(),
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

/// Separate GUI setup lab: generated password/OTP, no OS command execution.
#[cfg(unix)]
#[tokio::test]
#[ignore = "manual native shell startup acceptance; five-minute deadline"]
async fn native_shell_setup_lab() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target/shell-setup-native-lab");
    std::fs::create_dir_all(&root).unwrap();
    let directory = native_lab_directory(&root);
    let mut endpoints = JoinSet::new();
    native_shell_endpoints(
        directory.path(),
        &NATIVE_SHELL_CASES,
        Duration::from_secs(300),
        &mut endpoints,
    )
    .await;
    while let Some(result) = endpoints.join_next().await {
        result.unwrap();
    }
}

/// Success first; a private marker interrupts it. Later connections retain the
/// same trust/credentials but have zero startup credit.
#[cfg(unix)]
#[tokio::test]
#[ignore = "manual native reconnect startup acceptance; five-minute deadline"]
async fn native_shell_reconnect_lab() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target/shell-reconnect-native-lab");
    std::fs::create_dir_all(&root).unwrap();
    let directory = native_lab_directory(&root);
    let mut endpoints = JoinSet::new();
    native_shell_endpoints(
        directory.path(),
        &[("reconnect-stalled", ShellReply::StartupReconnectStall)],
        Duration::from_secs(300),
        &mut endpoints,
    )
    .await;
    while let Some(result) = endpoints.join_next().await {
        result.unwrap();
    }
}

#[cfg(unix)]
async fn native_shell_endpoints(
    directory: &std::path::Path,
    cases: &[(&str, ShellReply)],
    lifetime: Duration,
    endpoints: &mut JoinSet<()>,
) -> Vec<SocketAddr> {
    use mobarust_core::{AuthMethod, Protocol, SessionRecord};
    use std::os::unix::fs::OpenOptionsExt;
    let mut profiles = Vec::new();
    let mut addresses = Vec::new();
    for &(name, reply) in cases {
        let address = native_endpoint(directory, name, None, reply, lifetime, endpoints).await;
        addresses.push(address);
        let bytes = Zeroizing::new(std::fs::read(directory.join(format!("{name}.json"))).unwrap());
        let metadata: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let mut profile = SessionRecord::local_terminal(format!("SSH setup {name}"));
        profile.protocol = Protocol::Ssh;
        profile.hostname = address.ip().to_string();
        profile.port = address.port();
        profile.username = Some("fixture".into());
        profile.auth = AuthMethod::KeyboardInteractivePrompt;
        profile.pinned_fingerprint = Some(metadata["fingerprint"].as_str().unwrap().to_owned());
        profile.folder = Some("Disposable SSH setup lab".into());
        profile.tags = vec!["loopback".into()];
        profile.startup_command = Some("fixture-startup-".repeat(512));
        profile.notes = Some("Bounded echo fixture; no OS shell or command execution.".into());
        profile.validate().unwrap();
        profiles.push(profile);
    }
    let path = directory.join("profiles.json");
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    serde_json::to_writer(
        file,
        &serde_json::json!({"schema_version": 1, "sessions": profiles}),
    )
    .unwrap();
    eprintln!(
        "Native shell setup secret-free profiles: {}",
        path.display()
    );
    addresses
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
        ShellReply::Accept,
        Duration::from_secs(300),
        &mut endpoints,
    )
    .await;
    if jumps {
        let second = native_endpoint(
            directory.path(),
            "bastion2",
            Some(target),
            ShellReply::Accept,
            Duration::from_secs(300),
            &mut endpoints,
        )
        .await;
        native_endpoint(
            directory.path(),
            "bastion1",
            Some(second),
            ShellReply::Accept,
            Duration::from_secs(300),
            &mut endpoints,
        )
        .await;
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
    shell_reply: ShellReply,
    lifetime: Duration,
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
    let mut config = server::Config {
        keys: vec![key],
        auth_rejection_time: Duration::ZERO,
        inactivity_timeout: Some(Duration::from_secs(180)),
        nodelay: true,
        ..Default::default()
    };
    if matches!(
        shell_reply,
        ShellReply::StartupFlood | ShellReply::StartupReconnectStall
    ) {
        config.window_size = 1024;
    } else if matches!(shell_reply, ShellReply::StartupStall) {
        config.window_size = 0;
    }
    let config = Arc::new(config);
    let label = name.to_owned();
    let interrupt = directory.join("interrupt");
    endpoints.spawn(async move {
        let mut sessions = JoinSet::new();
        let mut accepted = 0;
        let lifetime = tokio::time::sleep(lifetime);
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
                    accepted += 1;
                    let reconnect_lab = matches!(shell_reply, ShellReply::StartupReconnectStall);
                    let reply = if reconnect_lab {
                        if accepted == 1 { ShellReply::StartupFlood } else { ShellReply::StartupStall }
                    } else { shell_reply };
                    if reconnect_lab {
                        eprintln!("Native reconnect {label}: accepted={accepted}, mode={reply:?}");
                    }
                    let observations = Arc::new(Observations::default());
                    let handler = Handler {
                        method: Method::Distinct { together: false },
                        expected: password.clone(), otp: otp.clone(),
                        observations: observations.clone(), release: None,
                        native_echo: forward_to.is_none(), forward_to,
                        shell_reply: reply,
                        forwarded: JoinSet::new(),
                        sftp_reply: None,
                        sftp_channels: std::collections::HashSet::new(),
                    };
                    let config = if reconnect_lab && accepted > 1 {
                        Arc::new(server::Config { keys: config.keys.clone(),
                            auth_rejection_time: Duration::ZERO,
                            inactivity_timeout: Some(Duration::from_secs(180)), nodelay: true,
                            window_size: 0, ..Default::default() })
                    } else { config.clone() };
                    let label = label.clone();
                    let interrupt = interrupt.clone();
                    let interrupt_first = reconnect_lab && accepted == 1;
                    sessions.spawn(async move {
                        let running = server::run_stream(config, stream, handler).await?;
                        let handle = running.handle();
                        tokio::pin!(running);
                        let result = tokio::select! {
                            result = &mut running => result,
                            _ = async {
                                while !std::fs::symlink_metadata(&interrupt).is_ok_and(|metadata|
                                    metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() == 0) {
                                    tokio::time::sleep(Duration::from_millis(50)).await;
                                }
                            }, if interrupt_first => {
                                handle.disconnect(russh::Disconnect::ByApplication,
                                    "disposable fixture interruption".into(), "en".into()).await?;
                                running.await
                            }
                        };
                        if !matches!(reply, ShellReply::Accept) {
                            // Report only counts and equality, never entered input or credentials.
                            let expected = format!("{}\n", "fixture-startup-".repeat(512));
                            eprintln!("Native shell setup {label}: shell_requests={}, input_bytes={}, startup_exact_once={}",
                                observations.shell_requests.load(Ordering::SeqCst),
                                observations.shell_input_bytes.load(Ordering::SeqCst),
                                *observations.startup_input.lock().unwrap() == expected.as_bytes());
                        }
                        result
                    });
                }
            }
        }
        sessions.abort_all();
        while sessions.join_next().await.is_some() {}
        drop(listener);
        drop(TcpListener::bind(address).await.expect("native lab must release its port"));
    });
    address
}

#[cfg(unix)]
fn native_fixture_options(metadata: &serde_json::Value) -> SshConnectOptions {
    let password = Zeroizing::new(metadata["password"].as_str().unwrap().to_owned());
    let otp = Zeroizing::new(metadata["otp"].as_str().unwrap().to_owned());
    SshConnectOptions {
        host: metadata["host"].as_str().unwrap().to_owned(),
        port: u16::try_from(metadata["port"].as_u64().unwrap()).unwrap(),
        host_key_policy: HostKeyPolicy::PinnedFingerprint(
            metadata["fingerprint"].as_str().unwrap().to_owned(),
        ),
        timeout: Duration::from_millis(500),
        keepalive_interval: None,
        credentials: SshCredentials::keyboard_interactive_prompt("fixture", move |challenge| {
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
        }),
        x11: None,
        environment: Vec::new(),
        startup_directory: None,
        startup_command: Some("fixture-startup-".repeat(512)),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn native_shell_setup_endpoints_are_isolated_and_cleanup() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let directory = native_lab_directory(root.path());
    let path = directory.path().to_owned();
    let mut endpoints = JoinSet::new();
    let addresses = native_shell_endpoints(
        directory.path(),
        &NATIVE_SHELL_CASES,
        Duration::from_secs(3),
        &mut endpoints,
    )
    .await;
    let profiles_path = directory.path().join("profiles.json");
    assert_eq!(
        profiles_path.metadata().unwrap().permissions().mode() & 0o777,
        0o600
    );
    let profiles = std::fs::read_to_string(profiles_path).unwrap();
    let export: serde_json::Value = serde_json::from_str(&profiles).unwrap();
    assert_eq!(export["schema_version"], 1);
    let sessions: Vec<mobarust_core::SessionRecord> =
        serde_json::from_value(export["sessions"].clone()).unwrap();
    assert_eq!(sessions.len(), NATIVE_SHELL_CASES.len());
    for (((name, reply), address), profile) in
        NATIVE_SHELL_CASES.into_iter().zip(&addresses).zip(sessions)
    {
        assert!(address.ip().is_loopback());
        profile.validate().unwrap();
        assert_eq!(
            profile.auth,
            mobarust_core::AuthMethod::KeyboardInteractivePrompt
        );
        assert_eq!(profile.port, address.port());
        assert_eq!(profile.hostname, address.ip().to_string());
        assert_eq!(
            profile.startup_command.as_deref(),
            Some("fixture-startup-".repeat(512).as_str())
        );
        let file = directory.path().join(format!("{name}.json"));
        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        let bytes = Zeroizing::new(std::fs::read(file).unwrap());
        let metadata: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            profile.pinned_fingerprint.as_deref(),
            metadata["fingerprint"].as_str()
        );
        for secret in ["password", "otp"] {
            assert!(
                !profiles.contains(metadata[secret].as_str().unwrap()),
                "generated factors must not enter session exports"
            );
        }
        let options = native_fixture_options(&metadata);
        let connection = SshConnection::connect(options).await.unwrap();
        let result = connection.open_shell(80, 24).await;
        match reply {
            ShellReply::StartupFlood => {
                let mut shell = result.expect("manual startup endpoint must drain output");
                let expected = [
                    vec![b'x'; 256 * 1024],
                    SHELL_BANNER.to_vec(),
                    format!("{}\n", "fixture-startup-".repeat(512)).into_bytes(),
                ]
                .concat();
                let received = tokio::time::timeout(DEADLINE, async {
                    let mut received = Vec::new();
                    while received.len() < expected.len() {
                        if let SshOutput::Stdout(bytes) | SshOutput::Stderr(bytes) =
                            shell.next_output().await.unwrap().unwrap()
                        {
                            received.extend(bytes);
                        }
                    }
                    received
                })
                .await
                .unwrap();
                assert_eq!(received, expected);
            }
            ShellReply::Reject => assert!(matches!(
                result,
                Err(SshError::ChannelRequestRejected { request: "shell" })
            )),
            ShellReply::StartupStall => {
                assert!(matches!(result, Err(SshError::StartupInputTimeout)))
            }
            _ => unreachable!(),
        }
        connection.disconnect().await.unwrap();
        drop(connection);
    }
    while let Some(result) = tokio::time::timeout(DEADLINE, endpoints.join_next())
        .await
        .unwrap()
    {
        result.unwrap();
    }
    for address in addresses {
        drop(TcpListener::bind(address).await.unwrap());
    }
    drop(directory);
    assert!(!path.exists(), "manual lab metadata must be removed");
}

#[cfg(unix)]
#[tokio::test]
async fn native_reconnect_endpoint_interrupts_once_then_stalls_startup() {
    use std::os::unix::fs::OpenOptionsExt;
    let root = tempfile::tempdir().unwrap();
    let directory = native_lab_directory(root.path());
    let path = directory.path().to_owned();
    let mut endpoints = JoinSet::new();
    let addresses = native_shell_endpoints(
        directory.path(),
        &[("reconnect-stalled", ShellReply::StartupReconnectStall)],
        Duration::from_secs(3),
        &mut endpoints,
    )
    .await;
    let bytes =
        Zeroizing::new(std::fs::read(directory.path().join("reconnect-stalled.json")).unwrap());
    let metadata: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let profiles = std::fs::read_to_string(directory.path().join("profiles.json")).unwrap();
    for factor in ["password", "otp"] {
        assert!(!profiles.contains(metadata[factor].as_str().unwrap()));
    }
    let connection = SshConnection::connect(native_fixture_options(&metadata))
        .await
        .unwrap();
    let mut shell = connection.open_shell(80, 24).await.unwrap();
    let expected = [
        vec![b'x'; 256 * 1024],
        SHELL_BANNER.to_vec(),
        format!("{}\n", "fixture-startup-".repeat(512)).into_bytes(),
    ]
    .concat();
    let received = tokio::time::timeout(DEADLINE, async {
        let mut received = Vec::new();
        while received.len() < expected.len() {
            if let SshOutput::Stdout(bytes) | SshOutput::Stderr(bytes) =
                shell.next_output().await.unwrap().unwrap()
            {
                received.extend(bytes);
            }
        }
        received
    })
    .await
    .unwrap();
    assert_eq!(received, expected);
    std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(directory.path().join("interrupt"))
        .unwrap();
    tokio::time::timeout(DEADLINE, async {
        while let Some(output) = shell.next_output().await {
            if output.is_err() {
                break;
            }
        }
    })
    .await
    .expect("controlled interruption must close the first transport");
    drop(shell);
    drop(connection);
    let replacement = SshConnection::connect(native_fixture_options(&metadata))
        .await
        .unwrap();
    assert!(matches!(
        replacement.open_shell(80, 24).await,
        Err(SshError::StartupInputTimeout)
    ));
    replacement.disconnect().await.unwrap();
    drop(replacement);
    while let Some(result) = tokio::time::timeout(DEADLINE, endpoints.join_next())
        .await
        .unwrap()
    {
        result.unwrap();
    }
    for address in addresses {
        drop(TcpListener::bind(address).await.unwrap());
    }
    drop(directory);
    assert!(
        !path.exists(),
        "private control and generated metadata must be removed"
    );
}

#[tokio::test]
async fn cancelling_pending_startup_releases_the_owned_transport() {
    let fixture = Fixture::start_with_shell(
        Method::Password,
        false,
        None,
        true,
        ShellReply::StartupStall,
    )
    .await;
    let mut options = fixture.options(Method::Password, true);
    options.startup_command = Some("must-not-replay".into());
    let connection = SshConnection::connect(options).await.unwrap();
    // Consume the authentication notification before waiting for shell setup.
    fixture.observations.entered.notified().await;
    let attempt = tokio::spawn(async move { connection.open_shell(80, 24).await });
    tokio::time::timeout(
        Duration::from_secs(1),
        fixture.observations.entered.notified(),
    )
    .await
    .expect("server accepts the shell and advertises zero input credit");
    assert!(!attempt.is_finished());
    attempt.abort();
    assert!(attempt.await.err().unwrap().is_cancelled());
    assert_eq!(
        fixture
            .observations
            .shell_input_bytes
            .load(Ordering::SeqCst),
        0
    );
    fixture.finish().await;
}

#[tokio::test]
async fn startup_input_keeps_shell_output_draining() {
    for split in [false, true] {
        let fixture = Fixture::start_with_shell(
            Method::Password,
            false,
            None,
            true,
            ShellReply::StartupFlood,
        )
        .await;
        let mut options = fixture.options(Method::Password, true);
        options.timeout = Duration::from_secs(1);
        let command = "fixture-startup-".repeat(512);
        let expected_input = format!("{command}\n").into_bytes();
        options.startup_command = Some(command);
        let connection = SshConnection::connect(options).await.unwrap();
        let mut shell = Some(
            connection
                .open_shell(80, 24)
                .await
                .expect("startup input must not deadlock the SSH actor behind unread output"),
        );
        let expected = [
            vec![b'x'; 256 * 1024],
            SHELL_BANNER.to_vec(),
            expected_input.clone(),
        ]
        .concat();
        let output = tokio::time::timeout(Duration::from_secs(1), async {
            let (mut reader, _writer) = if split {
                let (reader, writer) = shell.take().unwrap().split();
                (Some(reader), Some(writer))
            } else {
                (None, None)
            };
            let mut output = Vec::new();
            while output.len() < expected.len() {
                let next = if let Some(reader) = &mut reader {
                    reader.next_output().await
                } else {
                    shell.as_mut().unwrap().next_output().await
                };
                if let SshOutput::Stdout(bytes) | SshOutput::Stderr(bytes) = next.unwrap().unwrap()
                {
                    output.extend(bytes);
                }
            }
            output
        })
        .await
        .expect("startup output and echo delivery deadline");
        assert_eq!(
            output, expected,
            "buffered startup and live output retain exact order"
        );
        assert_eq!(
            *fixture.observations.startup_input.lock().unwrap(),
            expected_input,
            "partial startup input is sent once across all output events"
        );
        assert_eq!(
            fixture.observations.shell_requests.load(Ordering::SeqCst),
            1
        );
        connection.disconnect().await.unwrap();
        drop(connection);
        fixture.finish().await;
    }
    for reply in [
        ShellReply::StartupStall,
        ShellReply::StartupExit,
        ShellReply::StartupPartialExit,
        ShellReply::StartupOverflow,
    ] {
        let fixture = Fixture::start_with_shell(Method::Password, false, None, true, reply).await;
        let mut options = fixture.options(Method::Password, true);
        options.timeout = Duration::from_secs(1);
        let command = if matches!(reply, ShellReply::StartupPartialExit) {
            "must-not-replay".repeat(512)
        } else {
            "must-not-replay".into()
        };
        let expected_input = format!("{command}\n").into_bytes();
        options.startup_command = Some(command);
        let connection = SshConnection::connect(options).await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(3), connection.open_shell(80, 24))
            .await
            .unwrap();
        let message = result.as_ref().err().unwrap().to_string();
        if matches!(reply, ShellReply::StartupStall) {
            assert_eq!(
                message,
                "SSH startup input timed out; some input may have reached the server. Check the remote session and startup settings before reconnecting."
            );
        }
        assert!(
            !message.contains("must-not-replay"),
            "startup text stays private"
        );
        assert!(
            message.contains("Check the remote session and startup settings before reconnecting.")
        );
        assert!(
            match (reply, result) {
                (ShellReply::StartupStall, Err(SshError::StartupInputTimeout)) => true,
                (
                    ShellReply::StartupExit | ShellReply::StartupPartialExit,
                    Err(SshError::StartupInputFailed(source)),
                ) => matches!(
                    *source,
                    SshError::ChannelRequestClosed {
                        request: "startup input"
                    } | SshError::Channel(_)
                ),
                (ShellReply::StartupOverflow, Err(SshError::StartupInputFailed(source))) =>
                    matches!(*source, SshError::ShellSetupOutputTooLarge),
                _ => false,
            },
            "startup write must observe deadline, shell exit and output limits"
        );
        if !matches!(
            reply,
            ShellReply::StartupExit | ShellReply::StartupPartialExit
        ) {
            tokio::time::timeout(
                Duration::from_secs(1),
                fixture.observations.channel_closed.notified(),
            )
            .await
            .expect("failed startup closes its channel before disconnect");
        }
        let accepted = fixture
            .observations
            .shell_input_bytes
            .load(Ordering::SeqCst);
        if matches!(reply, ShellReply::StartupPartialExit) {
            assert!(
                accepted > 0 && accepted < expected_input.len(),
                "peer accepted a strict startup prefix"
            );
            assert_eq!(
                *fixture.observations.startup_input.lock().unwrap(),
                expected_input[..accepted]
            );
        } else {
            assert_eq!(accepted, 0);
        }
        assert_eq!(
            fixture.observations.shell_requests.load(Ordering::SeqCst),
            1
        );
        connection.disconnect().await.unwrap();
        drop(connection);
        fixture.finish().await;
    }
}

#[tokio::test]
async fn shell_setup_requires_server_acceptance() {
    for (reply, x11) in [
        (ShellReply::Reject, false),
        (ShellReply::Reject, true),
        (ShellReply::Silent, false),
        (ShellReply::Close, false),
        (ShellReply::Flood, false),
        (ShellReply::X11Reject, true),
    ] {
        let fixture = Fixture::start_with_shell(Method::Password, false, None, true, reply).await;
        let mut options = fixture.options(Method::Password, true);
        options.timeout = Duration::from_secs(1);
        options.startup_command = Some("must-not-execute".into());
        if x11 {
            options.x11 = Some(mobarust_ssh::X11ForwardingOptions {
                display: mobarust_ssh::X11Display::parse("tcp://127.0.0.1:6000").unwrap(),
                single_connection: false,
            });
        }
        let connection = SshConnection::connect(options).await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(3), connection.open_shell(80, 24))
            .await
            .unwrap();
        assert!(
            matches!(
                (reply, &result),
                (
                    ShellReply::Reject,
                    Err(SshError::ChannelRequestRejected { request: "shell" })
                ) | (
                    ShellReply::X11Reject,
                    Err(SshError::ChannelRequestRejected {
                        request: "X11 forwarding"
                    })
                ) | (ShellReply::Silent, Err(SshError::Timeout))
                    | (
                        ShellReply::Close,
                        Err(SshError::ChannelRequestClosed { request: "shell" })
                    )
                    | (ShellReply::Flood, Err(SshError::ShellSetupOutputTooLarge))
            ),
            "unexpected {reply:?} shell result"
        );
        if !matches!(reply, ShellReply::Close) {
            tokio::time::timeout(
                Duration::from_secs(1),
                fixture.observations.channel_closed.notified(),
            )
            .await
            .expect("failed setup sends channel close before transport teardown");
        }
        assert_eq!(
            fixture
                .observations
                .shell_input_bytes
                .load(Ordering::SeqCst),
            0,
            "startup commands must not reach an unaccepted shell"
        );
        assert_eq!(
            fixture.observations.shell_requests.load(Ordering::SeqCst),
            usize::from(!matches!(reply, ShellReply::X11Reject))
        );
        assert_eq!(
            fixture.observations.x11_requests.load(Ordering::SeqCst),
            usize::from(x11)
        );
        connection.disconnect().await.unwrap();
        drop(connection);
        fixture.finish().await;
    }
    for split in [false, true] {
        let fixture =
            Fixture::start_with_shell(Method::Password, false, None, true, ShellReply::Prelude)
                .await;
        let connection = SshConnection::connect(fixture.options(Method::Password, true))
            .await
            .unwrap();
        let mut shell = Some(connection.open_shell(80, 24).await.unwrap());
        let expected = [SHELL_PRELUDE, SHELL_STDERR, SHELL_BANNER].concat();
        let output = tokio::time::timeout(Duration::from_secs(1), async {
            let (mut reader, _writer) = if split {
                let (reader, writer) = shell.take().unwrap().split();
                (Some(reader), Some(writer))
            } else {
                (None, None)
            };
            let mut output = Vec::new();
            while output.len() < expected.len() {
                let next = if let Some(reader) = &mut reader {
                    reader.next_output().await
                } else {
                    shell.as_mut().unwrap().next_output().await
                };
                if let SshOutput::Stdout(bytes) | SshOutput::Stderr(bytes) = next.unwrap().unwrap()
                {
                    output.extend(bytes);
                }
            }
            output
        })
        .await
        .unwrap();
        assert_eq!(
            output, expected,
            "setup bytes precede live bytes exactly once"
        );
        connection.disconnect().await.unwrap();
        drop(connection);
        fixture.finish().await;
    }
}

#[tokio::test]
async fn sftp_setup_requires_server_acceptance_on_both_channels() {
    for listing in [false, true] {
        for reply in [
            SftpReply::Reject,
            SftpReply::Close,
            SftpReply::Silent,
            SftpReply::Cancel,
            SftpReply::Flood,
            SftpReply::OversizedPacket,
            SftpReply::Accept,
            SftpReply::Prelude,
        ] {
            let fixture = Fixture::start_with_replies(
                Method::Password,
                false,
                None,
                true,
                ShellReply::Accept,
                Some((reply, listing)),
            )
            .await;
            let connection = SshConnection::connect(fixture.options(Method::Password, true))
                .await
                .unwrap();
            let deadline = if matches!(reply, SftpReply::Silent) {
                Duration::from_secs(14)
            } else {
                Duration::from_secs(2)
            };
            let mut operation = Box::pin(async {
                let sftp = connection.open_sftp().await?;
                let result = if listing {
                    sftp.read_dir("/fixture").await.map(|_| ())
                } else {
                    Ok(())
                };
                sftp.close().await.unwrap();
                result
            });
            let result = if matches!(reply, SftpReply::Cancel) {
                tokio::time::timeout(Duration::from_secs(2), async {
                    while fixture.observations.sftp_requests.load(Ordering::SeqCst)
                        < 1 + usize::from(listing)
                    {
                        tokio::select! {
                            _ = &mut operation => panic!("silent subsystem setup must stay pending"),
                            _ = fixture.observations.sftp_entered.notified() => {},
                        }
                    }
                }).await.expect("reach the pending subsystem before cancelling");
                tokio::time::timeout(Duration::ZERO, operation).await
            } else {
                tokio::time::timeout(deadline, operation).await
            };
            let closed = if result.is_ok() || matches!(reply, SftpReply::Cancel) {
                tokio::time::timeout(Duration::from_secs(1), async {
                    // A server-initiated Close removes its channel before the
                    // client's acknowledgement can invoke channel_close here.
                    let expected =
                        1 + usize::from(listing) - usize::from(matches!(reply, SftpReply::Close));
                    while fixture.observations.closed_channels.load(Ordering::SeqCst) < expected {
                        fixture.observations.channel_closed.notified().await;
                    }
                })
                .await
                .is_ok()
            } else {
                false
            };
            let requests = fixture.observations.sftp_requests.load(Ordering::SeqCst);
            let input = fixture.observations.sftp_input_bytes.load(Ordering::SeqCst);
            if matches!(reply, SftpReply::Cancel | SftpReply::OversizedPacket) && closed {
                tokio::time::timeout(Duration::from_secs(1), async {
                    let (mut reader, writer) = connection.open_shell(80, 24).await.unwrap().split();
                    assert_eq!(
                        reader.next_output().await.unwrap().unwrap(),
                        SshOutput::Stdout(SHELL_BANNER.to_vec())
                    );
                    writer.write(b"transport still usable").await.unwrap();
                    assert_eq!(
                        reader.next_output().await.unwrap().unwrap(),
                        SshOutput::Stdout(b"transport still usable".to_vec())
                    );
                    drop(reader);
                    writer.close().await.unwrap();
                })
                .await
                .expect("failed SFTP setup must preserve the authenticated transport");
            }
            connection.disconnect().await.unwrap();
            drop(connection);
            fixture.finish().await;
            if matches!(reply, SftpReply::Cancel) {
                assert!(
                    result.is_err(),
                    "dropping setup before the request deadline"
                );
                assert!(
                    closed,
                    "cancelled SFTP setup closes its channel before disconnect"
                );
                assert_eq!(requests, 1 + usize::from(listing));
                assert_eq!(input, if listing { 9 } else { 0 });
                continue;
            }
            let result = result.expect("SFTP refusal/closure must not become a request timeout");
            assert!(
                matches!(
                    (reply, &result, listing),
                    (
                        SftpReply::Reject,
                        Err(SshError::ChannelRequestRejected {
                            request: "SFTP subsystem"
                        }),
                        _
                    ) | (
                        SftpReply::Close,
                        Err(SshError::ChannelRequestClosed {
                            request: "SFTP subsystem"
                        }),
                        _
                    ) | (SftpReply::Silent, Err(SshError::Timeout), _)
                        | (SftpReply::Flood, Err(SshError::SftpSetupOutputTooLarge), _)
                        | (SftpReply::OversizedPacket, Err(SshError::SftpProtocol), _)
                        | (SftpReply::Accept | SftpReply::Prelude, Ok(()), false)
                        | (
                            SftpReply::Accept | SftpReply::Prelude,
                            Err(SshError::SftpProtocol),
                            true
                        )
                ),
                "unexpected {reply:?}, listing={listing}: {result:?}"
            );
            assert!(
                closed,
                "{reply:?}, listing={listing}: failed or finished SFTP channels close before transport disconnect"
            );
            assert_eq!(requests, 1 + usize::from(listing));
            if matches!(reply, SftpReply::OversizedPacket) {
                assert_eq!(input, 9 * (1 + usize::from(listing)));
            } else if !matches!(reply, SftpReply::Accept | SftpReply::Prelude) {
                assert_eq!(
                    input,
                    if listing { 9 } else { 0 },
                    "no INIT goes to an unaccepted subsystem"
                );
            }
            if let Err(error) = result {
                assert!(!error.to_string().contains("fixture diagnostic"));
            }
        }
    }
}

#[tokio::test]
async fn shell_exit_status_waits_for_the_end_of_output() {
    for reply in [ShellReply::ExitBeforeOutput, ShellReply::EofBeforeExit] {
        let fixture = Fixture::start_with_shell(Method::Password, false, None, true, reply).await;
        let connection = SshConnection::connect(fixture.options(Method::Password, true))
            .await
            .unwrap();
        let (mut reader, writer) = connection.open_shell(80, 24).await.unwrap().split();
        let output = tokio::time::timeout(DEADLINE, async {
            let mut output = Vec::new();
            loop {
                match reader
                    .next_output()
                    .await
                    .expect("exit status must be delivered")
                    .unwrap()
                {
                    SshOutput::Stdout(bytes) | SshOutput::Stderr(bytes) => output.extend(bytes),
                    SshOutput::ExitStatus(status) => {
                        assert_eq!(status, 23);
                        break;
                    }
                    SshOutput::Control => {}
                }
            }
            output
        })
        .await
        .unwrap();
        let expected = if matches!(reply, ShellReply::ExitBeforeOutput) {
            [
                SHELL_BANNER,
                b"output after exit status\r\n",
                b"stderr after exit status\r\n",
            ]
            .concat()
        } else {
            SHELL_BANNER.to_vec()
        };
        assert_eq!(output, expected);
        assert!(reader.next_output().await.is_none());
        drop(reader);
        drop(writer);
        connection.disconnect().await.unwrap();
        drop(connection);
        fixture.finish().await;
    }
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
