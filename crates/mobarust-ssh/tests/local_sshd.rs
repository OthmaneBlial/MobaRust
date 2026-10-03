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

async fn connect_through_interruptible_bridge(
    fixture: &LocalSshd,
) -> (
    SshConnection,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
    std::net::SocketAddr,
) {
    // Own both sockets: interruption never kills another session or daemon.
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let trust = fixture
        .directory
        .path()
        .join(format!("bridge_known_hosts_{}", address.port()));
    fs::write(
        &trust,
        fs::read_to_string(&fixture.known_hosts).unwrap().replace(
            &format!("[127.0.0.1]:{}", fixture.port),
            &format!("[127.0.0.1]:{}", address.port()),
        ),
    )
    .unwrap();
    let server_port = fixture.port;
    let (cut_tx, cut_rx) = oneshot::channel();
    let worker = tokio::spawn(async move {
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
    options.port = address.port();
    options.host_key_policy = HostKeyPolicy::KnownHosts(trust);
    let connection = SshConnection::connect(options).await.unwrap();
    (connection, cut_tx, worker, address)
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
        let sftp = connection.open_sftp().await.expect("open isolated SFTP");
        assert_eq!(
            PathBuf::from(sftp.canonicalize(".").await.unwrap()),
            fs::canonicalize(fixture.directory.path()).unwrap(),
            "SFTP must start in its disposable HOME, independently of shell environment"
        );
        assert_eq!(
            fixture
                .directory
                .path()
                .metadata()
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        fs::write(
            fixture.directory.path().join("isolation-marker"),
            b"generated fixture bytes",
        )
        .unwrap();
        assert!(
            sftp.read_dir(".")
                .await
                .unwrap()
                .iter()
                .any(|entry| entry.name == "isolation-marker")
        );
        let mut bytes = Vec::new();
        sftp.download_to("isolation-marker", &mut bytes)
            .await
            .unwrap();
        assert_eq!(bytes, b"generated fixture bytes");
        sftp.close().await.unwrap();
        connection.disconnect().await.expect("disconnect fixture");
    });
}

#[test]
fn remote_editor_encoding_changes_preserve_byte_conflicts_and_permissions() {
    let runtime = tokio::runtime::Runtime::new().expect("create editor test runtime");
    runtime.block_on(async {
        let fixture = LocalSshd::start().expect("start local sshd fixture");
        wait_for_port(fixture.port).await;
        let connection = SshConnection::connect(fixture.options()).await.unwrap();
        let sftp = connection.open_sftp().await.unwrap();
        let path = fixture.directory.path().join("encoding.txt");
        let remote_path = path.to_string_lossy();
        let text = "café · €\n";
        let legacy_bytes = b"caf\xe9 \xb7 \x80\n";
        fs::write(&path, text).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let opened = sftp.read_text_document(remote_path.as_ref()).await.unwrap();
        let legacy = sftp
            .save_text_document_with_encoding(
                remote_path.as_ref(),
                &opened.revision,
                text,
                RemoteTextEncoding::Windows1252,
            )
            .await
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), legacy_bytes);
        assert_eq!(legacy.content, text);
        assert_eq!(legacy.encoding, RemoteTextEncoding::Windows1252);
        assert!(matches!(
            sftp.read_text_document(remote_path.as_ref()).await,
            Err(SshError::RemoteFileNotUtf8)
        ));

        let converted = sftp
            .save_text_document_with_encoding(
                remote_path.as_ref(),
                &legacy.revision,
                text,
                RemoteTextEncoding::Utf8,
            )
            .await
            .expect("convert existing Windows-1252 bytes to UTF-8");
        assert_eq!(fs::read(&path).unwrap(), text.as_bytes());
        assert_eq!(converted.content, text);
        assert_eq!(converted.encoding, RemoteTextEncoding::Utf8);
        assert_eq!(converted.size, text.len() as u64);
        assert_eq!(converted.permissions.map(|mode| mode & 0o7777), Some(0o640));

        // A concurrent change must be a byte conflict, even if the new bytes
        // cannot be decoded using the selected output encoding.
        fs::write(&path, b"changed\xff").unwrap();
        assert!(matches!(
            sftp.save_text_document_with_encoding(
                remote_path.as_ref(),
                &converted.revision,
                text,
                RemoteTextEncoding::Utf8
            )
            .await,
            Err(SshError::RemoteConflict)
        ));
        assert_eq!(fs::read(&path).unwrap(), b"changed\xff");
        assert!(matches!(
            sftp.save_text_document_with_encoding(
                remote_path.as_ref(),
                &converted.revision,
                "😀",
                RemoteTextEncoding::Windows1252
            )
            .await,
            Err(SshError::RemoteTextEncodingUnsupported)
        ));
        assert_eq!(fs::read(&path).unwrap(), b"changed\xff");

        // Save as checks the target's bytes independently of the output
        // encoding, but still requires explicit replacement permission.
        fs::write(&path, legacy_bytes).unwrap();
        assert!(matches!(
            sftp.save_text_document_as(remote_path.as_ref(), text, RemoteTextEncoding::Utf8, false)
                .await,
            Err(SshError::RemoteTargetExists)
        ));
        assert_eq!(fs::read(&path).unwrap(), legacy_bytes);
        let replaced = sftp
            .save_text_document_as(remote_path.as_ref(), text, RemoteTextEncoding::Utf8, true)
            .await
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), text.as_bytes());
        assert_eq!(replaced.content, text);
        assert_eq!(replaced.permissions.map(|mode| mode & 0o7777), Some(0o640));
        assert_no_remote_editor_artifacts(&sftp, fixture.directory.path().to_str().unwrap()).await;
        sftp.close().await.unwrap();
        connection.disconnect().await.unwrap();
    });
}

