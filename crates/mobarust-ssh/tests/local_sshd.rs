#![cfg(unix)]

use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use mobarust_ssh::{
    HostKeyPolicy, SshConnectOptions, SshConnection, SshCredentials, SshError, SshOutput,
};
use tokio::net::TcpStream;
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
        sftp.close().await.expect("close SFTP subsystem");
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
                "Port {port}\nListenAddress 127.0.0.1\nHostKey {}\nAuthorizedKeysFile {}\nSubsystem sftp internal-sftp\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPubkeyAuthentication yes\nPermitRootLogin no\nUsePAM no\nStrictModes no\nAllowUsers {username}\nPrintMotd no\nUseDNS no\nLogLevel QUIET\n",
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
