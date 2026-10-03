//! Scripted SFTP faults over authenticated loopback SSH. Files and keys stay in memory.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mobarust_ssh::{
    HostKeyPolicy, RemoteTextEncoding, SshConnectOptions, SshConnection, SshCredentials, SshError,
};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::keys::{HashAlg, PrivateKey};
use russh::server::{self, Auth};
use russh_sftp::protocol::{Attrs, Data, FileAttributes, Handle, OpenFlags, Status, StatusCode};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use uuid::Uuid;
use zeroize::Zeroizing;

const DEADLINE: Duration = Duration::from_secs(10);
const TARGET: &str = "/document.txt";
const ORIGINAL: &[u8] = b"original";
const WRITTEN: &str = "saved café\n";
const EXTERNAL: &[u8] = b"another writer after promotion";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Fault {
    None,
    MissingTypeMetadata,
    RereadDenied,
    ConcurrentWrite,
    BackupCleanupDenied,
}

#[derive(Clone)]
struct File {
    bytes: Vec<u8>,
    mode: u32,
}

struct State {
    files: HashMap<String, File>,
    fault: Fault,
    promoted: bool,
    post_promotion_reads: usize,
    writes: usize,
    renames: usize,
}

struct Sftp {
    state: Arc<Mutex<State>>,
    ended: Option<oneshot::Sender<()>>,
}

fn ok(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: String::new(),
        language_tag: String::new(),
    }
}

impl russh_sftp::server::Handler for Sftp {
    type Error = StatusCode;
    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let mut state = self.state.lock().unwrap();
        if state.promoted && path == TARGET {
            state.post_promotion_reads += 1;
            if state.fault == Fault::RereadDenied {
                return Err(StatusCode::PermissionDenied);
            }
        }
        let file = state.files.get(&path).ok_or(StatusCode::NoSuchFile)?;
        let mut attrs = FileAttributes::empty();
        attrs.size = Some(file.bytes.len() as u64);
        attrs.permissions = if state.fault == Fault::MissingTypeMetadata && path == TARGET {
            None
        } else {
            Some(file.mode)
        };
        attrs.mtime = Some(123);
        Ok(Attrs { id, attrs })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.lstat(id, path).await
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        flags: OpenFlags,
        attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let mut state = self.state.lock().unwrap();
        if flags.contains(OpenFlags::CREATE) {
            assert!(flags.contains(OpenFlags::EXCLUDE));
            if state.files.contains_key(&filename) {
                return Err(StatusCode::Failure);
            }
            state.files.insert(
                filename.clone(),
                File {
                    bytes: Vec::new(),
                    mode: 0o100000 | attrs.permissions.unwrap(),
                },
            );
        }
        if !state.files.contains_key(&filename) {
            return Err(StatusCode::NoSuchFile);
        }
        Ok(Handle {
            id,
            handle: filename,
        })
    }

    async fn close(&mut self, id: u32, _handle: String) -> Result<Status, Self::Error> {
        Ok(ok(id))
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let state = self.state.lock().unwrap();
        let file = state.files.get(&handle).ok_or(StatusCode::NoSuchFile)?;
        let start = usize::try_from(offset).unwrap();
        if start >= file.bytes.len() {
            return Err(StatusCode::Eof);
        }
        let end = (start + len as usize).min(file.bytes.len());
        Ok(Data {
            id,
            data: file.bytes[start..end].to_vec(),
        })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let mut state = self.state.lock().unwrap();
        state.writes += 1;
        let file = state.files.get_mut(&handle).ok_or(StatusCode::NoSuchFile)?;
        let start = usize::try_from(offset).unwrap();
        let end = start + data.len();
        assert!(end <= 1024, "fixture files stay tiny");
        file.bytes.resize(file.bytes.len().max(end), 0);
        file.bytes[start..end].copy_from_slice(&data);
        Ok(ok(id))
    }

    async fn fsetstat(
        &mut self,
        id: u32,
        handle: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let mut state = self.state.lock().unwrap();
        state
            .files
            .get_mut(&handle)
            .ok_or(StatusCode::NoSuchFile)?
            .mode = attrs.permissions.unwrap();
        Ok(ok(id))
    }

    async fn rename(&mut self, id: u32, old: String, new: String) -> Result<Status, Self::Error> {
        let mut state = self.state.lock().unwrap();
        state.renames += 1;
        if state.files.contains_key(&new) {
            return Err(StatusCode::Failure);
        }
        let file = state.files.remove(&old).ok_or(StatusCode::NoSuchFile)?;
        state.files.insert(new.clone(), file);
        if new == TARGET && old.contains(".mobarust-edit-") {
            state.promoted = true;
            if state.fault == Fault::ConcurrentWrite {
                state.files.get_mut(TARGET).unwrap().bytes = EXTERNAL.to_vec();
            }
        }
        Ok(ok(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        let mut state = self.state.lock().unwrap();
        if state.fault == Fault::BackupCleanupDenied && filename.contains(".mobarust-edit-backup-")
        {
            return Err(StatusCode::PermissionDenied);
        }
        state
            .files
            .remove(&filename)
            .ok_or(StatusCode::NoSuchFile)?;
        Ok(ok(id))
    }
}

impl Drop for Sftp {
    fn drop(&mut self) {
        if let Some(ended) = self.ended.take() {
            let _ = ended.send(());
        }
    }
}

struct Ssh {
    channels: HashMap<russh::ChannelId, russh::Channel<server::Msg>>,
    state: Arc<Mutex<State>>,
    ended: Option<oneshot::Sender<()>>,
}

impl server::Handler for Ssh {
    type Error = russh::Error;
    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        assert_eq!(user, "fixture");
        assert_eq!(password, "disposable");
        Ok(Auth::Accept)
    }
    async fn channel_open_session(
        &mut self,
        channel: russh::Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
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
            self.channels.remove(&channel).unwrap().into_stream(),
            Sftp {
                state: self.state.clone(),
                ended: self.ended.take(),
            },
        )
        .await;
        Ok(())
    }
}