#[test]
fn remote_editor_temporary_collisions_preserve_unowned_files() {
    const CHILD: &str = "MOBARUST_EDITOR_COLLISION_CHILD";
    if std::env::var_os(CHILD).is_none() {
        // Give the editor's process-local counter a deterministic starting
        // point without racing other tests or mutating their environment.
        let mut command = Command::new(std::env::current_exe().unwrap());
        clear_credential_environment(&mut command);
        assert!(
            command
                .args([
                    "--exact",
                    "remote_editor_temporary_collisions_preserve_unowned_files",
                    "--nocapture"
                ])
                .env(CHILD, "1")
                .status()
                .unwrap()
                .success()
        );
        return;
    }
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let fixture = LocalSshd::start().unwrap();
        wait_for_port(fixture.port).await;
        let connection = SshConnection::connect(fixture.options()).await.unwrap();
        let sftp = connection.open_sftp().await.unwrap();
        let original = fixture.directory.path().join("original.txt");
        let victim = fixture.directory.path().join("unrelated.txt");
        fs::write(&original, b"original document").unwrap();
        fs::set_permissions(&original, fs::Permissions::from_mode(0o640)).unwrap();
        fs::write(&victim, b"unrelated bytes").unwrap();
        let path = original.to_str().unwrap();
        let document = sftp.read_text_document(path).await.unwrap();
        for (index, (save_as, symlink)) in
            [(false, false), (false, true), (true, false), (true, true)]
                .into_iter()
                .enumerate()
        {
            let temporary = PathBuf::from(format!(
                "{path}.mobarust-edit-{}-{}",
                std::process::id(),
                index + 1
            ));
            if symlink {
                std::os::unix::fs::symlink(&victim, &temporary).unwrap();
            } else {
                fs::write(&temporary, b"unowned temporary bytes").unwrap();
            }
            let result = if save_as {
                sftp.save_text_document_as(path, "new bytes", RemoteTextEncoding::Utf8, true)
                    .await
            } else {
                sftp.save_text_document(path, &document.revision, "new bytes")
                    .await
            };
            assert!(
                result.is_err(),
                "a colliding editor temporary must be refused"
            );
            assert_eq!(fs::read(&original).unwrap(), b"original document");
            assert_eq!(fs::read(&victim).unwrap(), b"unrelated bytes");
            if symlink {
                assert!(
                    fs::symlink_metadata(&temporary)
                        .unwrap()
                        .file_type()
                        .is_symlink()
                );
                assert_eq!(fs::read_link(&temporary).unwrap(), victim);
            } else {
                assert_eq!(fs::read(&temporary).unwrap(), b"unowned temporary bytes");
            }
            fs::remove_file(&temporary).unwrap();
        }
        let created_path = fixture.directory.path().join("new-private.txt");
        let created = sftp
            .save_text_document_as(
                created_path.to_str().unwrap(),
                "private bytes",
                RemoteTextEncoding::Utf8,
                false,
            )
            .await
            .unwrap();
        assert_eq!(created.content, "private bytes");
        assert_eq!(created.permissions.map(|mode| mode & 0o7777), Some(0o600));
        assert_eq!(
            fs::metadata(&created_path).unwrap().permissions().mode() & 0o7777,
            0o600
        );
        let saved = sftp
            .save_text_document(path, &document.revision, "new document")
            .await
            .unwrap();
        assert_eq!(saved.content, "new document");
        assert_eq!(saved.permissions.map(|mode| mode & 0o7777), Some(0o640));
        assert_no_remote_editor_artifacts(&sftp, fixture.directory.path().to_str().unwrap()).await;
        sftp.close().await.unwrap();
        connection.disconnect().await.unwrap();
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
        let connection = SshConnection::connect(fixture.ipv6_options())
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

        let (connection, cut_tx, bridge, _) = connect_through_interruptible_bridge(&fixture).await;
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
fn interrupted_large_sftp_and_scp_transfers_fail_then_retry_from_the_start() {
    let runtime = tokio::runtime::Runtime::new().expect("create interrupted transfer runtime");
    runtime.block_on(async {
        let fixture = LocalSshd::start().expect("start interrupted transfer fixture");
        wait_for_port(fixture.port).await;
        let payload = (0..16 * 1024 * 1024)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let source = fixture.directory.path().join("source café 🦀.bin");
        fs::write(&source, &payload).unwrap();
        let remote_source = source.to_string_lossy().into_owned();
        let original_trust = fs::read(&fixture.known_hosts).unwrap();

        for scp in [false, true] {
            for upload in [false, true] {
                let label = format!(
                    "{}-{}",
                    if scp { "scp" } else { "sftp" },
                    if upload { "upload" } else { "download" },
                );
                let destination = fixture.directory.path().join(format!("{label} café final.bin"));
                let part = fixture.directory.path().join(format!(".{label} café.mobarust.part"));
                let remote_part = part.to_string_lossy().into_owned();
                let remote_destination = destination.to_string_lossy().into_owned();
                fs::write(&destination, b"original destination").unwrap();
                let (connection, cut, bridge, address) =
                    connect_through_interruptible_bridge(&fixture).await;
                let sftp = connection.open_sftp().await.unwrap();
                let mut input = tokio::fs::File::open(&source).await.unwrap();
                let mut output = if upload {
                    None
                } else {
                    Some(tokio::fs::File::create(&part).await.unwrap())
                };
                let (_cancel_sender, mut cancel) = oneshot::channel();
                let (progress_tx, progress_rx) = oneshot::channel();
                let mut progress_tx = Some(progress_tx);
                let mut progress = |bytes| {
                    if bytes >= 256 * 1024
                        && let Some(sender) = progress_tx.take()
                    {
                        sender.send(bytes).unwrap();
                    }
                };
                {
                    let transfer = async {
                        match (scp, upload) {
                            (false, false) => sftp
                                .download_to_with_cancel(
                                    &remote_source,
                                    output.as_mut().unwrap(),
                                    &mut cancel,
                                    &mut progress,
                                )
                                .await,
                            (false, true) => sftp
                                .upload_from_with_cancel(
                                    &mut input, &remote_part, &mut cancel, &mut progress,
                                )
                                .await,
                            (true, false) => connection
                                .scp_download_with_cancel(
                                    &remote_source,
                                    output.as_mut().unwrap(),
                                    &mut cancel,
                                    |bytes, total| {
                                        assert_eq!(total, payload.len() as u64);
                                        progress(bytes);
                                    },
                                )
                                .await,
                            (true, true) => connection
                                .scp_upload_with_cancel(
                                    &remote_part,
                                    payload.len() as u64,
                                    &mut input,
                                    &mut cancel,
                                    &mut progress,
                                )
                                .await,
                        }
                    };
                    tokio::pin!(transfer);
                    let progressed = tokio::time::timeout(Duration::from_secs(20), async {
                        tokio::select! {
                            biased;
                            bytes = progress_rx => bytes.unwrap(),
                            result = &mut transfer => panic!("{label} completed before interruption: {result:?}"),
                        }
                    })
                    .await
                    .expect("transfer must reach the interruption boundary");
                    assert!(progressed < payload.len() as u64);
                    if upload {
                        // Progress can mean queued writes. Require server-side
                        // bytes before cutting an actual in-flight upload.
                        tokio::time::timeout(Duration::from_secs(5), async {
                            loop {
                                if fs::metadata(&part).is_ok_and(|metadata| metadata.len() >= progressed) {
                                    break;
                                }
                                tokio::time::sleep(Duration::from_millis(10)).await;
                            }
                        })
                        .await
                        .expect("server must receive partial upload bytes");
                    }
                    // Stop polling the copy until both relay sockets are gone;
                    // neither a sleep nor a racing cancellation selects the cut.
                    cut.send(()).unwrap();
                    tokio::time::timeout(Duration::from_secs(5), bridge)
                        .await
                        .unwrap()
                        .unwrap();
                    let failure = tokio::time::timeout(Duration::from_secs(20), &mut transfer)
                        .await
                        .expect("interrupted transfer must terminate")
                        .expect_err("interrupted transfer must not report success");
                    assert!(
                        !matches!(failure, SshError::Cancelled),
                        "{label} must fail from transport loss, not cancellation",
                    );
                    if !scp {
                        assert!(
                            matches!(failure, SshError::SftpConnectionLost),
                            "{label}: misleading error {failure:?}",
                        );
                    }
                    eprintln!("{label}: interrupted after {progressed} bytes: {failure}");
                }
                drop(output);
                assert_eq!(fs::read(&destination).unwrap(), b"original destination");
                let partial = fs::read(&part).expect("interruption leaves a real partial file");
                assert!(
                    !partial.is_empty() && partial.len() < payload.len(),
                    "{label}: invalid partial length {}",
                    partial.len(),
                );
                assert_eq!(partial, payload[..partial.len()], "{label}: partial bytes differ");
                if upload {
                    assert!(
                        matches!(
                            sftp.cleanup_temporary_file(&remote_part).await,
                            Err(SshError::RemoteTemporaryCleanupFailed),
                        ),
                        "lost transport must report unavailable remote cleanup",
                    );
                    assert!(part.exists());
                } else {
                    fs::remove_file(&part).unwrap();
                }
                let _ = tokio::time::timeout(Duration::from_secs(5), connection.disconnect())
                    .await
                    .unwrap();
                drop(sftp);
                drop(connection);
                drop(
                    TcpListener::bind(address)
                        .await
                        .expect("interrupted relay listener must be released"),
                );

                let recovered = SshConnection::connect(fixture.options()).await.unwrap();
                let recovered_sftp = recovered.open_sftp().await.unwrap();
                if upload {
                    recovered_sftp
                        .cleanup_temporary_file(&remote_part)
                        .await
                        .unwrap();
                    assert!(!part.exists());
                }
                let copied = tokio::time::timeout(Duration::from_secs(30), async {
                    let input = tokio::fs::File::open(&source).await.unwrap();
                    if upload {
                        if scp {
                            recovered.scp_upload(&remote_part, payload.len() as u64, input).await
                        } else {
                            recovered_sftp.upload_from(input, &remote_part).await
                        }
                    } else {
                        let output = tokio::fs::File::create(&part).await.unwrap();
                        if scp {
                            recovered.scp_download(&remote_source, output).await
                        } else {
                            recovered_sftp.download_to(&remote_source, output).await
                        }
                    }
                })
                .await
                .expect("full retry deadline")
                .expect("full retry succeeds");
                assert_eq!(copied, payload.len() as u64);
                assert_eq!(fs::read(&part).unwrap(), payload);
                if upload {
                    recovered_sftp
                        .promote_uploaded_file(&remote_part, &remote_destination, true)
                        .await
                        .unwrap();
                    assert_eq!(fs::read(&destination).unwrap(), payload);
                } else {
                    fs::remove_file(&part).unwrap();
                    assert_eq!(fs::read(&destination).unwrap(), b"original destination");
                }
                assert!(!part.exists());
                assert!(fs::read_dir(fixture.directory.path()).unwrap().all(|entry| {
                    !entry.unwrap().file_name().to_string_lossy()
                        .contains(".mobarust-upload-backup-")
                }));
                assert_eq!(fs::read(&fixture.known_hosts).unwrap(), original_trust);
                recovered_sftp.close().await.unwrap();
                recovered.disconnect().await.unwrap();
            }
        }
    });
}

#[test]
fn sftp_local_stream_failures_remain_local_and_redacted() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let fixture = LocalSshd::start().unwrap();
        wait_for_port(fixture.port).await;
        let source = fixture.directory.path().join("private fixture path.bin");
        fs::write(&source, b"source stays intact").unwrap();
        let remote_source = source.to_string_lossy().into_owned();
        let part = fixture.directory.path().join(".local-failure.part");
        let remote_part = part.to_string_lossy().into_owned();
        let connection = SshConnection::connect(fixture.options()).await.unwrap();
        let sftp = connection.open_sftp().await.unwrap();
        let source_without_read = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&source)
            .await
            .unwrap();
        let destination_without_write = tokio::fs::File::open(&source).await.unwrap();
        let errors = [
            sftp.upload_from(source_without_read, &remote_part)
                .await
                .unwrap_err(),
            sftp.download_to(&remote_source, destination_without_write)
                .await
                .unwrap_err(),
        ];
        for error in errors {
            assert!(matches!(error, SshError::LocalIo(_)));
            assert_eq!(error.to_string(), "local file operation failed");
        }
        assert_eq!(fs::read(&source).unwrap(), b"source stays intact");
        sftp.cleanup_temporary_file(&remote_part).await.unwrap();
        sftp.close().await.unwrap();
        connection.disconnect().await.unwrap();
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
fn ipv6_jump_chain_streams_large_shell_output_and_sftp_concurrently() {
    run_ipv6_stream_lab(false);
}

