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
use russh_sftp::protocol::{
    Attrs, Data, File as ProtocolFile, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode,
};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use uuid::Uuid;
use zeroize::Zeroizing;

const DEADLINE: Duration = Duration::from_secs(10);
const TARGET: &str = "/document.txt";
const DIRECT_UPLOAD: &str = "/direct-upload.txt";
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
    UploadCloseDenied,
    UploadWriteDenied,
    DownloadReadDenied,
    DownloadCloseDenied,
    DownloadHandleMetadataDenied,
    DirectoryDotFlood,
    DirectoryTextFlood,
    DirectoryDrip,
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
    setstats: usize,
    lstats: usize,
    cancel_on_close: Option<oneshot::Sender<()>>,
    opens: usize,
    reads: usize,
    closes: usize,
    handle_mode: Option<u32>,
    close_started: Option<oneshot::Sender<()>>,
    close_gate: Option<oneshot::Receiver<()>>,
    directory_reads: usize,
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

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        assert_eq!(path, "/listing");
        self.state.lock().unwrap().directory_reads = 0;
        Ok(Handle {
            id,
            handle: "directory".into(),
        })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        assert_eq!(handle, "directory");
        let (fault, reads) = {
            let mut state = self.state.lock().unwrap();
            state.directory_reads += 1;
            (state.fault, state.directory_reads)
        };
        let files = match fault {
            Fault::DirectoryDotFlood if reads <= 11 => {
                vec![ProtocolFile::dummy("."); if reads == 11 { 3 } else { 1_000 }]
            }
            Fault::DirectoryTextFlood if reads <= 129 => {
                vec![ProtocolFile::dummy("x".repeat(65_536))]
            }
            Fault::DirectoryDrip => {
                tokio::time::sleep(Duration::from_secs(2)).await;
                vec![ProtocolFile::dummy(".")]
            }
            Fault::None if reads == 1 => {
                let mut attrs = FileAttributes::empty();
                attrs.permissions = Some(0o100640);
                attrs.size = Some(7);
                vec![
                    ProtocolFile::dummy("."),
                    ProtocolFile::dummy(".."),
                    ProtocolFile::new("café.txt", attrs),
                ]
            }
            _ => return Err(StatusCode::Eof),
        };
        let reply = Name { id, files };
        assert!(
            russh_sftp::ser::to_bytes(&reply).unwrap().len()
                < russh_sftp::client::Config::default().max_packet_len as usize,
            "listing responses stay within the existing packet limit"
        );
        Ok(reply)
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let mut state = self.state.lock().unwrap();
        state.lstats += 1;
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

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        if self.state.lock().unwrap().fault == Fault::DownloadHandleMetadataDenied {
            return Err(StatusCode::PermissionDenied);
        }
        let mut attrs = self.lstat(id, handle).await?;
        if let Some(mode) = self.state.lock().unwrap().handle_mode {
            attrs.attrs.permissions = Some(mode);
        }
        Ok(attrs)
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        flags: OpenFlags,
        attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let mut state = self.state.lock().unwrap();
        state.opens += 1;
        if filename == DIRECT_UPLOAD {
            assert_eq!(
                flags.bits(),
                (OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE).bits()
            );
            assert!(attrs.permissions.is_none());
            state
                .files
                .entry(filename.clone())
                .or_insert_with(|| File {
                    bytes: Vec::new(),
                    mode: 0o100644,
                })
                .bytes
                .clear();
        } else if flags.contains(OpenFlags::CREATE) {
            assert!(flags.contains(OpenFlags::EXCLUDE));
            assert_eq!(attrs.permissions, Some(0o600));
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

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        let gate = {
            let mut state = self.state.lock().unwrap();
            state.closes += 1;
            if (state.fault == Fault::UploadCloseDenied && handle != TARGET)
                || (state.fault == Fault::DownloadCloseDenied && handle == TARGET)
            {
                return Err(StatusCode::PermissionDenied);
            }
            if let Some(sender) = state.cancel_on_close.take() {
                sender.send(()).unwrap();
            }
            if let Some(sender) = state.close_started.take() {
                sender.send(()).unwrap();
            }
            state.close_gate.take()
        };
        if let Some(gate) = gate {
            gate.await.unwrap();
        }
        Ok(ok(id))
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let mut state = self.state.lock().unwrap();
        state.reads += 1;
        if state.fault == Fault::DownloadReadDenied {
            return Err(StatusCode::PermissionDenied);
        }
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
        if state.fault == Fault::UploadWriteDenied {
            return Err(StatusCode::PermissionDenied);
        }
        let file = state.files.get_mut(&handle).ok_or(StatusCode::NoSuchFile)?;
        let start = usize::try_from(offset).unwrap();
        let end = start + data.len();
        assert!(end <= 1024, "fixture files stay tiny");
        file.bytes.resize(file.bytes.len().max(end), 0);
        file.bytes[start..end].copy_from_slice(&data);
        Ok(ok(id))
    }

    async fn setstat(
        &mut self,
        id: u32,
        path: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let mut state = self.state.lock().unwrap();
        state.setstats += 1;
        let file = state.files.get_mut(&path).ok_or(StatusCode::NoSuchFile)?;
        let permissions = attrs.permissions.unwrap();
        assert!(permissions <= 0o7777);
        file.mode = (file.mode & 0o170000) | permissions;
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
            setstats: 0,
            lstats: 0,
            cancel_on_close: None,
            opens: 0,
            reads: 0,
            closes: 0,
            handle_mode: None,
            close_started: None,
            close_gate: None,
            directory_reads: 0,
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

#[tokio::test]
async fn directory_listing_bounds_include_filtered_entries_text_and_drip_replies() {
    let control = Fixture::connect(Fault::None, false).await;
    let sftp = control.connection.open_sftp().await.unwrap();
    let entries = tokio::time::timeout(DEADLINE, sftp.read_dir("/listing"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "café.txt");
    assert_eq!(entries[0].path, "/listing/café.txt");
    assert_eq!(entries[0].size, Some(7));
    assert_eq!(entries[0].permissions, Some(0o100640));
    assert!(entries[0].is_regular && !entries[0].is_directory && !entries[0].is_symlink);
    assert_eq!(control.state.lock().unwrap().closes, 1);
    sftp.close().await.unwrap();
    control.finish().await;
    let mut failures = Vec::new();
    for fault in [
        Fault::DirectoryDotFlood,
        Fault::DirectoryTextFlood,
        Fault::DirectoryDrip,
    ] {
        let fixture = Fixture::connect(fault, false).await;
        let sftp = fixture.connection.open_sftp().await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(14), sftp.read_dir("/listing")).await;
        let refused = if fault == Fault::DirectoryDrip {
            matches!(result, Ok(Err(SshError::Timeout)))
        } else {
            matches!(result, Ok(Err(SshError::SftpDirectoryTooLarge)))
        };
        let closes = fixture.state.lock().unwrap().closes;
        if !refused || closes != 1 {
            let outcome = match &result {
                Ok(Ok(entries)) => format!("accepted {} entries", entries.len()),
                Ok(Err(error)) => format!("refused: {error}"),
                Err(_) => "outer test deadline expired".into(),
            };
            failures.push(format!(
                "{fault:?}: {outcome}, acknowledged closes={closes}"
            ));
        }
        if refused && closes == 1 {
            fixture.state.lock().unwrap().fault = Fault::None;
            let entries = tokio::time::timeout(DEADLINE, sftp.read_dir("/listing"))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].name, "café.txt");
            assert_eq!(entries[0].path, "/listing/café.txt");
            assert_eq!(entries[0].size, Some(7));
            assert_eq!(entries[0].permissions, Some(0o100640));
            assert_eq!(fixture.state.lock().unwrap().closes, 2);
        }
        sftp.close().await.unwrap();
        fixture.finish().await;
    }
    assert!(failures.is_empty(), "directory bounds failed: {failures:?}");
}

