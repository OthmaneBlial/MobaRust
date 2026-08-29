#![cfg(unix)]

use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use mobarust_ssh::{
    HostKeyPolicy, SshConnectOptions, SshConnection, SshCredentials, SshError, SshOutput,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

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
            credentials: SshCredentials::private_key(
                fixture.username.clone(),
                fixture.client_key.clone(),
                None::<String>,
            ),
        })
        .await;
        match rejection {
            Err(SshError::HostKeyRejected { fingerprint }) => {
                assert!(fingerprint.starts_with("SHA256:"));
            }
            _ => panic!("unknown host key was not rejected"),
        }

        let connection = SshConnection::connect(SshConnectOptions {
            host: "127.0.0.1".into(),
            port: fixture.port,
            host_key_policy: HostKeyPolicy::KnownHosts(fixture.known_hosts.clone()),
            timeout: Duration::from_secs(5),
            credentials: SshCredentials::private_key(
                fixture.username.clone(),
                fixture.client_key.clone(),
                None::<String>,
            ),
        })
        .await
        .expect("connect to local sshd");

        assert_eq!(
            connection.state(),
            mobarust_core::ConnectionState::Connected
        );
        let shell = connection.open_shell(100, 30).await.expect("open SSH PTY");
        let (mut reader, writer) = shell.split();
        writer.resize(120, 40).await.expect("resize SSH PTY");
        writer
            .write(b"printf 'MOBARUST_SSH_OK\n'; exit\n")
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
                SshOutput::ExitStatus(_) => break,
                SshOutput::Control => {}
            }
        }

        assert!(String::from_utf8_lossy(&output).contains("MOBARUST_SSH_OK"));
        let source = fixture.directory.path().join("source.bin");
        let downloaded = fixture.directory.path().join("downloaded.bin");
        fs::write(&source, vec![b'R'; 128 * 1024]).expect("write upload source");
        let sftp = connection.open_sftp().await.expect("open SFTP subsystem");
        let remote_path = format!("/tmp/mobarust-sftp-{}", std::process::id());
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
        let entries = sftp.read_dir("/tmp").await.expect("list remote directory");
        assert!(entries.iter().any(|entry| entry.path == remote_path));
        assert!(
            sftp.try_exists(&remote_path)
                .await
                .expect("check remote file")
        );
        assert_eq!(
            sftp.file_info(&remote_path)
                .await
                .expect("read remote file info"),
            (128 * 1024, false)
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
        let directory_path = format!("/tmp/mobarust-directory-{}", std::process::id());
        sftp.create_dir(&directory_path)
            .await
            .expect("create remote directory");
        assert!(
            sftp.file_info(&directory_path)
                .await
                .expect("read directory info")
                .1
        );
        sftp.remove_dir(&directory_path)
            .await
            .expect("remove remote directory");
        sftp.remove_file(&renamed_path)
            .await
            .expect("remove fixture file");
        assert!(!sftp.try_exists(&renamed_path).await.expect("check removal"));

        let editor_source = fixture.directory.path().join("editor-source.txt");
        fs::write(&editor_source, "before\neditor fixture\n").expect("write editor source");
        let editor_path = format!("/tmp/mobarust-editor-{}.txt", std::process::id());
        sftp.upload_from(
            tokio::fs::File::open(&editor_source)
                .await
                .expect("open editor source"),
            &editor_path,
        )
        .await
        .expect("upload editor fixture");
        let document = sftp
            .read_text_document(&editor_path)
            .await
            .expect("read bounded remote text document");
        assert_eq!(document.content, "before\neditor fixture\n");
        let saved = sftp
            .save_text_document(&editor_path, &document.revision, "after\n")
            .await
            .expect("atomically save remote text document");
        assert_eq!(saved.content, "after\n");
        assert!(matches!(
            sftp.save_text_document(&editor_path, &document.revision, "stale\n")
                .await,
            Err(SshError::RemoteConflict)
        ));
        sftp.remove_file(&editor_path)
            .await
            .expect("remove editor fixture");
        sftp.close().await.expect("close SFTP subsystem");

        let scp_remote_path = format!("/tmp/mobarust-scp-{}", std::process::id());
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

        let jumped = SshConnection::connect_with_jump_chain(
            SshConnectOptions {
                host: "127.0.0.1".into(),
                port: fixture.port,
                host_key_policy: HostKeyPolicy::KnownHosts(fixture.known_hosts.clone()),
                timeout: Duration::from_secs(5),
                credentials: SshCredentials::private_key(
                    fixture.username.clone(),
                    fixture.client_key.clone(),
                    None::<String>,
                ),
            },
            vec![SshConnectOptions {
                host: "127.0.0.1".into(),
                port: fixture.port,
                host_key_policy: HostKeyPolicy::KnownHosts(fixture.known_hosts.clone()),
                timeout: Duration::from_secs(5),
                credentials: SshCredentials::private_key(
                    fixture.username.clone(),
                    fixture.client_key.clone(),
                    None::<String>,
                ),
            }],
        )
        .await
        .expect("connect through a real SSH jump host");
        let jumped_shell = jumped.open_shell(100, 30).await.expect("open jumped PTY");
        let (mut jumped_reader, jumped_writer) = jumped_shell.split();
        jumped_writer
            .write(b"printf 'MOBARUST_JUMP_OK\\n'\\n")
            .await
            .expect("write through jumped shell");
        let mut jumped_output = Vec::new();
        while let Some(message) =
            tokio::time::timeout(Duration::from_secs(5), jumped_reader.next_output())
                .await
                .expect("jumped shell output timeout")
        {
            match message.expect("jumped shell output error") {
                SshOutput::Stdout(bytes) | SshOutput::Stderr(bytes) => {
                    jumped_output.extend(bytes);
                    if String::from_utf8_lossy(&jumped_output).contains("MOBARUST_JUMP_OK") {
                        break;
                    }
                }
                SshOutput::ExitStatus(_) => break,
                SshOutput::Control => {}
            }
        }
        assert!(String::from_utf8_lossy(&jumped_output).contains("MOBARUST_JUMP_OK"));
        drop(jumped_reader);
        drop(jumped_writer);
        jumped
            .disconnect()
            .await
            .expect("disconnect jumped SSH fixture");

        connection
            .disconnect()
            .await
            .expect("disconnect SSH fixture");
    });
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
    fn start() -> Result<Self, Box<dyn std::error::Error>> {
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

        let username = std::env::var("USER")?;
        let config = directory.path().join("sshd_config");
        fs::write(
            &config,
            format!(
                "Port {port}\nListenAddress 127.0.0.1\nHostKey {}\nAuthorizedKeysFile {}\nSubsystem sftp internal-sftp\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPubkeyAuthentication yes\nPermitRootLogin no\nUsePAM no\nStrictModes no\nAllowTcpForwarding yes\nAllowUsers {username}\nPrintMotd no\nUseDNS no\nLogLevel QUIET\n",
                host_key.display(),
                authorized_keys.display(),
            ),
        )?;

        let sshd = if Path::new("/usr/sbin/sshd").exists() {
            "/usr/sbin/sshd"
        } else {
            "/usr/local/sbin/sshd"
        };
        let child = Command::new(sshd)
            .args(["-D", "-e", "-f"])
            .arg(&config)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()?;

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

impl Drop for LocalSshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = self.directory.path();
    }
}

fn run_keygen(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let status = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", ""])
        .arg("-f")
        .arg(path)
        .status()?;
    assert!(status.success(), "ssh-keygen failed for {}", path.display());
    Ok(())
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