#[test]
fn ipv6_jump_chain_streams_large_shell_output_and_sftp_during_frequent_rekey() {
    run_ipv6_stream_lab(true);
}

fn run_ipv6_stream_lab(frequent_rekey: bool) {
    if std::net::TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, 0)).is_err() {
        eprintln!("skipping routed IPv6 OpenSSH fixture: IPv6 loopback is unavailable");
        return;
    }
    let runtime = tokio::runtime::Runtime::new().expect("create routed IPv6 runtime");
    runtime.block_on(async {
        let fixtures = std::array::from_fn::<_, 3, _>(|_| {
            let mut fixture =
                LocalSshd::start_internal(false, true).expect("start distinct IPv6 fixture");
            if !frequent_rekey {
                return fixture;
            }
            // Exercise packet-write resumption across server-requested key
            // exchanges on every transport, rather than only initial keys.
            fixture.child.kill().unwrap();
            fixture.child.wait().unwrap();
            let config = fixture.directory.path().join("sshd_config");
            fs::write(&config, format!("{}\nRekeyLimit 256K\n", fs::read_to_string(&config).unwrap())).unwrap();
            fixture.child = spawn_sshd(&config).expect("restart owned daemon with frequent rekey");
            fixture
        });
        for fixture in &fixtures {
            wait_for_port(fixture.port).await;
        }
        for rejected_hop in 0..3 {
            let mut options = fixtures.each_ref().map(LocalSshd::ipv6_options);
            options[rejected_hop].host_key_policy =
                HostKeyPolicy::PinnedFingerprint("SHA256:untrusted-ipv6-fixture".into());
            let [first, second, target] = options;
            let result = tokio::time::timeout(
                Duration::from_secs(20),
                SshConnection::connect_with_jump_chain(target, vec![first, second]),
            )
            .await
            .expect("IPv6 rejected-hop deadline");
            assert!(matches!(result, Err(SshError::HostKeyRejected { .. })));
        }
        let [first, second, target] = fixtures.each_ref().map(LocalSshd::ipv6_options);
        let trust_before = fixtures.each_ref().map(|fixture| {
            fs::read(fixture.directory.path().join("known_hosts_ipv6")).unwrap()
        });
        let connection = tokio::time::timeout(
            Duration::from_secs(20),
            SshConnection::connect_with_jump_chain(target, vec![first, second]),
        )
        .await
        .expect("IPv6 two-bastion connection deadline")
        .expect("connect via IPv6 at every hop");

        let line = "MobaRust café 🦀 \x1b[32mgreen\x1b[0m\n".as_bytes();
        let mut payload = line.repeat(8 * 1024 * 1024 / line.len());
        payload.resize(8 * 1024 * 1024, b'\n');
        let root = fixtures[2].directory.path();
        let source = root.join("sustained café 🦀.txt");
        fs::write(&source, &payload).unwrap();
        let uploaded = root.join("concurrent café 🦀.bin").to_string_lossy().into_owned();
        let sftp = connection.open_sftp().await.unwrap();
        let (mut reader, writer) = connection.open_shell(100, 30).await.unwrap().split();
        let command = format!(
            "stty -echo -onlcr; printf '\\nMOBARUST_STREAM_BEGIN\\n'; cat '{}'; printf '\\nMOBARUST_STREAM_END\\n'; exit\n",
            source.to_string_lossy().replace('\'', "'\\''"),
        );
        writer.write(command.as_bytes()).await.unwrap();
        let streams_started = std::time::Instant::now();
        let output = async {
            let mut bytes = Vec::new();
            let mut exit = None;
            while let Some(message) = reader.next_output().await {
                match message.expect("read routed IPv6 PTY output") {
                    SshOutput::Stdout(data) | SshOutput::Stderr(data) => {
                        bytes.extend(data);
                        assert!(bytes.len() <= payload.len() + 64 * 1024);
                    }
                    SshOutput::ExitStatus(status) => {
                        exit = Some(status);
                        break;
                    }
                    SshOutput::Control => {}
                }
            }
            assert_eq!(exit, Some(0));
            let begin = b"\nMOBARUST_STREAM_BEGIN\n";
            let start = bytes.windows(begin.len()).position(|window| window == begin)
                .expect("find output start marker") + begin.len();
            let end = start + payload.len();
            assert_eq!(&bytes[start..end], payload);
            assert!(bytes[end..].starts_with(b"\nMOBARUST_STREAM_END\n"));
            eprintln!("IPv6 progress: output completed at {:?}", streams_started.elapsed());
        };
        let files = async {
            let count = sftp.upload_from(payload.as_slice(), &uploaded).await.unwrap();
            assert_eq!(count, payload.len() as u64);
            eprintln!("IPv6 progress: upload completed at {:?}", streams_started.elapsed());
            let mut downloaded = Vec::new();
            let (_sender, mut cancel) = oneshot::channel();
            let mut reported = 0;
            assert_eq!(sftp.download_to_with_cancel(&uploaded, &mut downloaded, &mut cancel, |bytes| {
                if bytes / (1024 * 1024) > reported {
                    reported = bytes / (1024 * 1024);
                    eprintln!("IPv6 progress: downloaded {} MiB at {:?}", reported, streams_started.elapsed());
                }
            }).await.unwrap(), count);
            assert_eq!(downloaded, payload);
            assert_eq!(fs::read(&uploaded).unwrap(), payload);
            sftp.remove_file(&uploaded).await.unwrap();
        };
        // The ordinary stream gate remains 30 seconds. Frequent server key
        // renewal adds repeated cryptographic exchanges on all three hops.
        let stream_deadline = Duration::from_secs(if frequent_rekey { 60 } else { 30 });
        tokio::time::timeout(stream_deadline, async {
            tokio::join!(output, files);
        })
        .await
        .expect("concurrent IPv6 streams must finish without starving each other");
        sftp.close().await.unwrap();
        drop(reader);
        drop(writer);
        connection.disconnect().await.unwrap();
        for (fixture, original) in fixtures.iter().zip(trust_before) {
            assert_eq!(fs::read(fixture.directory.path().join("known_hosts_ipv6")).unwrap(), original);
        }
        let ports = fixtures.each_ref().map(|fixture| fixture.port);
        let homes = fixtures.each_ref().map(|fixture| fixture.directory.path().to_owned());
        drop(sftp);
        drop(connection);
        drop(fixtures);
        for port in ports {
            drop(TcpListener::bind(("127.0.0.1", port)).await.unwrap());
            drop(TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, port)).await.unwrap());
        }
        assert!(homes.iter().all(|home| !home.exists()));
        eprintln!("routed IPv6: 8 MiB PTY output and byte-matched SFTP upload/download via two bastions; frequent_rekey={frequent_rekey}");
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
fn deleting_remote_entries_unlinks_links_and_preserves_their_targets() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let fixture = LocalSshd::start().unwrap();
        wait_for_port(fixture.port).await;
        let connection = SshConnection::connect(fixture.options()).await.unwrap();
        let sftp = connection.open_sftp().await.unwrap();
        let victim = fixture.directory.path().join("delete-victim");
        let directory = fixture.directory.path().join("delete-victim-directory");
        let missing = fixture.directory.path().join("delete-missing-victim");
        fs::write(&victim, b"unchanged file target").unwrap();
        fs::set_permissions(&victim, fs::Permissions::from_mode(0o640)).unwrap();
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("child"), b"unchanged directory child").unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o750)).unwrap();
        assert!(sftp.remove_path(directory.to_str().unwrap()).await.is_err());
        assert!(matches!(
            sftp.remove_path(missing.to_str().unwrap()).await,
            Err(SshError::SftpPathMissing)
        ));
        let selected = fixture.directory.path().join("delete-selected");
        let mut failures = Vec::new();
        for kind in [
            "regular",
            "empty-directory",
            "file-link",
            "directory-link",
            "directory-link-slash",
            "directory-link-slashes",
            "dangling-link",
            "socket",
        ] {
            let socket = match kind {
                "regular" => {
                    fs::write(&selected, b"selected file").unwrap();
                    None
                }
                "empty-directory" => {
                    fs::create_dir(&selected).unwrap();
                    None
                }
                "socket" => Some(std::os::unix::net::UnixListener::bind(&selected).unwrap()),
                _ => {
                    std::os::unix::fs::symlink(
                        match kind {
                            "file-link" => &victim,
                            "directory-link"
                            | "directory-link-slash"
                            | "directory-link-slashes" => &directory,
                            _ => &missing,
                        },
                        &selected,
                    )
                    .unwrap();
                    None
                }
            };
            let suffix = match kind {
                "directory-link-slash" => "/",
                "directory-link-slashes" => "///",
                _ => "",
            };
            match sftp
                .remove_path(format!("{}{suffix}", selected.display()))
                .await
            {
                Ok(()) => assert!(
                    matches!(
                        fs::symlink_metadata(&selected),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound
                    ),
                    "removed {kind} entry"
                ),
                Err(error) => {
                    failures.push(format!("{kind}: {error}"));
                    assert!(fs::symlink_metadata(&selected).is_ok());
                    fs::remove_file(&selected).unwrap();
                }
            }
            drop(socket);
            assert_eq!(fs::read(&victim).unwrap(), b"unchanged file target");
            assert_eq!(
                fs::metadata(&victim).unwrap().permissions().mode() & 0o7777,
                0o640
            );
            assert_eq!(
                fs::read(directory.join("child")).unwrap(),
                b"unchanged directory child"
            );
            assert_eq!(
                fs::metadata(&directory).unwrap().permissions().mode() & 0o7777,
                0o750
            );
            assert!(!missing.exists());
        }
        sftp.close().await.unwrap();
        connection.disconnect().await.unwrap();
        assert!(failures.is_empty(), "entry deletion failures: {failures:?}");
    });
}

