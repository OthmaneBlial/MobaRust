#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use mobarust_ssh::{
    HostKeyPolicy, RemoteTextEncoding, SftpConnection, SshConnectOptions, SshConnection,
    SshCredentials, SshError, SshFingerprintOptions, SshOutput, X11Display, X11ForwardingOptions,
    inspect_host_key,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream, UnixStream};
use tokio::sync::oneshot;

async fn shell_output(connection: &SshConnection, command: &[u8]) -> String {
    let shell = connection
        .open_shell(80, 24)
        .await
        .expect("open fixture PTY");
    let (mut reader, writer) = shell.split();
    writer.write(command).await.expect("send fixture command");
    let mut output = Vec::new();
    let mut exit_status = None;
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(message) = reader.next_output().await {
            match message.expect("read fixture PTY") {
                SshOutput::Stdout(bytes) | SshOutput::Stderr(bytes) => {
                    output.extend(bytes);
                    assert!(
                        output.len() <= 64 * 1024,
                        "fixture output exceeded its bound"
                    );
                }
                SshOutput::ExitStatus(status) => {
                    exit_status = Some(status);
                    break;
                }
                SshOutput::Control => {}
            }
        }
    })
    .await
    .expect("fixture command deadline");
    assert_eq!(exit_status, Some(0), "fixture command must actually finish");
    String::from_utf8(output).expect("fixture UTF-8 output")
}

#[test]
fn fixture_shell_uses_only_the_disposable_home() {
    let runtime = tokio::runtime::Runtime::new().expect("create SSH test runtime");
    runtime.block_on(async {
        let fixture = LocalSshd::start().expect("start local sshd fixture");
        wait_for_port(fixture.port).await;
        let connection = SshConnection::connect(fixture.options())
            .await
            .expect("connect fixture");
        let output = shell_output(
            &connection,
            b"printf '\\nMOBARUST_HOME=%s\\nMOBARUST_ZDOTDIR=%s\\n' \"$HOME\" \"$ZDOTDIR\"; exit\n",
        )
        .await;
        let home = fixture.directory.path().to_string_lossy();
        assert!(output.contains(&format!("MOBARUST_HOME={home}\r\n")));
        assert!(output.contains(&format!("MOBARUST_ZDOTDIR={home}\r\n")));
        connection.disconnect().await.expect("disconnect fixture");
    });
}

#[test]
fn ipv6_loopback_verifies_known_hosts_and_runs_a_shell() {
    if std::net::TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, 0)).is_err() {
        eprintln!("skipping IPv6 OpenSSH fixture: IPv6 loopback is unavailable");
        return;
    }
    let runtime = tokio::runtime::Runtime::new().expect("create IPv6 test runtime");
    runtime.block_on(async {
        let fixture = LocalSshd::start_internal(false, true).expect("start IPv6 SSH fixture");
        wait_for_port(fixture.port).await;
        let trust = fixture.directory.path().join("known_hosts_ipv6");
        fs::write(
            &trust,
            fs::read_to_string(&fixture.known_hosts)
                .unwrap()
                .replace("[127.0.0.1]", "[::1]"),
        )
        .expect("write IPv6 fixture trust file");
        let mut options = fixture.options();
        options.host = "::1".into();
        options.host_key_policy = HostKeyPolicy::KnownHosts(trust);
        let connection = SshConnection::connect(options)
            .await
            .expect("connect over IPv6");
        let output = shell_output(&connection, b"printf 'MOBARUST_%s\\n' 'IPV6_OK'; exit\n").await;
        assert!(output.contains("MOBARUST_IPV6_OK"));
        connection
            .disconnect()
            .await
            .expect("disconnect IPv6 fixture");
    });
}

#[test]
fn stalled_handshakes_timeout_or_cancel_and_release_the_socket() {
    let runtime = tokio::runtime::Runtime::new().expect("create SSH test runtime");
    runtime.block_on(async {
        let fixture = LocalSshd::start().expect("start local sshd fixture");
        for cancel in [false, true] {
            let listener = TcpListener::bind(("127.0.0.1", 0))
                .await
                .expect("bind stalled SSH fixture");
            let mut options = fixture.options();
            options.port = listener
                .local_addr()
                .expect("stalled fixture address")
                .port();
            options.timeout = if cancel {
                Duration::from_secs(5)
            } else {
                Duration::from_millis(250)
            };
            let mut connecting = Box::pin(SshConnection::connect(options));
            let (mut peer, _) = tokio::select! {
                accepted = listener.accept() => accepted.expect("accept stalled handshake"),
                _ = &mut connecting => panic!("SSH setup ended before fixture accepted"),
            };
            let mut banner = [0; 256];
            tokio::time::timeout(Duration::from_secs(1), async {
                tokio::select! {
                    read = peer.read(&mut banner) => assert!(read.expect("read client banner") > 0),
                    _ = &mut connecting => panic!("SSH setup ended before fixture received banner"),
                }
            })
            .await
            .expect("client banner deadline");
            if !cancel {
                assert!(matches!(connecting.await, Err(SshError::Timeout)));
            } else {
                // This is how the native cancellation select releases an in-flight setup.
                drop(connecting);
            }
            let mut remaining = Vec::new();
            tokio::time::timeout(
                Duration::from_secs(1),
                peer.take(4096).read_to_end(&mut remaining),
            )
            .await
            .expect("cancelled/timed-out handshake must close its socket")
            .expect("read handshake EOF");
            assert!(remaining.len() < 4096);
        }
    });
}

#[test]
fn interrupted_transport_recovers_after_the_fixture_server_restarts() {
    let runtime = tokio::runtime::Runtime::new().expect("create SSH restart runtime");
    runtime.block_on(async {
        let mut fixture = LocalSshd::start().expect("start restart fixture");
        wait_for_port(fixture.port).await;
        let original_trust = fs::read(&fixture.known_hosts).expect("read original trust");

        // Own both sockets so dropping the bridge interrupts an established
        // SSH session without killing any process outside this fixture.
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let bridge_port = listener.local_addr().unwrap().port();
        let bridge_trust = fixture.directory.path().join("bridge_known_hosts");
        fs::write(
            &bridge_trust,
            String::from_utf8(original_trust.clone()).unwrap().replace(
                &format!("[127.0.0.1]:{}", fixture.port),
                &format!("[127.0.0.1]:{bridge_port}"),
            ),
        )
        .unwrap();
        let server_port = fixture.port;
        let (cut_tx, cut_rx) = oneshot::channel();
        let bridge = tokio::spawn(async move {
            let (mut client, _) = listener.accept().await.unwrap();
            let mut server = TcpStream::connect(("127.0.0.1", server_port))
                .await
                .unwrap();
            tokio::select! {
                _ = cut_rx => {},
                _ = copy_bidirectional(&mut client, &mut server) => {
                    panic!("fixture transport ended before the requested interruption");
                }
            }
        });
        let mut options = fixture.options();
        options.port = bridge_port;
        options.host_key_policy = HostKeyPolicy::KnownHosts(bridge_trust);
        let connection = SshConnection::connect(options).await.unwrap();
        let (mut reader, writer) = connection.open_shell(80, 24).await.unwrap().split();
        cut_tx
            .send(())
            .expect("interrupt established fixture transport");
        tokio::time::timeout(Duration::from_secs(5), bridge)
            .await
            .expect("transport bridge cleanup deadline")
            .expect("join interrupted transport bridge");
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(message) = reader.next_output().await {
                match message {
                    Err(_) => break,
                    Ok(SshOutput::ExitStatus(_)) => {
                        panic!("transport interruption must not look like a normal shell exit");
                    }
                    Ok(_) => {}
                }
            }
        })
        .await
        .expect("lost shell must stop producing output");
        drop(writer);
        drop(reader);
        let _ = tokio::time::timeout(Duration::from_secs(5), connection.disconnect())
            .await
            .expect("lost connection cleanup deadline");
        drop(connection);

        fixture.child.kill().expect("stop only the fixture daemon");
        fixture.child.wait().expect("reap stopped fixture daemon");
        assert!(matches!(
            SshConnection::connect(fixture.options()).await,
            Err(SshError::ConnectionRefused)
        ));
        fixture.child = spawn_sshd(&fixture.directory.path().join("sshd_config"))
            .expect("restart with the same generated keys and configuration");
        wait_for_port(fixture.port).await;
        let recovered = SshConnection::connect(fixture.options())
            .await
            .expect("reconnect after server restart");
        let output =
            shell_output(&recovered, b"printf 'MOBARUST_%s\\n' 'RESTART_OK'; exit\n").await;
        assert!(output.contains("MOBARUST_RESTART_OK"));
        assert_eq!(fs::read(&fixture.known_hosts).unwrap(), original_trust);
        recovered
            .disconnect()
            .await
            .expect("disconnect recovered session");
    });
}