struct Fixture {
    connection: SshConnection,
    state: Arc<Mutex<State>>,
    worker: Option<JoinHandle<Result<(), russh::Error>>>,
    ended: Option<oneshot::Receiver<()>>,
}

impl Fixture {
    async fn connect(fault: Fault, existing: bool) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        assert!(address.ip().is_loopback());
        let mut seed = Zeroizing::new([0; 32]);
        seed[..16].copy_from_slice(Uuid::new_v4().as_bytes());
        seed[16..].copy_from_slice(Uuid::new_v4().as_bytes());
        let key =
            PrivateKey::new(KeypairData::Ed25519(Ed25519Keypair::from_seed(&seed)), "").unwrap();
        let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
        let config = Arc::new(server::Config {
            keys: vec![key],
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            nodelay: true,
            ..Default::default()
        });
        let state = Arc::new(Mutex::new(State {
            files: if existing {
                HashMap::from([(
                    TARGET.into(),
                    File {
                        bytes: ORIGINAL.to_vec(),
                        mode: 0o100640,
                    },
                )])
            } else {
                HashMap::new()
            },
            fault,
            promoted: false,
            post_promotion_reads: 0,
            writes: 0,
            renames: 0,
        }));
        let (ended, receiver) = oneshot::channel();
        let handler = Ssh {
            channels: HashMap::new(),
            state: state.clone(),
            ended: Some(ended),
        };
        let worker = tokio::spawn(async move {
            let (stream, peer) = tokio::time::timeout(DEADLINE, listener.accept())
                .await
                .unwrap()
                .unwrap();
            assert!(peer.ip().is_loopback());
            drop(listener);
            server::run_stream(config, stream, handler).await?.await
        });
        let options = SshConnectOptions {
            host: address.ip().to_string(),
            port: address.port(),
            host_key_policy: HostKeyPolicy::PinnedFingerprint(fingerprint),
            timeout: DEADLINE,
            keepalive_interval: None,
            credentials: SshCredentials::password("fixture", "disposable"),
            x11: None,
            environment: Vec::new(),
            startup_directory: None,
            startup_command: None,
        };
        let connection = match SshConnection::connect(options).await {
            Ok(connection) => connection,
            Err(error) => {
                worker.abort();
                panic!("fixture connection failed: {error}");
            }
        };
        Self {
            connection,
            state,
            worker: Some(worker),
            ended: Some(receiver),
        }
    }
    async fn finish(mut self) {
        self.connection.disconnect().await.unwrap();
        tokio::time::timeout(DEADLINE, self.worker.take().unwrap())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        tokio::time::timeout(DEADLINE, self.ended.take().unwrap())
            .await
            .unwrap()
            .unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.abort();
        }
    }
}