#[test]
fn remote_permission_changes_require_an_explicit_non_link_target() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let fixture = LocalSshd::start().unwrap();
        wait_for_port(fixture.port).await;
        let connection = SshConnection::connect(fixture.options()).await.unwrap();
        let sftp = connection.open_sftp().await.unwrap();
        let victim = fixture.directory.path().join("permission-victim");
        let directory = fixture.directory.path().join("permission-directory");
        let missing = fixture.directory.path().join("permission-missing");
        let selected = fixture.directory.path().join("permission-link");
        fs::write(&victim, b"unchanged permission target").unwrap();
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("child"), b"unchanged permission child").unwrap();
        let mut failures = Vec::new();
        for (kind, target, suffix) in [
            ("file", &victim, ""),
            ("directory", &directory, ""),
            ("directory-slash", &directory, "/"),
            ("directory-slashes", &directory, "///"),
            ("dangling", &missing, ""),
        ] {
            fs::set_permissions(&victim, fs::Permissions::from_mode(0o600)).unwrap();
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
            std::os::unix::fs::symlink(target, &selected).unwrap();
            let result = sftp
                .set_permissions(format!("{}{suffix}", selected.display()), 0o777)
                .await;
            let file_mode = fs::metadata(&victim).unwrap().permissions().mode() & 0o7777;
            let directory_mode = fs::metadata(&directory).unwrap().permissions().mode() & 0o7777;
            if !matches!(result, Err(SshError::RemotePermissionsTargetSymlink))
                || file_mode != 0o600
                || directory_mode != 0o700
            {
                failures.push(format!(
                    "{kind}: {result:?}, file={file_mode:o}, directory={directory_mode:o}"
                ));
            }
            assert_eq!(fs::read_link(&selected).unwrap(), *target);
            assert_eq!(fs::read(&victim).unwrap(), b"unchanged permission target");
            assert_eq!(
                fs::read(directory.join("child")).unwrap(),
                b"unchanged permission child"
            );
            assert!(!missing.exists());
            fs::remove_file(&selected).unwrap();
        }
        // Chmod does not require reading a mode-000 file or opening a socket.
        fs::set_permissions(&victim, fs::Permissions::from_mode(0o000)).unwrap();
        sftp.set_permissions(victim.to_str().unwrap(), 0o640)
            .await
            .unwrap();
        sftp.set_permissions(format!("{}///", directory.display()), 0o750)
            .await
            .unwrap();
        let socket_path = fixture.directory.path().join("permission-socket");
        let socket = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        sftp.set_permissions(socket_path.to_str().unwrap(), 0o700)
            .await
            .unwrap();
        assert!(
            sftp.set_permissions(victim.to_str().unwrap(), 0o10000)
                .await
                .is_err()
        );
        assert!(matches!(
            sftp.set_permissions(missing.to_str().unwrap(), 0o600).await,
            Err(SshError::SftpPathMissing)
        ));
        assert_eq!(
            fs::metadata(&victim).unwrap().permissions().mode() & 0o7777,
            0o640
        );
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o7777,
            0o750
        );
        assert_eq!(
            fs::metadata(&socket_path).unwrap().permissions().mode() & 0o7777,
            0o700
        );
        assert_eq!(fs::read(&victim).unwrap(), b"unchanged permission target");
        assert_eq!(
            fs::read(directory.join("child")).unwrap(),
            b"unchanged permission child"
        );
        drop(socket);
        sftp.close().await.unwrap();
        connection.disconnect().await.unwrap();
        assert!(
            failures.is_empty(),
            "permission target failures: {failures:?}"
        );
    });
}