#[test]
fn dedicated_agent_rejects_empty_and_wrong_keys_then_authenticates() {
    const CHILD_SOCKET: &str = "MOBARUST_TEST_AGENT_SOCKET";
    if let Some(socket) = std::env::var_os(CHILD_SOCKET) {
        // Only the subprocess receives this socket. Never mutate the shared
        // test process environment or consult the operator's agent.
        let socket = PathBuf::from(socket);
        assert_eq!(socket.file_name().unwrap(), "agent.sock");
        assert!(
            socket
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("mobarust-agent-")
        );
        assert_eq!(
            std::env::var_os("SSH_AUTH_SOCK"),
            Some(socket.clone().into())
        );
        let runtime = tokio::runtime::Runtime::new().expect("create agent test runtime");
        runtime.block_on(async {
            for _ in 0..100 {
                if socket.exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            assert!(socket.exists(), "dedicated agent must create its socket");
            let fixture = LocalSshd::start().expect("start agent SSH fixture");
            wait_for_port(fixture.port).await;
            let options = || {
                let mut options = fixture.options();
                options.credentials = SshCredentials::agent(fixture.username.clone());
                options
            };
            let wrong_key = fixture.directory.path().join("agent-wrong-key");
            run_keygen(&wrong_key).expect("generate unauthorized agent key");
            for key in [&wrong_key, &fixture.client_key] {
                assert!(matches!(
                    SshConnection::connect(options()).await,
                    Err(SshError::AuthenticationRejected)
                ));
                let mut add = Command::new("ssh-add");
                clear_credential_environment(&mut add);
                assert!(
                    add.env("SSH_AUTH_SOCK", &socket)
                        .arg(key)
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .expect("load generated agent key")
                        .success()
                );
            }
            let connection = SshConnection::connect(options())
                .await
                .expect("authenticate with agent");
            let output =
                shell_output(&connection, b"printf 'MOBARUST_%s\\n' 'AGENT_OK'; exit\n").await;
            assert!(output.contains("MOBARUST_AGENT_OK"));
            connection
                .disconnect()
                .await
                .expect("disconnect agent fixture");
        });
        return;
    }

    let executable = std::env::current_exe().expect("locate SSH test executable");
    let directory = tempfile::Builder::new()
        .prefix("mobarust-agent-")
        .tempdir()
        .expect("create disposable agent directory");
    let socket = directory.path().join("agent.sock");
    let mut command = Command::new("ssh-agent");
    clear_credential_environment(&mut command);
    let mut agent = command
        .args(["-D", "-a"])
        .arg(&socket)
        .env("HOME", directory.path())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start dedicated foreground SSH agent");
    let mut child = Command::new(executable);
    clear_credential_environment(&mut child);
    let status = child
        .args([
            "--exact",
            "dedicated_agent_rejects_empty_and_wrong_keys_then_authenticates",
            "--nocapture",
        ])
        .env(CHILD_SOCKET, &socket)
        .env("SSH_AUTH_SOCK", &socket)
        .status();
    // Reap our own foreground agent even when the child test fails.
    let _ = agent.kill();
    let _ = agent.wait();
    assert!(status.expect("run isolated agent test").success());
}

#[test]
fn encrypted_keys_authenticate_and_wrong_credentials_fail_closed() {
    let runtime = tokio::runtime::Runtime::new().expect("create SSH test runtime");
    runtime.block_on(async {
        let fixture = LocalSshd::start().expect("start local sshd fixture");
        wait_for_port(fixture.port).await;
        let encrypted_key = fixture.directory.path().join("encrypted_key");
        fs::copy(&fixture.client_key, &encrypted_key).expect("copy fixture key");
        // Public test data, never an operator credential. One bcrypt round
        // keeps this interoperability fixture independent of CPU contention.
        let passphrase = "mobarust-disposable-fixture";
        let mut keygen = Command::new("ssh-keygen");
        clear_credential_environment(&mut keygen);
        assert!(
            keygen
                .args(["-q", "-p", "-a", "1", "-P", "", "-N", passphrase, "-f"])
                .arg(&encrypted_key)
                .stdout(Stdio::null())
                .status()
                .expect("encrypt fixture key")
                .success()
        );

        let mut options = fixture.options();
        options.credentials = SshCredentials::private_key(
            fixture.username.clone(),
            encrypted_key.clone(),
            Some(passphrase),
        );
        let connection = SshConnection::connect(options)
            .await
            .expect("authenticate encrypted key");
        let output = shell_output(
            &connection,
            b"printf 'MOBARUST_%s\\n' 'ENCRYPTED_OK'; exit\n",
        )
        .await;
        assert!(output.contains("MOBARUST_ENCRYPTED_OK"));
        connection
            .disconnect()
            .await
            .expect("disconnect encrypted-key fixture");

        for passphrase in [None, Some("incorrect-fixture-passphrase")] {
            let mut options = fixture.options();
            options.credentials = SshCredentials::private_key(
                fixture.username.clone(),
                encrypted_key.clone(),
                passphrase,
            );
            assert!(matches!(
                SshConnection::connect(options).await,
                Err(SshError::PrivateKey(_))
            ));
        }
        let unauthorized_key = fixture.directory.path().join("unauthorized_key");
        run_keygen(&unauthorized_key).expect("generate unauthorized fixture key");
        let mut options = fixture.options();
        options.credentials =
            SshCredentials::private_key(fixture.username.clone(), unauthorized_key, None::<String>);
        assert!(matches!(
            SshConnection::connect(options).await,
            Err(SshError::AuthenticationRejected)
        ));
        // A rejected attempt must not prevent a subsequent valid connection.
        let recovered = SshConnection::connect(fixture.options())
            .await
            .expect("recover after auth failure");
        recovered
            .disconnect()
            .await
            .expect("disconnect recovered fixture");
    });
}

#[test]
fn distinct_jump_hosts_verify_every_key_and_reach_the_target() {
    let runtime = tokio::runtime::Runtime::new().expect("create SSH test runtime");
    runtime.block_on(async {
        let first = LocalSshd::start().expect("start first jump fixture");
        let second = LocalSshd::start().expect("start second jump fixture");
        let target = LocalSshd::start().expect("start target fixture");
        let fixtures = [&first, &second, &target];
        let mut fingerprints = Vec::new();
        for fixture in fixtures {
            wait_for_port(fixture.port).await;
            fingerprints.push(
                inspect_host_key(SshFingerprintOptions {
                    host: "127.0.0.1".into(),
                    port: fixture.port,
                    timeout: Duration::from_secs(5),
                })
                .await
                .expect("inspect fixture host key")
                .fingerprint,
            );
        }
        assert!(
            fingerprints
                .iter()
                .enumerate()
                .all(|(i, key)| !fingerprints[..i].contains(key))
        );
        for rejected_hop in 0..3 {
            let mut options = fixtures.map(LocalSshd::options);
            options[rejected_hop].host_key_policy =
                HostKeyPolicy::PinnedFingerprint("SHA256:untrusted-fixture".into());
            let [first, second, target] = options;
            match SshConnection::connect_with_jump_chain(target, vec![first, second]).await {
                Err(SshError::HostKeyRejected { fingerprint }) => {
                    assert_eq!(fingerprint, fingerprints[rejected_hop])
                }
                _ => panic!("jump chain accepted an untrusted hop"),
            }
        }

        let wrong_hosts = target.directory.path().join("changed_known_hosts");
        let first_key = fs::read_to_string(&first.known_hosts).expect("read fixture public key");
        let (_, key) = first_key
            .split_once(' ')
            .expect("fixture known_hosts record");
        fs::write(&wrong_hosts, format!("[127.0.0.1]:{} {key}", target.port))
            .expect("write changed host key fixture");
        let mut options = target.options();
        options.host_key_policy = HostKeyPolicy::KnownHosts(wrong_hosts.clone());
        assert!(matches!(
            SshConnection::connect(options).await,
            Err(SshError::HostKeyRejected { .. })
        ));
        assert_eq!(
            fs::read_to_string(&wrong_hosts).expect("read unchanged trust file"),
            format!("[127.0.0.1]:{} {key}", target.port)
        );

        let mut invalid_trust = target.options();
        invalid_trust.host_key_policy = HostKeyPolicy::KnownHosts(target.directory.path().into());
        assert!(matches!(
            SshConnection::connect(invalid_trust).await,
            Err(SshError::KnownHosts(_))
        ));

        let connection = SshConnection::connect_with_jump_chain(
            target.options(),
            vec![first.options(), second.options()],
        )
        .await
        .expect("connect through two distinct jump hosts");
        let output = shell_output(
            &connection,
            b"printf 'MOBARUST_%s\\n' 'MULTIHOP_OK'; exit\n",
        )
        .await;
        assert!(output.contains("MOBARUST_MULTIHOP_OK"));
        let sftp = connection
            .open_sftp()
            .await
            .expect("open target SFTP through two jumps");
        let remote_path = target.directory.path().join("café target file.txt");
        let remote_path = remote_path.to_string_lossy().into_owned();
        let payload = "UTF-8 fixture: café 🦀\n".as_bytes();
        assert_eq!(
            sftp.upload_from(payload, &remote_path)
                .await
                .expect("upload through jump chain"),
            payload.len() as u64
        );
        let mut downloaded = Vec::new();
        sftp.download_to(&remote_path, &mut downloaded)
            .await
            .expect("download through jump chain");
        assert_eq!(downloaded, payload);
        assert_eq!(
            fs::read(&remote_path).expect("verify target fixture file"),
            payload
        );
        sftp.close().await.expect("close jumped SFTP");
        connection
            .disconnect()
            .await
            .expect("disconnect complete jump chain");
    });
}

#[test]
fn idle_shell_survives_the_connection_timeout_without_keepalives() {
    let runtime = tokio::runtime::Runtime::new().expect("create SSH test runtime");
    runtime.block_on(async {
        let fixture = LocalSshd::start().expect("start local sshd fixture");
        wait_for_port(fixture.port).await;
        let mut options = fixture.options();
        options.timeout = Duration::from_secs(2);
        let connection = SshConnection::connect(options)
            .await
            .expect("connect fixture");
        let sftp = connection.open_sftp().await.expect("open SFTP before idle");
        sftp.read_dir(fixture.directory.path().to_string_lossy().into_owned())
            .await
            .expect("finish initial channel traffic");

        tokio::time::sleep(Duration::from_secs(3)).await;
        let output = shell_output(&connection, b"printf 'MOBARUST_%s\\n' 'IDLE_OK'; exit\n").await;
        // The marker is assembled by printf, so terminal echo cannot pass this assertion.
        assert!(output.contains("MOBARUST_IDLE_OK"));
        sftp.close().await.expect("close idle SFTP");
        connection.disconnect().await.expect("disconnect fixture");
    });
}

#[test]
fn connects_to_a_reproducible_local_sshd_fixture_with_a_real_pty_shell() {
    let runtime = tokio::runtime::Runtime::new().expect("create SSH test runtime");
    runtime.block_on(async {
        let fixture = LocalSshd::start().expect("start local sshd fixture");
        wait_for_port(fixture.port).await;

        let unknown_hosts = fixture.directory.path().join("unknown_hosts");
        fs::write(&unknown_hosts, "").expect("create empty known_hosts file");
        let rejection = SshConnection::connect(SshConnectOptions {
            host: "127.0.0.1".into(),
            port: fixture.port,
            host_key_policy: HostKeyPolicy::KnownHosts(unknown_hosts),
            timeout: Duration::from_secs(5),
            keepalive_interval: None,
            credentials: SshCredentials::private_key(
                fixture.username.clone(),
                fixture.client_key.clone(),
                None::<String>,
            ),
            x11: None,
            environment: Vec::new(),
            startup_directory: None,
            startup_command: None,
        })
        .await;
        let rejected_fingerprint = match rejection {
            Err(SshError::HostKeyRejected { fingerprint }) => {
                assert!(fingerprint.starts_with("SHA256:"));
                fingerprint
            }
            _ => panic!("unknown host key was not rejected"),
        };

        let inspection = inspect_host_key(SshFingerprintOptions {
            host: "127.0.0.1".into(),
            port: fixture.port,
            timeout: Duration::from_secs(5),
        })
        .await
        .expect("inspect the local fixture host key without authentication");
        assert_eq!(inspection.fingerprint, rejected_fingerprint);
        assert_eq!(inspection.host, "127.0.0.1");
        assert_eq!(inspection.port, fixture.port);

        let connection = SshConnection::connect(SshConnectOptions {
            host: "127.0.0.1".into(),
            port: fixture.port,
            host_key_policy: HostKeyPolicy::KnownHosts(fixture.known_hosts.clone()),
            timeout: Duration::from_secs(5),
            keepalive_interval: None,
            credentials: SshCredentials::private_key(
                fixture.username.clone(),
                fixture.client_key.clone(),
                None::<String>,
            ),
            x11: None,
            environment: vec![("MOBARUST_FIXTURE".into(), "loopback-only".into())],
            startup_directory: Some("/tmp".into()),
            startup_command: Some("printf 'MOBARUST_%s\\n' 'STARTUP_OK'".into()),
        })
        .await
        .expect("connect to local sshd");

        assert_eq!(
            connection.state(),
            mobarust_core::ConnectionState::Connected
        );
        let monitor = connection
            .remote_monitor_snapshot()
            .await
            .expect("collect read-only remote monitor snapshot");
        assert!(monitor.hostname.is_some());
        assert!(monitor.kernel.is_some());
        assert!(!monitor.supported_metrics.is_empty());
        let shell = connection.open_shell(100, 30).await.expect("open SSH PTY");
        let (mut reader, writer) = shell.split();
        writer.resize(120, 40).await.expect("resize SSH PTY");
        writer
            .write(b"printf 'MOBARUST_%s\\n' 'SSH_OK'; printf 'MOBARUST_ENV_%s\\n' \"$MOBARUST_FIXTURE\"; exit\n")
            .await
            .expect("write shell command");

        let mut output = Vec::new();
        while let Some(message) = tokio::time::timeout(Duration::from_secs(5), reader.next_output())
            .await
            .expect("SSH shell output timeout")
        {
            let message = message.expect("SSH shell output error");
            match message {
                SshOutput::Stdout(bytes) | SshOutput::Stderr(bytes) => output.extend(bytes),
                SshOutput::ExitStatus(status) => {
                    assert_eq!(status, 0);
                    break;
                }
                SshOutput::Control => {}
            }
        }

        assert!(String::from_utf8_lossy(&output).contains("MOBARUST_SSH_OK"));
        assert!(String::from_utf8_lossy(&output).contains("MOBARUST_ENV_loopback-only"));
        assert!(String::from_utf8_lossy(&output).contains("MOBARUST_STARTUP_OK"));
        let source = fixture.directory.path().join("source.bin");
        let downloaded = fixture.directory.path().join("downloaded.bin");
        let remote_root = fixture.directory.path().to_string_lossy().into_owned();
        fs::write(&source, vec![b'R'; 128 * 1024]).expect("write upload source");
        let sftp = connection.open_sftp().await.expect("open SFTP subsystem");
        let remote_path = fixture
            .directory
            .path()
            .join(format!("mobarust-sftp-{} ", std::process::id()))
            .to_string_lossy()
            .into_owned();
        let uploaded = sftp
            .upload_from(
                tokio::fs::File::open(&source)
                    .await
                    .expect("open upload source"),
                &remote_path,
            )
            .await
            .expect("stream upload through SFTP");
        assert_eq!(uploaded, 128 * 1024);
        let entries = sftp
            .read_dir(&remote_root)
            .await
            .expect("list remote fixture directory");
        let uploaded_entry = entries
            .iter()
            .find(|entry| entry.path == remote_path)
            .expect("find uploaded remote file");
        assert_eq!(uploaded_entry.size, Some(128 * 1024));
        assert!(!uploaded_entry.is_directory);
        assert!(uploaded_entry.is_regular);
        assert!(uploaded_entry.permissions.is_some());
        assert!(uploaded_entry.uid.is_some() || uploaded_entry.owner.is_some());
        assert!(uploaded_entry.gid.is_some() || uploaded_entry.group.is_some());
        sftp.set_permissions(&remote_path, 0o640)
            .await
            .expect("change remote fixture permissions");
        let updated_entries = sftp
            .read_dir(&remote_root)
            .await
            .expect("list remote fixture directory after chmod");
        let updated_entry = updated_entries
            .iter()
            .find(|entry| entry.path == remote_path)
            .expect("find chmod fixture file");
        assert_eq!(
            updated_entry.permissions.map(|mode| mode & 0o7777),
            Some(0o640)
        );
        let cleanup_directory = fixture
            .directory
            .path()
            .join(format!("mobarust-upload-cleanup-{}", std::process::id()));
        fs::create_dir(&cleanup_directory).expect("create upload cleanup fixture directory");
        let cleanup_temporary = cleanup_directory.join(".partial.part");
        let cleanup_destination = cleanup_directory.join("existing.bin");
        fs::create_dir(&cleanup_temporary).expect("create unremovable temporary fixture");
        fs::write(&cleanup_destination, b"existing destination")
            .expect("create existing upload destination");
        let cleanup_temporary = cleanup_temporary.to_string_lossy().into_owned();
        let cleanup_destination = cleanup_destination.to_string_lossy().into_owned();
        assert!(matches!(
            sftp.promote_uploaded_file(&cleanup_temporary, &cleanup_destination, false)
                .await,
            Err(SshError::RemoteTemporaryCleanupFailed)
        ));
        assert!(Path::new(&cleanup_temporary).is_dir());
        fs::remove_dir_all(&cleanup_directory).expect("remove upload cleanup fixture directory");
        assert!(
            sftp.try_exists(&remote_path)
                .await
                .expect("check remote file")
        );
        assert_eq!(
            sftp.file_info(&remote_path)
                .await
                .expect("read remote file info"),
            (Some(128 * 1024), false)
        );

        let cancelled_destination = fixture.directory.path().join("cancelled.bin");
        let mut cancelled_file = tokio::fs::File::create(&cancelled_destination)
            .await
            .expect("create cancelled download destination");
        let (cancel_sender, mut cancel_receiver) = oneshot::channel();
        let mut cancel_sender = Some(cancel_sender);
        let mut last_progress = 0_u64;
        let cancellation = sftp
            .download_to_with_cancel(
                &remote_path,
                &mut cancelled_file,
                &mut cancel_receiver,
                |bytes| {
                    last_progress = bytes;
                    if let Some(sender) = cancel_sender.take() {
                        let _ = sender.send(());
                    }
                },
            )
            .await;
        assert!(matches!(cancellation, Err(SshError::Cancelled)));
        assert!(last_progress > 0 && last_progress < 128 * 1024);
        drop(cancelled_file);
        assert!(
            fs::metadata(&cancelled_destination)
                .expect("inspect cancelled destination")
                .len()
                < 128 * 1024
        );

        let downloaded_bytes = sftp
            .download_to(
                &remote_path,
                tokio::fs::File::create(&downloaded)
                    .await
                    .expect("create download destination"),
            )
            .await
            .expect("stream download through SFTP");
        assert_eq!(downloaded_bytes, 128 * 1024);
        assert_eq!(
            fs::read(&downloaded).expect("read downloaded fixture"),
            vec![b'R'; 128 * 1024]
        );
        let renamed_path = format!("{remote_path}.renamed");
        sftp.rename(&remote_path, &renamed_path)
            .await
            .expect("rename remote fixture file");
        assert!(!sftp.try_exists(&remote_path).await.expect("check old name"));
        assert!(
            sftp.try_exists(&renamed_path)
                .await
                .expect("check new name")
        );
        assert!(
            sftp.promote_uploaded_file(&renamed_path, &renamed_path, false)
                .await
                .is_err()
        );
        assert_eq!(
            fs::read(&renamed_path).expect("read original after invalid promotion"),
            vec![b'R'; 128 * 1024]
        );
        let replacement_source = fixture.directory.path().join("replacement.bin");
        fs::write(&replacement_source, b"replacement").expect("write replacement source");
        let temporary_upload = format!("{renamed_path}.mobarust-upload-part");
        sftp.upload_from(
            tokio::fs::File::open(&replacement_source)
                .await
                .expect("open replacement source"),
            &temporary_upload,
        )
        .await
        .expect("upload complete temporary file");
        assert!(
            sftp.rename(&temporary_upload, &renamed_path).await.is_err(),
            "standard SFTP rename must refuse an existing destination"
        );
        assert!(
            sftp.promote_uploaded_file(&temporary_upload, &renamed_path, false)
                .await
                .is_err()
        );
        assert_eq!(fs::read(&renamed_path).expect("read original remote file"), vec![b'R'; 128 * 1024]);
        assert!(!sftp.try_exists(&temporary_upload).await.expect("check rejected upload cleanup"));
        sftp.upload_from(
            tokio::fs::File::open(&replacement_source)
                .await
                .expect("open replacement source"),
            &temporary_upload,
        )
        .await
        .expect("upload replacement temporary file");
        sftp.promote_uploaded_file(&temporary_upload, &renamed_path, true)
            .await
            .expect("replace existing remote file through SFTP v3");
        assert_eq!(fs::read(&renamed_path).expect("read replaced remote file"), b"replacement");
        let replaced_mode = sftp
            .read_dir(&remote_root)
            .await
            .expect("list replaced remote file")
            .into_iter()
            .find(|entry| entry.path == renamed_path)
            .expect("find replaced remote file")
            .permissions
            .map(|mode| mode & 0o7777);
        assert_eq!(replaced_mode, Some(0o640));
        assert!(!sftp.try_exists(&temporary_upload).await.expect("check promoted upload cleanup"));
        assert!(
            !sftp.read_dir(&remote_root)
                .await
                .expect("list remote directory after replacement")
                .iter()
                .any(|entry| entry.path.starts_with(&format!("{renamed_path}.mobarust-upload-backup-")))
        );
        let directory_path = fixture
            .directory
            .path()
            .join(format!("mobarust-directory-{}", std::process::id()))
            .to_string_lossy()
            .into_owned();
        sftp.create_dir(&directory_path)
            .await
            .expect("create remote directory");
        assert!(
            sftp.file_info(&directory_path)
                .await
                .expect("read directory info")
                .1
        );
        assert!(
            sftp.is_real_directory(&directory_path)
                .await
                .expect("check real remote directory")
        );
        let directory_link = format!("{directory_path}-link");
        std::os::unix::fs::symlink(&directory_path, &directory_link)
            .expect("create remote directory symlink fixture");
        assert!(
            !sftp.is_real_directory(&directory_link)
                .await
                .expect("reject remote directory symlink")
        );
        fs::remove_file(&directory_link).expect("remove remote directory symlink fixture");
        sftp.remove_dir(&directory_path)
            .await
            .expect("remove remote directory");
        sftp.remove_file(&renamed_path)
            .await
            .expect("remove fixture file");
        assert!(!sftp.try_exists(&renamed_path).await.expect("check removal"));

        let editor_source = fixture.directory.path().join("editor-source.txt");
        fs::write(&editor_source, "before\neditor fixture\n").expect("write editor source");
        let editor_path = fixture
            .directory
            .path()
            .join(format!("mobarust-editor-{}.txt", std::process::id()))
            .to_string_lossy()
            .into_owned();
        sftp.upload_from(
            tokio::fs::File::open(&editor_source)
                .await
                .expect("open editor source"),
            &editor_path,
        )
        .await
        .expect("upload editor fixture");
        sftp.set_permissions(&editor_path, 0o640)
            .await
            .expect("set editor fixture permissions");
        let document = sftp
            .read_text_document(&editor_path)
            .await
            .expect("read bounded remote text document");
        assert_eq!(document.content, "before\neditor fixture\n");
        assert_eq!(document.permissions.map(|mode| mode & 0o7777), Some(0o640));
        let saved = sftp
            .save_text_document(&editor_path, &document.revision, "after\n")
            .await
            .expect("atomically save remote text document");
        assert_eq!(saved.content, "after\n");
        assert_eq!(saved.permissions.map(|mode| mode & 0o7777), Some(0o640));
        let saved_entries = sftp
            .read_dir(&remote_root)
            .await
            .expect("list remote fixture directory after editor save");
        let saved_entry = saved_entries
            .iter()
            .find(|entry| entry.path == editor_path)
            .expect("find saved editor fixture");
        assert_eq!(
            saved_entry.permissions.map(|mode| mode & 0o7777),
            Some(0o640)
        );
        assert_no_remote_editor_artifacts(&sftp, &remote_root).await;
        assert!(matches!(
            sftp.save_text_document(&editor_path, &document.revision, "stale\n")
                .await,
            Err(SshError::RemoteConflict)
        ));
        assert_no_remote_editor_artifacts(&sftp, &remote_root).await;
        let save_as_path = fixture
            .directory
            .path()
            .join(format!("mobarust-editor-copy-{}.txt", std::process::id()))
            .to_string_lossy()
            .into_owned();
        let copied = sftp
            .save_text_document_as(&save_as_path, "copy\n", RemoteTextEncoding::Utf8, false)
            .await
            .expect("create remote text document through save-as");
        assert_eq!(copied.content, "copy\n");
        assert_no_remote_editor_artifacts(&sftp, &remote_root).await;
        assert!(matches!(
            sftp.save_text_document_as(
                &save_as_path,
                "blocked\n",
                RemoteTextEncoding::Utf8,
                false,
            )
            .await,
            Err(SshError::RemoteTargetExists)
        ));
        assert_no_remote_editor_artifacts(&sftp, &remote_root).await;
        let replaced = sftp
            .save_text_document_as(&save_as_path, "replaced\n", RemoteTextEncoding::Utf8, true)
            .await
            .expect("replace remote save-as target explicitly");
        assert_eq!(replaced.content, "replaced\n");
        assert_no_remote_editor_artifacts(&sftp, &remote_root).await;
        let editor_link = fixture.directory.path().join("editor-link.txt");
        std::os::unix::fs::symlink(&editor_path, &editor_link)
            .expect("create remote editor symlink fixture");
        let editor_link_path = editor_link.to_string_lossy().into_owned();
        let link_entry = sftp
            .read_dir(&remote_root)
            .await
            .expect("list remote editor symlink")
            .into_iter()
            .find(|entry| entry.path == editor_link_path)
            .expect("find remote editor symlink");
        assert!(link_entry.is_symlink);
        assert!(!link_entry.is_regular);
        assert!(matches!(
            sftp.read_text_document(&editor_link_path).await,
            Err(SshError::RemoteFileSymlink)
        ));
        assert!(matches!(
            sftp.save_text_document_as(&editor_link_path, "blocked\n", RemoteTextEncoding::Utf8, true)
                .await,
            Err(SshError::RemoteFileSymlink)
        ));
        assert!(fs::symlink_metadata(&editor_link).expect("inspect editor link").file_type().is_symlink());
        assert_eq!(sftp.read_text_document(&editor_path).await.expect("read unchanged link target").content, "after\n");
        let dangling_link = fixture.directory.path().join("editor-dangling-link.txt");
        std::os::unix::fs::symlink("missing-editor-target.txt", &dangling_link)
            .expect("create dangling remote editor symlink fixture");
        let dangling_path = dangling_link.to_string_lossy().into_owned();
        assert!(sftp.try_exists(&dangling_path).await.expect("detect dangling remote symlink"));
        assert!(matches!(
            sftp.save_text_document_as(&dangling_path, "blocked\n", RemoteTextEncoding::Utf8, false)
                .await,
            Err(SshError::RemoteTargetExists)
        ));
        assert!(fs::symlink_metadata(&dangling_link).expect("inspect dangling link").file_type().is_symlink());
        fs::remove_file(&editor_link).expect("remove remote editor symlink fixture");
        fs::remove_file(&dangling_link).expect("remove dangling symlink fixture");
        sftp.remove_file(&save_as_path)
            .await
            .expect("remove save-as fixture");
        sftp.remove_file(&editor_path)
            .await
            .expect("remove editor fixture");
        sftp.close().await.expect("close SFTP subsystem");

        let scp_remote_path = fixture
            .directory
            .path()
            .join(format!("mobarust-scp-{}", std::process::id()))
            .to_string_lossy()
            .into_owned();
        let scp_uploaded = connection
            .scp_upload(
                &scp_remote_path,
                128 * 1024,
                tokio::fs::File::open(&source)
                    .await
                    .expect("open SCP upload source"),
            )
            .await
            .expect("stream upload through SCP");
        assert_eq!(scp_uploaded, 128 * 1024);
        let verify_scp = connection
            .open_sftp()
            .await
            .expect("open SCP verification SFTP");
        assert!(
            verify_scp
                .try_exists(&scp_remote_path)
                .await
                .expect("check SCP upload")
        );
        verify_scp
            .close()
            .await
            .expect("close SCP verification SFTP");
        let scp_cancelled = fixture.directory.path().join("scp-cancelled.bin");
        let mut scp_cancelled_file = tokio::fs::File::create(&scp_cancelled)
            .await
            .expect("create cancelled SCP destination");
        let (scp_cancel_sender, mut scp_cancel_receiver) = oneshot::channel();
        let mut scp_cancel_sender = Some(scp_cancel_sender);
        let mut scp_progress = 0_u64;
        let scp_cancellation = connection
            .scp_download_with_cancel(
                &scp_remote_path,
                &mut scp_cancelled_file,
                &mut scp_cancel_receiver,
                |bytes, _total| {
                    scp_progress = bytes;
                    if let Some(sender) = scp_cancel_sender.take() {
                        let _ = sender.send(());
                    }
                },
            )
            .await;
        assert!(matches!(scp_cancellation, Err(SshError::Cancelled)));
        assert!(scp_progress > 0 && scp_progress < 128 * 1024);
        drop(scp_cancelled_file);
        assert!(
            fs::metadata(&scp_cancelled)
                .expect("inspect cancelled SCP destination")
                .len()
                < 128 * 1024
        );
        let scp_downloaded = fixture.directory.path().join("scp-downloaded.bin");
        let scp_downloaded_bytes = connection
            .scp_download(
                &scp_remote_path,
                tokio::fs::File::create(&scp_downloaded)
                    .await
                    .expect("create SCP download destination"),
            )
            .await
            .expect("stream download through SCP");
        assert_eq!(scp_downloaded_bytes, 128 * 1024);
        assert_eq!(
            fs::read(&scp_downloaded).expect("read SCP download"),
            vec![b'R'; 128 * 1024]
        );
        let cleanup_sftp = connection.open_sftp().await.expect("open SCP cleanup SFTP");
        cleanup_sftp
            .remove_file(&scp_remote_path)
            .await
            .expect("remove SCP fixture file");
        cleanup_sftp.close().await.expect("close SCP cleanup SFTP");

        let echo_listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind direct-tcpip echo fixture");
        let echo_port = echo_listener.local_addr().expect("read echo port").port();
        let echo_task = tokio::spawn(async move {
            let (mut socket, _) = echo_listener.accept().await.expect("accept echo client");
            let mut payload = [0_u8; 13];
            socket
                .read_exact(&mut payload)
                .await
                .expect("read echo payload");
            socket
                .write_all(&payload)
                .await
                .expect("write echo payload");
        });
        let mut forwarded = connection
            .open_direct_tcpip("127.0.0.1", u32::from(echo_port))
            .await
            .expect("open SSH direct-tcpip channel");
        forwarded
            .write_all(b"MOBARUST_TUNL")
            .await
            .expect("write through direct-tcpip channel");
        let mut response = [0_u8; 13];
        tokio::time::timeout(Duration::from_secs(5), forwarded.read_exact(&mut response))
            .await
            .expect("direct-tcpip response timeout")
            .expect("read through direct-tcpip channel");
        assert_eq!(&response, b"MOBARUST_TUNL");
        drop(forwarded);
        echo_task.await.expect("join echo fixture");

        let remote_forward_target = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind remote-forward target fixture");
        let remote_forward_target_port = remote_forward_target
            .local_addr()
            .expect("read remote-forward target port")
            .port();
        let target_task = tokio::spawn(async move {
            let (mut socket, _) = remote_forward_target
                .accept()
                .await
                .expect("accept remote-forward target");
            let mut payload = [0_u8; 15];
            socket
                .read_exact(&mut payload)
                .await
                .expect("read remote-forward payload");
            socket
                .write_all(&payload)
                .await
                .expect("write remote-forward payload");
        });
        let remote_forward_port = connection
            .request_remote_forward("127.0.0.1", 0)
            .await
            .expect("request SSH remote port forward");
        let remote_client_task = tokio::spawn(async move {
            let mut client = TcpStream::connect(("127.0.0.1", remote_forward_port))
                .await
                .expect("connect to remote-forward listener");
            client
                .write_all(b"MOBARUST_REMOTE")
                .await
                .expect("write remote-forward payload");
            let mut response = [0_u8; 15];
            client
                .read_exact(&mut response)
                .await
                .expect("read remote-forward response");
            assert_eq!(&response, b"MOBARUST_REMOTE");
        });
        let forwarded_channel =
            tokio::time::timeout(Duration::from_secs(5), connection.next_forwarded_channel())
                .await
                .expect("remote-forward channel timeout")
                .expect("remote-forward channel closed");
        let mut forwarded_stream = forwarded_channel.into_stream();
        let mut target = TcpStream::connect(("127.0.0.1", remote_forward_target_port))
            .await
            .expect("connect local remote-forward target");
        copy_bidirectional(&mut forwarded_stream, &mut target)
            .await
            .expect("bridge remote-forward channel");
        connection
            .cancel_remote_forward("127.0.0.1", u32::from(remote_forward_port))
            .await
            .expect("cancel SSH remote port forward");
        remote_client_task
            .await
            .expect("join remote-forward client");
        target_task.await.expect("join remote-forward target");

        let (first_disconnect, second_disconnect) =
            tokio::join!(connection.disconnect(), connection.disconnect());
        first_disconnect.expect("disconnect SSH fixture");
        second_disconnect.expect("concurrent disconnect should be idempotent");
        assert_eq!(
            connection.state(),
            mobarust_core::ConnectionState::Disconnected
        );
    });
}

#[test]
fn forwards_an_explicit_x11_channel_to_a_loopback_display_fixture() {
    let runtime = tokio::runtime::Runtime::new().expect("create SSH X11 test runtime");
    runtime.block_on(async {
        let fixture = LocalSshd::start_with_x11().expect("start local sshd X11 fixture");
        wait_for_port(fixture.port).await;
        let display = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind loopback X11 display fixture");
        let display_address = display.local_addr().expect("read X11 fixture address");
        let display_task = tokio::spawn(async move {
            let (mut stream, peer) = display.accept().await.expect("accept X11 display client");
            assert_eq!(peer.ip(), std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
            let mut payload = [0_u8; 15];
            stream
                .read_exact(&mut payload)
                .await
                .expect("read X11 fixture payload");
            assert_eq!(&payload, b"MOBARUST_X11_OK");
            stream
                .write_all(&payload)
                .await
                .expect("write X11 fixture response");
        });

        let connection = SshConnection::connect(SshConnectOptions {
            host: "127.0.0.1".into(),
            port: fixture.port,
            host_key_policy: HostKeyPolicy::KnownHosts(fixture.known_hosts.clone()),
            timeout: Duration::from_secs(5),
            keepalive_interval: None,
            credentials: SshCredentials::private_key(
                fixture.username.clone(),
                fixture.client_key.clone(),
                None::<String>,
            ),
            x11: Some(X11ForwardingOptions::new(
                X11Display::parse(&format!("tcp://{display_address}")).unwrap(),
                true,
            )),
            environment: Vec::new(),
            startup_directory: None,
            startup_command: None,
        })
        .await
        .expect("connect local SSH X11 fixture");
        let shell = connection.open_shell(100, 30).await.expect("open X11 shell");
        let (mut reader, writer) = shell.split();
        writer
            .write(
            br#"python3 -c 'import os,socket; d=os.environ["DISPLAY"]; h,rest=d.rsplit(":",1); p=int(rest.split(".",1)[0])+6000; s=socket.create_connection((h,p),5); s.sendall(b"MOBARUST_X11_OK"); s.shutdown(socket.SHUT_WR); response=s.recv(15); assert response == b"MOBARUST_X11_OK"; s.close()'
exit
"#,
            )
            .await
            .expect("request remote X11 client fixture");

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut shell_output = Vec::new();
        let channel = loop {
            tokio::select! {
                channel = connection.next_x11_channel() => {
                    break channel.expect("SSH X11 channel was not opened");
                }
                output = reader.next_output() => {
                    if let Some(Ok(SshOutput::Stdout(bytes) | SshOutput::Stderr(bytes))) = output {
                        shell_output.extend(bytes);
                    }
                }
                _ = tokio::time::sleep_until(deadline) => {
                    panic!("wait for SSH X11 channel: {}", String::from_utf8_lossy(&shell_output));
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(5), connection.bridge_x11_channel(channel))
            .await
            .expect("bridge X11 channel timeout")
            .expect("bridge X11 channel");
        display_task.await.expect("join X11 display fixture");

        while tokio::time::timeout(Duration::from_millis(250), reader.next_output())
            .await
            .ok()
            .flatten()
            .is_some()
        {}
    });
}

#[test]
fn forwards_x11_setup_to_a_disposable_xvfb_server_when_available() {
    let Some(xvfb) = LocalXvfb::start().expect("start disposable Xvfb fixture") else {
        eprintln!(
            "skipping real X11 server fixture: Xvfb or a safe system Unix-socket directory is unavailable"
        );
        return;
    };
    let runtime = tokio::runtime::Runtime::new().expect("create real X11 test runtime");
    runtime.block_on(async {
        xvfb.wait_ready().await;
        let fixture = LocalSshd::start_with_x11().expect("start local sshd X11 fixture");
        wait_for_port(fixture.port).await;

        let connection = SshConnection::connect(SshConnectOptions {
            host: "127.0.0.1".into(),
            port: fixture.port,
            host_key_policy: HostKeyPolicy::KnownHosts(fixture.known_hosts.clone()),
            timeout: Duration::from_secs(5),
            keepalive_interval: None,
            credentials: SshCredentials::private_key(
                fixture.username.clone(),
                fixture.client_key.clone(),
                None::<String>,
            ),
            x11: Some(X11ForwardingOptions::new(X11Display::Unix(xvfb.socket.clone()), true)),
            environment: Vec::new(),
            startup_directory: None,
            startup_command: None,
        })
        .await
        .expect("connect local SSH X11 fixture");
        let shell = connection.open_shell(100, 30).await.expect("open X11 shell");
        let (mut reader, writer) = shell.split();
        writer
            .write(
                br#"python3 -c 'import os,socket; d=os.environ["DISPLAY"]; h,rest=d.rsplit(":",1); p=int(rest.split(".",1)[0])+6000; s=socket.create_connection((h,p),5); s.sendall(b"l\x00\x0b\x00\x00\x00\x00\x00\x00\x00\x00\x00"); response=b"";
while len(response)<8:
    chunk=s.recv(8-len(response))
    if not chunk: raise EOFError("X11 setup reply ended early")
    response+=chunk
assert response[0:1] == b"\x01"
s.close()'
exit
"#,
            )
            .await
            .expect("request real X11 setup");

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let channel = loop {
            tokio::select! {
                channel = connection.next_x11_channel() => {
                    break channel.expect("real X11 channel was not opened");
                }
                _ = reader.next_output() => {}
                _ = tokio::time::sleep_until(deadline) => {
                    panic!("wait for real X11 channel timed out");
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(5), connection.bridge_x11_channel(channel))
            .await
            .expect("bridge real X11 channel timeout")
            .expect("bridge real X11 channel");

        let mut shell_output = Vec::new();
        while let Some(message) = tokio::time::timeout(Duration::from_secs(5), reader.next_output())
            .await
            .expect("wait for real X11 command")
        {
            match message.expect("read real X11 command output") {
                SshOutput::Stdout(bytes) | SshOutput::Stderr(bytes) => shell_output.extend(bytes),
                SshOutput::ExitStatus(status) => {
                    assert_eq!(status, 0, "real X11 setup failed: {}", String::from_utf8_lossy(&shell_output));
                    break;
                }
                SshOutput::Control => {}
            }
        }
    });
}

struct LocalXvfb {
    child: Child,
    socket: PathBuf,
}

impl LocalXvfb {
    fn start() -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let Some(program) = find_xvfb() else {
            return Ok(None);
        };
        let Some((display_number, socket)) = reserve_xvfb_display() else {
            return Ok(None);
        };
        if !prepare_xvfb_socket_dir()? {
            return Ok(None);
        }
        let mut command = Command::new(program);
        clear_credential_environment(&mut command);
        let child = command
            .arg(format!(":{display_number}"))
            .args([
                "-screen",
                "0",
                "640x480x24",
                "-nolisten",
                "tcp",
                "-nolock",
                "-ac",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Some(Self { child, socket }))
    }

    async fn wait_ready(&self) {
        for _ in 0..120 {
            if UnixStream::connect(&self.socket).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("disposable Xvfb did not create its private Unix socket");
    }
}

impl Drop for LocalXvfb {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn assert_no_remote_editor_artifacts(sftp: &SftpConnection, remote_root: &str) {
    let entries = sftp
        .read_dir(remote_root)
        .await
        .expect("list remote fixture directory for editor cleanup assertion");
    let artifacts = entries
        .iter()
        .filter(|entry| entry.name.contains(".mobarust-edit-"))
        .map(|entry| entry.name.as_str())
        .collect::<Vec<_>>();
    assert!(
        artifacts.is_empty(),
        "remote editor left temporary artifacts: {artifacts:?}"
    );
}

struct LocalSshd {
    child: Child,
    directory: tempfile::TempDir,
    username: String,
    port: u16,
    client_key: std::path::PathBuf,
    known_hosts: std::path::PathBuf,
}

impl LocalSshd {
    fn options(&self) -> SshConnectOptions {
        SshConnectOptions {
            host: "127.0.0.1".into(),
            port: self.port,
            host_key_policy: HostKeyPolicy::KnownHosts(self.known_hosts.clone()),
            timeout: Duration::from_secs(5),
            keepalive_interval: None,
            credentials: SshCredentials::private_key(
                self.username.clone(),
                self.client_key.clone(),
                None::<String>,
            ),
            x11: None,
            environment: Vec::new(),
            startup_directory: None,
            startup_command: None,
        }
    }

    fn start() -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_internal(false, false)
    }

    fn start_with_x11() -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_internal(true, false)
    }

    fn start_internal(x11: bool, ipv6: bool) -> Result<Self, Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let host_key = directory.path().join("host_key");
        let client_key = directory.path().join("client_key");
        let authorized_keys = directory.path().join("authorized_keys");
        let host_public = directory.path().join("host_key.pub");
        let client_public = directory.path().join("client_key.pub");
        let known_hosts = directory.path().join("known_hosts");
        run_keygen(&host_key)?;
        run_keygen(&client_key)?;
        fs::copy(&client_public, &authorized_keys)?;

        let host_line = fs::read_to_string(&host_public)?;
        let host_key_material = host_line
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
        let port = reserve_port()?;
        fs::write(
            &known_hosts,
            format!("[127.0.0.1]:{port} {host_key_material}\n"),
        )?;

        let x11_config = if x11 {
            let xauth =
                find_command("xauth").ok_or("X11 fixture requires a local xauth executable")?;
            // sshd may derive its xauth path from the OS account's home.
            // This test-only wrapper always writes the fixture authority file.
            let wrapper = directory.path().join("fixture-xauth");
            let authority = directory.path().join(".Xauthority");
            fs::write(
                &wrapper,
                format!(
                    "#!/bin/sh\nexec '{}' -q -f '{}' -\n",
                    xauth.to_string_lossy().replace('\'', "'\\''"),
                    authority.to_string_lossy().replace('\'', "'\\''"),
                ),
            )?;
            fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700))?;
            format!(
                "X11Forwarding yes\nX11UseLocalhost yes\nXAuthLocation {}\n",
                wrapper.display()
            )
        } else {
            "X11Forwarding no\n".to_owned()
        };

        let username = std::env::var("USER")?;
        // sshd builds a session environment from the OS account database, not
        // the daemon's HOME. Override it before the user's shell starts.
        let home = directory
            .path()
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        let config = directory.path().join("sshd_config");
        let ipv6_listener = if ipv6 { "ListenAddress ::1\n" } else { "" };
        fs::write(
            &config,
            format!(
                "Port {port}\nListenAddress 127.0.0.1\n{ipv6_listener}HostKey {}\nAuthorizedKeysFile {}\nPidFile \"{home}/sshd.pid\"\nSubsystem sftp internal-sftp\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPubkeyAuthentication yes\nPermitRootLogin no\nPermitUserRC no\nPermitUserEnvironment no\nUsePAM no\nStrictModes no\nAllowTcpForwarding yes\nAcceptEnv MOBARUST_FIXTURE\nSetEnv \"HOME={home}\" \"ZDOTDIR={home}\" \"XDG_CONFIG_HOME={home}\" \"XAUTHORITY={home}/.Xauthority\" BASH_ENV=/dev/null ENV=/dev/null\n{x11_config}AllowUsers {username}\nPrintMotd no\nUseDNS no\nLogLevel QUIET\n",
                host_key.display(),
                authorized_keys.display(),
            ),
        )?;

        let child = spawn_sshd(&config)?;

        Ok(Self {
            child,
            directory,
            username,
            port,
            client_key,
            known_hosts,
        })
    }
}

fn spawn_sshd(config: &Path) -> std::io::Result<Child> {
    let sshd = if Path::new("/usr/sbin/sshd").exists() {
        "/usr/sbin/sshd"
    } else {
        "/usr/local/sbin/sshd"
    };
    let mut command = Command::new(sshd);
    clear_credential_environment(&mut command);
    command
        .args(["-D", "-e", "-f"])
        .arg(config)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
}

fn find_command(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

fn find_xvfb() -> Option<PathBuf> {
    [
        PathBuf::from("/opt/X11/bin/Xvfb"),
        PathBuf::from("/usr/bin/Xvfb"),
        PathBuf::from("/usr/local/bin/Xvfb"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
    .or_else(|| find_command("Xvfb"))
}

const X11_SOCKET_DIR: &str = "/tmp/.X11-unix";

fn reserve_xvfb_display() -> Option<(u16, PathBuf)> {
    (90..200).find_map(|display_number| {
        let socket = PathBuf::from(format!("{X11_SOCKET_DIR}/X{display_number}"));
        let lock = PathBuf::from(format!("/tmp/.X{display_number}-lock"));
        match (fs::symlink_metadata(&socket), fs::symlink_metadata(lock)) {
            (Err(socket_error), Err(lock_error))
                if socket_error.kind() == std::io::ErrorKind::NotFound
                    && lock_error.kind() == std::io::ErrorKind::NotFound =>
            {
                Some((display_number, socket))
            }
            _ => None,
        }
    })
}

fn prepare_xvfb_socket_dir() -> Result<bool, Box<dyn std::error::Error>> {
    match fs::symlink_metadata(X11_SOCKET_DIR) {
        Ok(metadata) => Ok(metadata.is_dir() && !metadata.file_type().is_symlink()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

impl Drop for LocalSshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = self.directory.path();
    }
}

fn run_keygen(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut command = Command::new("ssh-keygen");
    clear_credential_environment(&mut command);
    let status = command
        .args(["-q", "-t", "ed25519", "-N", ""])
        .arg("-f")
        .arg(path)
        .status()?;
    assert!(status.success(), "ssh-keygen failed for {}", path.display());
    Ok(())
}

fn clear_credential_environment(command: &mut Command) {
    for variable in [
        "SSH_AUTH_SOCK",
        "SSH_AGENT_PID",
        "MOBARUST_TEST_AGENT_SOCKET",
        "GIT_SSH_COMMAND",
        "GIT_CONFIG_GLOBAL",
        "GIT_CONFIG_SYSTEM",
    ] {
        command.env_remove(variable);
    }
}

fn reserve_port() -> Result<u16, Box<dyn std::error::Error>> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

async fn wait_for_port(port: u16) {
    for _ in 0..100 {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("local sshd did not listen on port {port}");
}