async fn verify_receipt(fault: Fault) {
    // Normal Save, replacing Save as, and creating Save as all share the
    // committed-receipt boundary, but have different backup preparation.
    for operation in ["save", "replace", "create"] {
        let fixture = Fixture::connect(fault, operation != "create").await;
        let sftp = fixture.connection.open_sftp().await.unwrap();
        let saved = if operation == "save" {
            let opened = sftp.read_text_document(TARGET).await.unwrap();
            sftp.save_text_document(TARGET, &opened.revision, WRITTEN)
                .await
        } else {
            sftp.save_text_document_as(
                TARGET,
                WRITTEN,
                RemoteTextEncoding::Utf8,
                operation == "replace",
            )
            .await
        }
        .expect("acknowledged promotion returns a committed save receipt");
        assert_eq!(saved.content, WRITTEN, "{fault:?}: {operation}");
        assert_eq!(saved.size, WRITTEN.len() as u64);
        assert_eq!(
            saved.modified_unix_seconds, None,
            "a receipt does not invent a fresh server timestamp"
        );
        let warning = serde_json::to_value(&saved).unwrap()["backupCleanupFailed"].as_bool();
        assert_eq!(
            warning,
            Some(fault == Fault::BackupCleanupDenied && operation != "create")
        );
        {
            let state = fixture.state.lock().unwrap();
            assert!(state.promoted);
            assert_eq!(state.post_promotion_reads, 0);
            let expected = if fault == Fault::ConcurrentWrite {
                EXTERNAL
            } else {
                WRITTEN.as_bytes()
            };
            assert_eq!(state.files[TARGET].bytes, expected);
            let backups = state
                .files
                .iter()
                .filter(|(path, _)| path.contains(".mobarust-edit-backup-"))
                .collect::<Vec<_>>();
            if fault == Fault::BackupCleanupDenied && operation != "create" {
                assert_eq!(backups.len(), 1);
                assert_eq!(backups[0].1.bytes, ORIGINAL);
            } else {
                assert!(backups.is_empty());
            }
            assert!(
                state
                    .files
                    .keys()
                    .all(|path| !path.contains(".mobarust-edit-")
                        || path.contains(".mobarust-edit-backup-"))
            );
        }
        if fault == Fault::ConcurrentWrite {
            let writes = fixture.state.lock().unwrap().writes;
            assert!(matches!(
                sftp.save_text_document(TARGET, &saved.revision, "retry")
                    .await,
                Err(SshError::RemoteConflict)
            ));
            assert_eq!(fixture.state.lock().unwrap().writes, writes);
            assert_eq!(fixture.state.lock().unwrap().files[TARGET].bytes, EXTERNAL);
        } else if fault == Fault::BackupCleanupDenied {
            let retry = sftp
                .save_text_document(TARGET, &saved.revision, "next edit")
                .await
                .unwrap();
            assert_eq!(retry.content, "next edit");
            assert_eq!(
                fixture.state.lock().unwrap().files[TARGET].bytes,
                b"next edit"
            );
        }
        sftp.close().await.unwrap();
        fixture.finish().await;
    }
}

#[tokio::test]
async fn committed_editor_saves_do_not_depend_on_a_followup_read() {
    verify_receipt(Fault::RereadDenied).await;
}

#[tokio::test]
async fn concurrent_post_save_changes_do_not_replace_the_saved_buffer_or_revision() {
    verify_receipt(Fault::ConcurrentWrite).await;
}

#[tokio::test]
async fn backup_cleanup_failures_keep_a_usable_saved_revision_and_report_a_warning() {
    verify_receipt(Fault::BackupCleanupDenied).await;
}

#[tokio::test]
async fn upload_promotion_refuses_unknown_special_and_directory_types_before_rename() {
    for (mode, cancellable) in [
        None,
        Some(0),
        Some(0o010600),
        Some(0o020600),
        Some(0o040755),
        Some(0o060600),
        Some(0o140600),
        Some(0o170600),
    ]
    .into_iter()
    .flat_map(|mode| [false, true].map(|cancellable| (mode, cancellable)))
    {
        let fixture = Fixture::connect(
            if mode.is_none() {
                Fault::MissingTypeMetadata
            } else {
                Fault::None
            },
            true,
        )
        .await;
        const PART: &str = "/complete.part";
        {
            let mut state = fixture.state.lock().unwrap();
            if let Some(mode) = mode {
                state.files.get_mut(TARGET).unwrap().mode = mode;
            }
            state.files.insert(
                PART.into(),
                File {
                    bytes: WRITTEN.as_bytes().to_vec(),
                    mode: 0o100600,
                },
            );
        }
        let sftp = fixture.connection.open_sftp().await.unwrap();
        let preflight = sftp.check_upload_destination(TARGET).await.unwrap_err();
        let result = if cancellable {
            let (_sender, mut cancel) = oneshot::channel();
            sftp.promote_uploaded_file_with_cancel(PART, TARGET, true, &mut cancel)
                .await
        } else {
            sftp.promote_uploaded_file(PART, TARGET, true).await
        };
        let error = result.expect_err("refuse unsafe upload destination");
        assert_eq!(error.to_string(), preflight.to_string());
        if mode == Some(0o040755) {
            assert!(matches!(error, SshError::RemoteUploadDestinationDirectory));
        } else {
            assert!(matches!(
                error,
                SshError::RemoteUploadDestinationUnsupported
            ));
        }
        {
            let state = fixture.state.lock().unwrap();
            assert_eq!(state.files[TARGET].bytes, ORIGINAL);
            assert!(!state.files.contains_key(PART));
            assert_eq!(
                state.renames, 0,
                "refuse unsafe types before any rename request"
            );
            assert_eq!(state.files.len(), 1, "no unexpected backup");
        }
        sftp.close().await.unwrap();
        fixture.finish().await;
    }
}