#[test]
fn download_source_guards_keep_regular_files_and_file_symlinks_working() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let fixture = LocalSshd::start().unwrap();
        wait_for_port(fixture.port).await;
        let connection = SshConnection::connect(fixture.options()).await.unwrap();
        let sftp = connection.open_sftp().await.unwrap();
        let source = fixture.directory.path().join("résumé source.txt");
        let payload = vec![b'D'; 128 * 1024];
        fs::write(&source, &payload).unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();
        let link = fixture.directory.path().join("file-link");
        std::os::unix::fs::symlink(&source, &link).unwrap();
        for path in [&source, &link] {
            assert_eq!(
                sftp.file_info(path.to_str().unwrap()).await.unwrap(),
                (Some(payload.len() as u64), false)
            );
            let mut downloaded = Vec::new();
            assert_eq!(
                sftp.download_to(path.to_str().unwrap(), &mut downloaded)
                    .await
                    .unwrap(),
                payload.len() as u64
            );
            assert_eq!(downloaded, payload);
            downloaded.clear();
            let (_sender, mut cancel) = oneshot::channel();
            let mut progress = 0;
            assert_eq!(
                sftp.download_to_with_cancel(
                    path.to_str().unwrap(),
                    &mut downloaded,
                    &mut cancel,
                    |bytes| progress = bytes
                )
                .await
                .unwrap(),
                payload.len() as u64
            );
            assert_eq!(downloaded, payload);
            assert_eq!(progress, payload.len() as u64);
            let (sender, mut cancel) = oneshot::channel();
            sender.send(()).unwrap();
            downloaded.clear();
            assert!(matches!(
                sftp.download_to_with_cancel(
                    path.to_str().unwrap(),
                    &mut downloaded,
                    &mut cancel,
                    |_| {}
                )
                .await,
                Err(SshError::Cancelled)
            ));
            assert!(downloaded.is_empty());
        }
        let directory = fixture.directory.path().join("download-directory");
        fs::create_dir(&directory).unwrap();
        let directory_link = fixture.directory.path().join("directory-link");
        std::os::unix::fs::symlink(&directory, &directory_link).unwrap();
        for path in [&directory, &directory_link] {
            assert!(sftp.file_info(path.to_str().unwrap()).await.unwrap().1);
            let mut downloaded = Vec::new();
            assert!(matches!(
                sftp.download_to(path.to_str().unwrap(), &mut downloaded)
                    .await,
                Err(SshError::RemoteDownloadSourceDirectory)
            ));
            assert!(downloaded.is_empty());
        }
        let missing = fixture.directory.path().join("missing-download-target");
        let dangling = fixture.directory.path().join("dangling-link");
        std::os::unix::fs::symlink(&missing, &dangling).unwrap();
        let mut downloaded = Vec::new();
        assert!(matches!(
            sftp.download_to(dangling.to_str().unwrap(), &mut downloaded)
                .await,
            Err(SshError::SftpPathMissing)
        ));
        let socket = fixture.directory.path().join("download-socket");
        let _socket = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        assert!(matches!(
            sftp.file_info(socket.to_str().unwrap()).await,
            Err(SshError::RemoteDownloadSourceUnsupported)
        ));
        assert!(matches!(
            sftp.download_to(socket.to_str().unwrap(), &mut downloaded)
                .await,
            Err(SshError::RemoteDownloadSourceUnsupported)
        ));
        assert!(downloaded.is_empty());
        assert_eq!(fs::read(&source).unwrap(), payload);
        assert_eq!(
            fs::metadata(&source).unwrap().permissions().mode() & 0o7777,
            0o640
        );
        assert_eq!(fs::read_link(&link).unwrap(), source);
        assert_eq!(fs::read_link(&directory_link).unwrap(), directory);
        assert_eq!(fs::read_link(&dangling).unwrap(), missing);
        assert!(!missing.exists());
        sftp.close().await.unwrap();
        connection.disconnect().await.unwrap();
    });
}