#[tokio::test]
async fn remote_permission_guards_refuse_unknown_types_and_preserve_special_modes() {
    let mut failures = Vec::new();
    for mode in [None, Some(0), Some(0o120777), Some(0o170600)] {
        for suffix in ["", "///"] {
            let fixture = Fixture::connect(
                if mode.is_none() {
                    Fault::MissingTypeMetadata
                } else {
                    Fault::None
                },
                true,
            )
            .await;
            if let Some(mode) = mode {
                fixture
                    .state
                    .lock()
                    .unwrap()
                    .files
                    .get_mut(TARGET)
                    .unwrap()
                    .mode = mode;
            }
            let sftp = fixture.connection.open_sftp().await.unwrap();
            let result = sftp
                .set_permissions(format!("{TARGET}{suffix}"), 0o600)
                .await;
            let refused = if mode == Some(0o120777) {
                matches!(result, Err(SshError::RemotePermissionsTargetSymlink))
            } else {
                matches!(result, Err(SshError::RemotePermissionsTargetUnsupported))
            };
            {
                let state = fixture.state.lock().unwrap();
                if !refused
                    || state.setstats != 0
                    || state.files[TARGET].mode != mode.unwrap_or(0o100640)
                {
                    failures.push(format!(
                        "mode={mode:?}, suffix={suffix:?}, result={result:?}, SETSTAT={}",
                        state.setstats
                    ));
                }
                assert_eq!(state.files[TARGET].bytes, ORIGINAL);
                assert_eq!((state.opens, state.reads), (0, 0));
            }
            sftp.close().await.unwrap();
            fixture.finish().await;
        }
    }
    assert!(
        failures.is_empty(),
        "unsafe permission changes: {failures:?}"
    );

    for kind in [0o100000, 0o040000, 0o010000, 0o020000, 0o060000, 0o140000] {
        let fixture = Fixture::connect(Fault::None, true).await;
        fixture
            .state
            .lock()
            .unwrap()
            .files
            .get_mut(TARGET)
            .unwrap()
            .mode = kind;
        let sftp = fixture.connection.open_sftp().await.unwrap();
        // A mode-000 entry remains repairable without opening its contents.
        sftp.set_permissions(format!("{TARGET}///"), 0o640)
            .await
            .unwrap();
        for alias in ["/", "//", ".", "./", "..", "/srv/..///"] {
            assert!(matches!(
                sftp.set_permissions(alias, 0o600).await,
                Err(SshError::InvalidRemoteMutationPath)
            ));
            assert!(matches!(
                sftp.remove_path(alias).await,
                Err(SshError::InvalidRemoteMutationPath)
            ));
        }
        assert!(sftp.set_permissions(TARGET, 0o10000).await.is_err());
        {
            let state = fixture.state.lock().unwrap();
            assert_eq!(state.files[TARGET].mode, kind | 0o640);
            assert_eq!(state.files[TARGET].bytes, ORIGINAL);
            assert_eq!(
                (state.lstats, state.setstats, state.opens, state.reads),
                (1, 1, 0, 0)
            );
        }
        sftp.close().await.unwrap();
        fixture.finish().await;
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

#[tokio::test]
async fn temporary_upload_close_failures_clean_owned_parts_before_promotion() {
    for operation in ["copy", "reserve", "editor"] {
        let fixture = Fixture::connect(Fault::UploadCloseDenied, true).await;
        let sftp = fixture.connection.open_sftp().await.unwrap();
        const PART: &str = "/private-upload.part";
        let (_sender, mut cancel) = oneshot::channel();
        let result = match operation {
            "copy" => {
                let mut source = &b"private bytes"[..];
                sftp.upload_temporary_from_with_cancel(&mut source, PART, &mut cancel, |_| {})
                    .await
                    .map(|_| ())
            }
            "reserve" => sftp.prepare_upload_temporary(PART, &mut cancel).await,
            _ => {
                let opened = sftp.read_text_document(TARGET).await.unwrap();
                sftp.save_text_document(TARGET, &opened.revision, WRITTEN)
                    .await
                    .map(|_| ())
            }
        };
        let error = result.expect_err("failed close must refuse promotion");
        assert!(
            matches!(error, SshError::SftpPermissionDenied),
            "{operation}: {error}"
        );
        {
            let state = fixture.state.lock().unwrap();
            assert_eq!(state.files[TARGET].bytes, ORIGINAL);
            assert_eq!(state.files.len(), 1, "owned part was removed");
            assert_eq!(state.renames, 0, "failed close must precede promotion");
        }
        sftp.close().await.unwrap();
        fixture.finish().await;
    }
}

#[tokio::test]
async fn cancellation_during_temporary_close_cleans_the_owned_part() {
    for reserve_only in [false, true] {
        let fixture = Fixture::connect(Fault::None, true).await;
        let sftp = fixture.connection.open_sftp().await.unwrap();
        let (sender, mut cancel) = oneshot::channel();
        fixture.state.lock().unwrap().cancel_on_close = Some(sender);
        const PART: &str = "/private-upload.part";
        let result = if reserve_only {
            sftp.prepare_upload_temporary(PART, &mut cancel).await
        } else {
            let mut source = &b"private bytes"[..];
            sftp.upload_temporary_from_with_cancel(&mut source, PART, &mut cancel, |_| {})
                .await
                .map(|_| ())
        };
        assert!(matches!(result, Err(SshError::Cancelled)));
        {
            let state = fixture.state.lock().unwrap();
            assert_eq!(state.files[TARGET].bytes, ORIGINAL);
            assert_eq!(state.files.len(), 1, "owned part was removed");
            assert_eq!(state.renames, 0);
            assert!(state.cancel_on_close.is_none());
        }
        sftp.close().await.unwrap();
        fixture.finish().await;
    }
}

#[tokio::test]
async fn file_read_and_write_denials_keep_their_status_and_preserve_originals() {
    for fault in [Fault::DownloadReadDenied, Fault::UploadWriteDenied] {
        let fixture = Fixture::connect(fault, true).await;
        let sftp = fixture.connection.open_sftp().await.unwrap();
        let mut downloaded = Vec::new();
        let result = if fault == Fault::DownloadReadDenied {
            sftp.download_to(TARGET, &mut downloaded).await
        } else {
            let (_sender, mut cancel) = oneshot::channel();
            let mut source = &b"private bytes"[..];
            sftp.upload_temporary_from_with_cancel(
                &mut source,
                "/private-upload.part",
                &mut cancel,
                |_| {},
            )
            .await
        };
        let error = result.expect_err("server denied file I/O");
        assert!(
            matches!(error, SshError::SftpPermissionDenied),
            "{fault:?}: {error}"
        );
        assert_eq!(error.to_string(), "SFTP permission denied");
        assert!(downloaded.is_empty());
        {
            let state = fixture.state.lock().unwrap();
            assert_eq!(state.files[TARGET].bytes, ORIGINAL);
            assert_eq!(state.files.len(), 1, "no owned upload part remains");
            assert_eq!(state.renames, 0);
        }
        sftp.close().await.unwrap();
        fixture.finish().await;
    }
}

#[tokio::test]
async fn download_sources_refuse_unsafe_types_before_open_or_destination_writes() {
    let mut failures = Vec::new();
    for (mode, cancellable) in [
        None,
        Some(0),
        Some(0o010600),
        Some(0o020600),
        Some(0o040755),
        Some(0o060600),
        Some(0o120777),
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
        if let Some(mode) = mode {
            fixture
                .state
                .lock()
                .unwrap()
                .files
                .get_mut(TARGET)
                .unwrap()
                .mode = mode;
        }
        let sftp = fixture.connection.open_sftp().await.unwrap();
        let info = sftp.file_info(TARGET).await;
        let is_directory = mode == Some(0o040755);
        let info_valid = if is_directory {
            matches!(info, Ok((Some(8), true)))
        } else {
            matches!(info, Err(SshError::RemoteDownloadSourceUnsupported))
        };
        let mut downloaded = b"untouched destination".to_vec();
        let mut progress = 0;
        let result = if cancellable {
            let (_sender, mut cancel) = oneshot::channel();
            sftp.download_to_with_cancel(TARGET, &mut downloaded, &mut cancel, |bytes| {
                progress = bytes
            })
            .await
        } else {
            sftp.download_to(TARGET, &mut downloaded).await
        };
        let result_valid = if is_directory {
            matches!(result, Err(SshError::RemoteDownloadSourceDirectory))
        } else {
            matches!(result, Err(SshError::RemoteDownloadSourceUnsupported))
        };
        {
            let state = fixture.state.lock().unwrap();
            if !info_valid
                || !result_valid
                || state.opens != 0
                || state.reads != 0
                || downloaded != b"untouched destination"
                || progress != 0
            {
                failures.push(format!(
                    "mode={mode:?}, cancellable={cancellable}, opens={}, reads={}",
                    state.opens, state.reads
                ));
            }
            assert_eq!(state.files[TARGET].bytes, ORIGINAL);
            assert_eq!(state.renames, 0);
        }
        sftp.close().await.unwrap();
        fixture.finish().await;
    }
    assert!(
        failures.is_empty(),
        "unsafe download sources accepted: {failures:?}"
    );
}

#[tokio::test]
async fn download_rechecks_the_open_handle_type_and_closes_without_reading() {
    let mut failures = Vec::new();
    for mode in [
        0, 0o010600, 0o020600, 0o040755, 0o060600, 0o120777, 0o140600, 0o170600,
    ] {
        let fixture = Fixture::connect(Fault::None, true).await;
        fixture.state.lock().unwrap().handle_mode = Some(mode);
        let sftp = fixture.connection.open_sftp().await.unwrap();
        let mut downloaded = Vec::new();
        let result = sftp.download_to(TARGET, &mut downloaded).await;
        {
            let state = fixture.state.lock().unwrap();
            if !matches!(result, Err(SshError::RemoteDownloadSourceUnsupported))
                || state.opens != 1
                || state.reads != 0
                || state.closes != 1
                || !downloaded.is_empty()
            {
                failures.push(format!(
                    "mode={mode:o}, opens={}, reads={}, closes={}",
                    state.opens, state.reads, state.closes
                ));
            }
            assert_eq!(state.files[TARGET].bytes, ORIGINAL);
        }
        sftp.close().await.unwrap();
        fixture.finish().await;
    }
    assert!(failures.is_empty(), "unsafe handles read: {failures:?}");
}

#[tokio::test]
async fn download_close_denial_and_cancellation_do_not_return_success() {
    for cancelled in [false, true] {
        let fixture = Fixture::connect(
            if cancelled {
                Fault::None
            } else {
                Fault::DownloadCloseDenied
            },
            true,
        )
        .await;
        let sftp = fixture.connection.open_sftp().await.unwrap();
        let (sender, mut cancel) = oneshot::channel();
        let mut sender = Some(sender);
        if cancelled {
            fixture.state.lock().unwrap().cancel_on_close = sender.take();
        }
        let mut downloaded = Vec::new();
        let result = sftp
            .download_to_with_cancel(TARGET, &mut downloaded, &mut cancel, |_| {})
            .await;
        assert_eq!(downloaded, ORIGINAL);
        assert_eq!(fixture.state.lock().unwrap().closes, 1);
        if cancelled {
            assert!(
                matches!(result, Err(SshError::Cancelled)),
                "cancellation during close must refuse completion"
            );
        } else {
            assert!(matches!(result, Err(SshError::SftpPermissionDenied)));
        }
        sftp.close().await.unwrap();
        fixture.finish().await;
    }
}

#[tokio::test]
async fn file_copy_failures_wait_for_close_acknowledgement() {
    use std::future::{Future, poll_fn};
    use std::task::Poll;
    for fault in [
        Fault::DownloadReadDenied,
        Fault::DownloadHandleMetadataDenied,
        Fault::UploadWriteDenied,
    ] {
        let fixture = Fixture::connect(fault, true).await;
        let sftp = fixture.connection.open_sftp().await.unwrap();
        let (started, received) = oneshot::channel();
        let (release, gate) = oneshot::channel();
        {
            let mut state = fixture.state.lock().unwrap();
            state.close_started = Some(started);
            state.close_gate = Some(gate);
        }
        let mut downloaded = Vec::new();
        let upload_bytes = vec![b'U'; 128 * 1024];
        let mut copied = Box::pin(async {
            if fault == Fault::UploadWriteDenied {
                sftp.upload_from(&upload_bytes[..], DIRECT_UPLOAD).await
            } else {
                sftp.download_to(TARGET, &mut downloaded).await
            }
        });
        tokio::time::timeout(DEADLINE, async {
            tokio::select! {
                result = &mut copied => panic!("{fault:?} returned before CLOSE acknowledgement: {result:?}; writes={}", fixture.state.lock().unwrap().writes),
                result = received => result.unwrap(),
            }
        }).await.unwrap();
        assert!(poll_fn(|cx| Poll::Ready(copied.as_mut().poll(cx).is_pending())).await);
        release.send(()).unwrap();
        assert!(matches!(copied.await, Err(SshError::SftpPermissionDenied)));
        assert!(downloaded.is_empty());
        {
            let state = fixture.state.lock().unwrap();
            assert_eq!(state.closes, 1);
            assert_eq!(state.files[TARGET].bytes, ORIGINAL);
            if fault == Fault::DownloadHandleMetadataDenied {
                assert_eq!(state.reads, 0);
            }
            if fault == Fault::UploadWriteDenied {
                assert!(
                    state.writes >= 2,
                    "exercise multiple failed WRITE acknowledgements"
                );
                assert!(state.files[DIRECT_UPLOAD].bytes.is_empty());
            }
        }
        sftp.close().await.unwrap();
        fixture.finish().await;
    }
}

#[tokio::test]
async fn direct_upload_queued_cancellation_does_not_create_or_truncate() {
    for existing in [false, true] {
        let fixture = Fixture::connect(Fault::None, true).await;
        if existing {
            fixture.state.lock().unwrap().files.insert(
                DIRECT_UPLOAD.into(),
                File {
                    bytes: ORIGINAL.to_vec(),
                    mode: 0o100640,
                },
            );
        }
        let sftp = fixture.connection.open_sftp().await.unwrap();
        let (sender, mut cancel) = oneshot::channel();
        sender.send(()).unwrap();
        let mut source = &b"new bytes"[..];
        assert!(matches!(
            sftp.upload_from_with_cancel(&mut source, DIRECT_UPLOAD, &mut cancel, |_| {})
                .await,
            Err(SshError::Cancelled)
        ));
        {
            let state = fixture.state.lock().unwrap();
            assert_eq!(
                state.opens, 0,
                "queued cancellation must precede CREATE/TRUNCATE"
            );
            if existing {
                assert_eq!(state.files[DIRECT_UPLOAD].bytes, ORIGINAL);
                assert_eq!(state.files[DIRECT_UPLOAD].mode, 0o100640);
            } else {
                assert!(!state.files.contains_key(DIRECT_UPLOAD));
            }
        }
        sftp.close().await.unwrap();
        fixture.finish().await;
    }
}

#[tokio::test]
async fn direct_upload_close_denial_and_cancellation_do_not_return_success() {
    for cancelled in [false, true] {
        let fixture = Fixture::connect(
            if cancelled {
                Fault::None
            } else {
                Fault::UploadCloseDenied
            },
            true,
        )
        .await;
        let sftp = fixture.connection.open_sftp().await.unwrap();
        let (sender, mut cancel) = oneshot::channel();
        let mut sender = Some(sender);
        if cancelled {
            fixture.state.lock().unwrap().cancel_on_close = sender.take();
        }
        let mut source = &b"written bytes"[..];
        let result = sftp
            .upload_from_with_cancel(&mut source, DIRECT_UPLOAD, &mut cancel, |_| {})
            .await;
        assert_eq!(fixture.state.lock().unwrap().closes, 1);
        if cancelled {
            assert!(
                matches!(result, Err(SshError::Cancelled)),
                "direct upload must observe cancellation during close"
            );
        } else {
            assert!(matches!(result, Err(SshError::SftpPermissionDenied)));
        }
        {
            let state = fixture.state.lock().unwrap();
            assert_eq!(state.files[DIRECT_UPLOAD].bytes, b"written bytes");
            assert_eq!(state.files[TARGET].bytes, ORIGINAL);
        }
        sftp.close().await.unwrap();
        fixture.finish().await;
    }
}