#[test]
fn sftp_transfer_parts_are_private_during_copy_and_cleaned_on_errors() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let fixture = LocalSshd::start().unwrap();
        wait_for_port(fixture.port).await;
        let connection = SshConnection::connect(fixture.options()).await.unwrap();
        let sftp = connection.open_sftp().await.unwrap();
        let temporary = fixture.directory.path().join("private-upload.part");
        let part = temporary.to_str().unwrap();
        let payload = vec![b'P'; 128 * 1024];
        let (_sender, mut cancel) = oneshot::channel();
        let mut source = &payload[..];
        let mut progress = 0;
        assert_eq!(
            sftp.upload_temporary_from_with_cancel(&mut source, part, &mut cancel, |bytes| {
                assert_eq!(
                    fs::metadata(&temporary).unwrap().permissions().mode() & 0o7777,
                    0o600
                );
                progress = bytes;
            })
            .await
            .unwrap(),
            payload.len() as u64
        );
        assert_eq!(progress, payload.len() as u64);
        assert_eq!(fs::read(&temporary).unwrap(), payload);
        sftp.remove_file(part).await.unwrap();

        let (sender, mut cancel) = oneshot::channel();
        sender.send(()).unwrap();
        let mut source = &payload[..];
        assert!(matches!(
            sftp.upload_temporary_from_with_cancel(&mut source, part, &mut cancel, |_| {})
                .await,
            Err(SshError::Cancelled)
        ));
        assert!(!temporary.exists());

        let (sender, mut cancel) = oneshot::channel();
        let mut sender = Some(sender);
        let mut source = &payload[..];
        assert!(matches!(
            sftp.upload_temporary_from_with_cancel(&mut source, part, &mut cancel, |_| {
                assert_eq!(
                    fs::metadata(&temporary).unwrap().permissions().mode() & 0o7777,
                    0o600
                );
                if let Some(sender) = sender.take() {
                    sender.send(()).unwrap();
                }
            })
            .await,
            Err(SshError::Cancelled)
        ));
        assert!(!temporary.exists());

        let original = fixture.directory.path().join("read-failure-source");
        fs::write(&original, b"original source").unwrap();
        let mut source = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&original)
            .await
            .unwrap();
        let (_sender, mut cancel) = oneshot::channel();
        assert!(matches!(
            sftp.upload_temporary_from_with_cancel(&mut source, part, &mut cancel, |_| {})
                .await,
            Err(SshError::LocalIo(_))
        ));
        assert!(!temporary.exists());
        assert_eq!(fs::read(&original).unwrap(), b"original source");
        sftp.close().await.unwrap();
        connection.disconnect().await.unwrap();
    });
}

#[test]
fn sftp_transfer_parts_refuse_unowned_paths_without_truncation() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let fixture = LocalSshd::start().unwrap();
        wait_for_port(fixture.port).await;
        let connection = SshConnection::connect(fixture.options()).await.unwrap();
        let sftp = connection.open_sftp().await.unwrap();
        let victim = fixture.directory.path().join("victim");
        let directory = fixture.directory.path().join("victim-directory");
        let missing = fixture.directory.path().join("missing-victim");
        fs::write(&victim, b"unchanged victim").unwrap();
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("child"), b"unchanged child").unwrap();
        let occupied = fixture.directory.path().join("occupied.part");
        for kind in [
            "regular",
            "file-link",
            "directory-link",
            "dangling-link",
            "directory",
        ] {
            match kind {
                "regular" => fs::write(&occupied, b"unowned original").unwrap(),
                "directory" => fs::create_dir(&occupied).unwrap(),
                _ => std::os::unix::fs::symlink(
                    match kind {
                        "file-link" => &victim,
                        "directory-link" => &directory,
                        _ => &missing,
                    },
                    &occupied,
                )
                .unwrap(),
            }
            let old_mode = fs::symlink_metadata(&occupied)
                .unwrap()
                .permissions()
                .mode();
            let (_sender, mut cancel) = oneshot::channel();
            assert!(
                sftp.prepare_upload_temporary(occupied.to_str().unwrap(), &mut cancel)
                    .await
                    .is_err(),
                "SCP reservation must refuse {kind}"
            );
            let mut source = &b"replacement"[..];
            assert!(
                sftp.upload_temporary_from_with_cancel(
                    &mut source,
                    occupied.to_str().unwrap(),
                    &mut cancel,
                    |_| {}
                )
                .await
                .is_err(),
                "{kind}"
            );
            for reserve_only in [false, true] {
                let (sender, mut cancel) = oneshot::channel();
                sender.send(()).unwrap();
                let result = if reserve_only {
                    sftp.prepare_upload_temporary(occupied.to_str().unwrap(), &mut cancel)
                        .await
                } else {
                    sftp.upload_temporary_from_with_cancel(
                        &mut source,
                        occupied.to_str().unwrap(),
                        &mut cancel,
                        |_| {},
                    )
                    .await
                    .map(|_| ())
                };
                assert!(matches!(result, Err(SshError::Cancelled)));
            }
            assert_eq!(
                fs::symlink_metadata(&occupied)
                    .unwrap()
                    .permissions()
                    .mode(),
                old_mode
            );
            if kind == "regular" {
                assert_eq!(fs::read(&occupied).unwrap(), b"unowned original");
            } else if kind != "directory" {
                let expected = match kind {
                    "file-link" => &victim,
                    "directory-link" => &directory,
                    _ => &missing,
                };
                assert_eq!(fs::read_link(&occupied).unwrap(), *expected);
            }
            assert_eq!(fs::read(&victim).unwrap(), b"unchanged victim");
            assert_eq!(
                fs::read(directory.join("child")).unwrap(),
                b"unchanged child"
            );
            assert!(!missing.exists());
            if kind == "directory" {
                fs::remove_dir(&occupied).unwrap();
            } else {
                fs::remove_file(&occupied).unwrap();
            }
        }
        sftp.close().await.unwrap();
        connection.disconnect().await.unwrap();
    });
}

#[test]
fn scp_transfer_parts_are_private_during_copy() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let fixture = LocalSshd::start().unwrap();
        wait_for_port(fixture.port).await;
        let connection = SshConnection::connect(fixture.options()).await.unwrap();
        let temporary = fixture.directory.path().join("private-scp.part");
        let payload = vec![b'S'; 128 * 1024];
        let (_sender, mut cancel) = oneshot::channel();
        let mut source = &payload[..];
        assert_eq!(
            connection
                .scp_upload_with_cancel(
                    temporary.to_str().unwrap(),
                    payload.len() as u64,
                    &mut source,
                    &mut cancel,
                    |_| {
                        assert_eq!(
                            fs::metadata(&temporary).unwrap().permissions().mode() & 0o7777,
                            0o600
                        );
                    }
                )
                .await
                .unwrap(),
            payload.len() as u64
        );
        assert_eq!(fs::read(&temporary).unwrap(), payload);
        let sftp = connection.open_sftp().await.unwrap();
        sftp.remove_file(temporary.to_str().unwrap()).await.unwrap();
        let (_sender, mut cancel) = oneshot::channel();
        sftp.prepare_upload_temporary(temporary.to_str().unwrap(), &mut cancel)
            .await
            .unwrap();
        assert_eq!(
            fs::metadata(&temporary).unwrap().permissions().mode() & 0o7777,
            0o600
        );
        let mut source = &payload[..];
        connection
            .scp_upload_with_cancel(
                temporary.to_str().unwrap(),
                payload.len() as u64,
                &mut source,
                &mut cancel,
                |_| {
                    assert_eq!(
                        fs::metadata(&temporary).unwrap().permissions().mode() & 0o7777,
                        0o600
                    );
                },
            )
            .await
            .unwrap();
        let destination = fixture.directory.path().join("scp-original");
        fs::write(&destination, b"old contents").unwrap();
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o640)).unwrap();
        sftp.promote_uploaded_file(
            temporary.to_str().unwrap(),
            destination.to_str().unwrap(),
            true,
        )
        .await
        .unwrap();
        assert_eq!(fs::read(&destination).unwrap(), payload);
        assert_eq!(
            fs::metadata(&destination).unwrap().permissions().mode() & 0o7777,
            0o640
        );
        assert!(!temporary.exists());
        sftp.close().await.unwrap();
        connection.disconnect().await.unwrap();
    });
}

#[test]
fn upload_replacement_replaces_links_without_following_their_targets() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let fixture = LocalSshd::start().unwrap();
        wait_for_port(fixture.port).await;
        let connection = SshConnection::connect(fixture.options()).await.unwrap();
        let sftp = connection.open_sftp().await.unwrap();
        let target_file = fixture.directory.path().join("link-target.txt");
        let target_directory = fixture.directory.path().join("link-target-directory");
        let missing_target = fixture.directory.path().join("missing-link-target");
        fs::write(&target_file, b"unrelated file").unwrap();
        fs::create_dir(&target_directory).unwrap();
        fs::write(
            target_directory.join("child.txt"),
            b"unrelated directory child",
        )
        .unwrap();
        let destination = fixture.directory.path().join("link-destination");
        let temporary = fixture.directory.path().join("complete-upload.part");
        let remote = destination.to_str().unwrap();
        let part = temporary.to_str().unwrap();
        for target in [&target_file, &target_directory, &missing_target] {
            for cancellable in [false, true] {
                std::os::unix::fs::symlink(target, &destination).unwrap();
                assert_eq!(sftp.check_upload_destination(remote).await.unwrap(), None);
                for cancel_upload in [false, true] {
                    sftp.upload_from(&b"new complete file"[..], part)
                        .await
                        .unwrap();
                    let result = if cancel_upload {
                        let (sender, mut cancel) = oneshot::channel();
                        sender.send(()).unwrap();
                        sftp.promote_uploaded_file_with_cancel(part, remote, true, &mut cancel)
                            .await
                    } else {
                        sftp.promote_uploaded_file(part, remote, false).await
                    };
                    assert!(result.is_err());
                    assert_eq!(fs::read_link(&destination).unwrap(), *target);
                    assert!(!temporary.exists());
                }
                sftp.upload_from(&b"new complete file"[..], part)
                    .await
                    .unwrap();
                sftp.set_permissions(part, 0o600).await.unwrap();
                if cancellable {
                    let (_sender, mut cancel) = oneshot::channel();
                    sftp.promote_uploaded_file_with_cancel(part, remote, true, &mut cancel)
                        .await
                        .expect("explicit overwrite replaces the link itself");
                } else {
                    sftp.promote_uploaded_file(part, remote, true)
                        .await
                        .expect("explicit overwrite replaces the link itself");
                }
                assert!(fs::symlink_metadata(&destination).unwrap().is_file());
                assert_eq!(fs::read(&destination).unwrap(), b"new complete file");
                assert_eq!(
                    fs::metadata(&destination).unwrap().permissions().mode() & 0o7777,
                    0o600
                );
                assert_eq!(fs::read(&target_file).unwrap(), b"unrelated file");
                assert_eq!(
                    fs::read(target_directory.join("child.txt")).unwrap(),
                    b"unrelated directory child"
                );
                assert!(!missing_target.exists());
                assert!(!temporary.exists());
                fs::remove_file(&destination).unwrap();
            }
        }
        sftp.upload_from(&b"new complete file"[..], part)
            .await
            .unwrap();
        assert!(matches!(
            sftp.check_upload_destination(target_directory.to_str().unwrap())
                .await,
            Err(SshError::RemoteUploadDestinationDirectory)
        ));
        assert!(
            sftp.promote_uploaded_file(part, target_directory.to_str().unwrap(), true)
                .await
                .is_err()
        );
        assert!(target_directory.is_dir());
        assert_eq!(
            fs::read(target_directory.join("child.txt")).unwrap(),
            b"unrelated directory child"
        );
        assert!(!temporary.exists());
        assert!(
            fs::read_dir(fixture.directory.path())
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".mobarust-upload-backup-"))
        );
        sftp.close().await.unwrap();
        connection.disconnect().await.unwrap();
    });
}

#[test]
fn cancelled_upload_promotion_preserves_original_and_removes_complete_part() {
    use std::future::{Future, poll_fn};
    use std::task::Poll;

    let runtime = tokio::runtime::Runtime::new().expect("create SSH test runtime");
    runtime.block_on(async {
        let fixture = LocalSshd::start().expect("start loopback SSH fixture");
        wait_for_port(fixture.port).await;
        let connection = SshConnection::connect(fixture.options()).await.unwrap();
        let sftp = connection.open_sftp().await.unwrap();
        let destination = fixture.directory.path().join("promotion-original.txt");
        let temporary = fixture.directory.path().join("promotion-complete.part");
        let remote = destination.to_string_lossy();
        let part = temporary.to_string_lossy();

        for timing in ["before", "metadata", "sender_closed"] {
            for overwrite in [false, true] {
                if overwrite {
                    fs::write(&destination, b"original bytes").unwrap();
                } else if destination.exists() {
                    fs::remove_file(&destination).unwrap();
                }
                sftp.upload_from(&b"replacement bytes"[..], part.as_ref())
                    .await
                    .unwrap();
                let (sender, mut cancel) = oneshot::channel();
                let mut sender = Some(sender);
                if timing == "before" {
                    sender.take().unwrap().send(()).unwrap();
                }
                if timing == "sender_closed" {
                    drop(sender.take());
                }
                let mut promotion = Box::pin(sftp.promote_uploaded_file_with_cancel(
                    &part,
                    &remote,
                    overwrite,
                    &mut cancel,
                ));
                if timing == "metadata" {
                    // Poll once into the real SFTP metadata request, then cancel.
                    // The second guard must run before the first destination rename.
                    poll_fn(|cx| {
                        assert!(promotion.as_mut().poll(cx).is_pending());
                        Poll::Ready(())
                    })
                    .await;
                    sender.take().unwrap().send(()).unwrap();
                }
                let result = tokio::time::timeout(Duration::from_secs(5), promotion)
                    .await
                    .expect("cancelled promotion deadline");
                assert!(
                    matches!(result, Err(SshError::Cancelled)),
                    "{timing}: {result:?}"
                );
                if overwrite {
                    assert_eq!(fs::read(&destination).unwrap(), b"original bytes");
                } else {
                    assert!(!destination.exists());
                }
                assert!(
                    !temporary.exists(),
                    "cancelled complete part must be removed"
                );
                assert!(
                    !fs::read_dir(fixture.directory.path())
                        .unwrap()
                        .any(|entry| {
                            entry
                                .unwrap()
                                .file_name()
                                .to_string_lossy()
                                .contains("mobarust-upload-backup-")
                        })
                );
            }
        }
        // Invalid same-path input must never make cancellation delete the original.
        let (sender, mut cancel) = oneshot::channel();
        sender.send(()).unwrap();
        assert!(
            sftp.promote_uploaded_file_with_cancel(&remote, &remote, true, &mut cancel)
                .await
                .is_err()
        );
        assert_eq!(fs::read(&destination).unwrap(), b"original bytes");
        sftp.close().await.unwrap();
        connection.disconnect().await.unwrap();
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
            Err(error) => panic!("expected host-key rejection, received: {error}"),
            Ok(_) => panic!("unknown host key was accepted"),
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
            .expect("save remote text document with a recovery copy");
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
                    let xauth = fs::read_to_string(fixture.directory.path().join("xauth-progress"))
                        .unwrap_or_else(|_| "not invoked".to_owned());
                    panic!("wait for SSH X11 channel; xauth={xauth}: {}", String::from_utf8_lossy(&shell_output));
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

/// Manual GUI fixture only; passing this harness is not native acceptance.
#[tokio::test]
#[ignore = "manual native editor lab; generated loopback keys/files; fifteen-minute deadline"]
async fn native_file_editor_lab() {
    use mobarust_core::{AuthMethod, Protocol, SessionRecord};
    use std::os::unix::fs::OpenOptionsExt;

    let fixture = LocalSshd::start().expect("start disposable native file lab");
    assert_eq!(
        fixture
            .directory
            .path()
            .metadata()
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    wait_for_port(fixture.port).await;
    let root = fixture.directory.path().to_owned();
    let files = root.join("files");
    fs::create_dir(&files).unwrap();
    fs::set_permissions(&files, fs::Permissions::from_mode(0o700)).unwrap();
    for (name, bytes) in [
        ("app.conf", b"greeting=initial\n".as_slice()),
        ("occupied.conf", b"preserve=occupied\n".as_slice()),
        ("legacy.txt", b"caf\xe9\n".as_slice()),
    ] {
        fs::write(files.join(name), bytes).unwrap();
        fs::set_permissions(files.join(name), fs::Permissions::from_mode(0o640)).unwrap();
    }
    let mut profile = SessionRecord::local_terminal("Disposable editor lab");
    profile.protocol = Protocol::Ssh;
    profile.hostname = "127.0.0.1".into();
    profile.port = fixture.port;
    profile.username = Some(fixture.username.clone());
    profile.auth = AuthMethod::PrivateKey {
        key_ref: fixture.client_key.to_string_lossy().into_owned(),
        credential_ref: None,
    };
    profile.known_hosts_path = Some(fixture.known_hosts.to_string_lossy().into_owned());
    profile.folder = Some("Native acceptance lab".into());
    profile.tags = vec!["loopback".into()];
    profile.favorite = false;
    profile.notes = Some("Generated loopback OpenSSH keys/files; disposable HOME.".into());
    profile.validate().unwrap();
    for (name, value) in [
        (
            "profiles.json",
            serde_json::json!({"schema_version": 1, "sessions": [profile]}),
        ),
        (
            "file_lab.json",
            serde_json::json!({"root": root, "files": files, "host": "127.0.0.1", "port": fixture.port, "sshd_pid": fixture.child.id(), "stop": root.join("stop")}),
        ),
    ] {
        let file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(root.join(name))
            .unwrap();
        serde_json::to_writer(file, &value).unwrap();
    }
    eprintln!(
        "Native editor lab metadata: {}",
        root.join("file_lab.json").display()
    );
    eprintln!(
        "Import secret-free profiles: {}",
        root.join("profiles.json").display()
    );
    eprintln!(
        "Close the owned app before creating the stop marker; this harness does not assert GUI results."
    );
    let stopped = tokio::time::timeout(Duration::from_secs(900), async {
        while !root.join("stop").exists() {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await;
    let port = fixture.port;
    drop(fixture);
    assert!(!root.exists(), "native file lab state must be removed");
    drop(TcpListener::bind(("127.0.0.1", port)).await.unwrap());
    stopped.expect("manual native file lab deadline");
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
    fn ipv6_options(&self) -> SshConnectOptions {
        let trust = self.directory.path().join("known_hosts_ipv6");
        fs::write(
            &trust,
            fs::read_to_string(&self.known_hosts)
                .unwrap()
                .replace("[127.0.0.1]", "[::1]"),
        )
        .expect("write IPv6 fixture trust file");
        let mut options = self.options();
        options.host = "::1".into();
        options.host_key_policy = HostKeyPolicy::KnownHosts(trust);
        options
    }

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
        let directory = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()?;
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
            let progress = directory.path().join("xauth-progress");
            // Relay commands unchanged; record stages, never displays or cookies.
            fs::write(
                &wrapper,
                format!(
                    r#"#!/bin/sh
umask 077
progress='{progress}'
printf 'started\n' >> "$progress"
{{ while IFS= read -r xauth_line; do
case "$xauth_line" in
'remove unix:'*|'add unix:'*) printf 'unix-command\n' >> "$progress";;
*) printf 'other-command\n' >> "$progress";;
esac
printf '%s\n' "$xauth_line"
done
printf 'input-ended\n' >> "$progress"
}} | '{xauth}' -q -f '{authority}' -
xauth_result=$?
printf 'finished=%s\n' "$xauth_result" >> "$progress"
exit "$xauth_result"
"#,
                    progress = progress.to_string_lossy().replace('\'', "'\\''"),
                    xauth = xauth.to_string_lossy().replace('\'', "'\\''"),
                    authority = authority.to_string_lossy().replace('\'', "'\\''"),
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
                "Port {port}\nListenAddress 127.0.0.1\n{ipv6_listener}HostKey {}\nAuthorizedKeysFile {}\nPidFile \"{home}/sshd.pid\"\nSubsystem sftp internal-sftp -d \"{home}\"\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPubkeyAuthentication yes\nPermitRootLogin no\nPermitUserRC no\nPermitUserEnvironment no\nUsePAM no\nStrictModes no\nAllowTcpForwarding yes\nAcceptEnv MOBARUST_FIXTURE\nSetEnv \"HOME={home}\" \"ZDOTDIR={home}\" \"XDG_CONFIG_HOME={home}\" \"XAUTHORITY={home}/.Xauthority\" BASH_ENV=/dev/null ENV=/dev/null\n{x11_config}AllowUsers {username}\nPrintMotd no\nUseDNS no\nLogLevel QUIET\n",
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
