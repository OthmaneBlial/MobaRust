use bytes::Bytes;
use dashmap::DashMap as HashMap;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc,
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, oneshot},
};

use super::{error::Error, runtime, Handler};
use crate::{
    client::{run_with_config, Config},
    de,
    extensions::{
        self, ExpandPathExtension, FsyncExtension, HardlinkExtension, LimitsExtension, Statvfs,
        StatvfsExtension,
    },
    protocol::{
        Attrs, Close, Data, Extended, ExtendedReply, FSetStat, FileAttributes, Fstat, Handle, Init,
        Lstat, MkDir, Name, Open, OpenDir, OpenFlags, Packet, Read, ReadDir, ReadLink, RealPath,
        Remove, Rename, RmDir, SetStat, Stat, Status, StatusCode, Symlink, Version, Write,
    },
};

pub type SftpResult<T> = Result<T, Error>;
type SharedRequests = HashMap<Option<u32>, oneshot::Sender<SftpResult<Packet>>>;

/// Owns the reply slot until completion or cancellation, including nowait I/O.
pub(crate) struct PendingRequest {
    rx: oneshot::Receiver<SftpResult<Packet>>,
    id: Option<u32>,
    requests: Arc<SharedRequests>,
}

impl Future for PendingRequest {
    type Output = Result<SftpResult<Packet>, oneshot::error::RecvError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.rx).poll(cx)
    }
}

impl Drop for PendingRequest {
    fn drop(&mut self) {
        self.rx.close();
        // Do not remove a newer live receiver if a request ID was reused.
        self.requests
            .remove_if(&self.id, |_, sender| sender.is_closed());
    }
}

pub(crate) struct SessionInner {
    version: Option<u32>,
    requests: Arc<SharedRequests>,
    closed: Arc<AtomicBool>,
}

impl Drop for SessionInner {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
        // Wake requests on EOF, malformed frames and either worker's failure.
        self.requests.clear();
    }
}

impl SessionInner {
    pub fn reply(&mut self, id: Option<u32>, packet: Packet) -> SftpResult<()> {
        if let Some((_, sender)) = self.requests.remove(&id) {
            let validate = if id.is_some() && self.version.is_none() {
                Err(Error::UnexpectedPacket)
            } else if id.is_none() && self.version.is_some() {
                Err(Error::UnexpectedBehavior("Duplicate version".to_owned()))
            } else {
                Ok(())
            };

            // Ignore send error: receiver was dropped (request timed out).
            let _ = sender.send(validate.clone().map(|_| packet));

            return validate;
        }

        if id.is_some() && self.version.is_some() {
            // Cancellation and timeout cannot retract a transmitted request.
            // Discard its late reply; never deliver it to a different request.
            Ok(())
        } else {
            Err(Error::UnexpectedPacket)
        }
    }
}

impl Handler for SessionInner {
    type Error = Error;

    async fn version(&mut self, packet: Version) -> Result<(), Self::Error> {
        let version = packet.version;
        self.reply(None, packet.into())?;
        self.version = Some(version);
        Ok(())
    }

    async fn name(&mut self, name: Name) -> Result<(), Self::Error> {
        self.reply(Some(name.id), name.into())
    }

    async fn status(&mut self, status: Status) -> Result<(), Self::Error> {
        self.reply(Some(status.id), status.into())
    }

    async fn handle(&mut self, handle: Handle) -> Result<(), Self::Error> {
        self.reply(Some(handle.id), handle.into())
    }

    async fn data(&mut self, data: Data) -> Result<(), Self::Error> {
        self.reply(Some(data.id), data.into())
    }

    async fn attrs(&mut self, attrs: Attrs) -> Result<(), Self::Error> {
        self.reply(Some(attrs.id), attrs.into())
    }

    async fn extended_reply(&mut self, reply: ExtendedReply) -> Result<(), Self::Error> {
        self.reply(Some(reply.id), reply.into())
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Limits {
    pub packet_len: Option<u64>,
    pub read_len: Option<u64>,
    pub write_len: Option<u64>,
    pub open_handles: Option<u64>,
}

impl From<LimitsExtension> for Limits {
    fn from(limits: LimitsExtension) -> Self {
        Self {
            packet_len: (limits.max_packet_len > 0).then_some(limits.max_packet_len),
            read_len: (limits.max_read_len > 0).then_some(limits.max_read_len),
            write_len: (limits.max_write_len > 0).then_some(limits.max_write_len),
            open_handles: (limits.max_open_handles > 0).then_some(limits.max_open_handles),
        }
    }
}

/// Implements raw work with the protocol in request-response format.
/// If the server returns a `Status` packet and it has the code Ok
/// then the packet is returned as Ok in other error cases
/// the packet is stored as Err.
pub struct RawSftpSession {
    tx: mpsc::UnboundedSender<Bytes>,
    requests: Arc<SharedRequests>,
    closed: Arc<AtomicBool>,
    next_req_id: AtomicU32,
    handles: AtomicU64,
    timeout: AtomicU64,
    limits: Limits,
}

macro_rules! into_with_status {
    ($result:ident, $packet:ident) => {
        match $result {
            Packet::$packet(p) => Ok(p),
            Packet::Status(p) => Err(p.into()),
            _ => Err(Error::UnexpectedPacket),
        }
    };
}

macro_rules! into_status {
    ($result:ident) => {
        match $result {
            Packet::Status(status) if status.status_code == StatusCode::Ok => Ok(status),
            Packet::Status(status) => Err(status.into()),
            _ => Err(Error::UnexpectedPacket),
        }
    };
}

impl RawSftpSession {
    pub fn new<S>(stream: S) -> Self
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        Self::new_with_config(stream, Config::default())
    }

    pub fn new_with_config<S>(stream: S, cfg: Config) -> Self
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let req_map = Arc::new(HashMap::new());
        let closed = Arc::new(AtomicBool::new(false));
        let inner = SessionInner {
            version: None,
            requests: req_map.clone(),
            closed: closed.clone(),
        };

        Self {
            tx: run_with_config(stream, inner, cfg.clone()),
            requests: req_map,
            closed,
            next_req_id: AtomicU32::new(1),
            handles: AtomicU64::new(0),
            timeout: AtomicU64::new(cfg.request_timeout_secs),
            limits: Limits::default(),
        }
    }

    /// Set the maximum response time in seconds.
    /// Default: 10 seconds
    pub fn set_timeout(&self, secs: u64) {
        self.timeout.store(secs, Ordering::Relaxed);
    }

    /// Setting limits. For the `limits@openssh.com` extension
    pub fn set_limits(&mut self, limits: Limits) {
        self.limits = limits;
    }

    fn send(&self, id: Option<u32>, packet: Packet) -> SftpResult<PendingRequest> {
        if self.tx.is_closed() || self.closed.load(Ordering::SeqCst) {
            return Err(Error::UnexpectedBehavior("session closed".into()));
        }

        let bytes = Bytes::try_from(packet)?;

        if let Some(max_len) = self.limits.packet_len {
            if bytes.len() as u64 > max_len {
                return Err(Error::Limited("packet exceeds server limit".to_owned()));
            }
        }

        let (tx, rx) = oneshot::channel();
        self.requests.insert(id, tx);
        // The reader can close between the first check and insertion.
        if self.closed.load(Ordering::SeqCst) {
            self.requests.remove(&id);
            return Err(Error::UnexpectedBehavior("session closed".into()));
        }
        if let Err(err) = self.tx.send(bytes) {
            self.requests.remove(&id);
            return Err(err.into());
        }

        Ok(PendingRequest {
            rx,
            id,
            requests: self.requests.clone(),
        })
    }

    async fn request(&self, id: Option<u32>, packet: Packet) -> SftpResult<Packet> {
        let rx = self.send(id, packet)?;
        let timeout = self.timeout.load(Ordering::Relaxed);

        match runtime::timeout(Duration::from_secs(timeout), rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(Error::UnexpectedBehavior("sender dropped".into())),
            Err(error) => Err(error),
        }
    }

    fn use_next_id(&self) -> u32 {
        self.next_req_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Closes the inner channel stream. Called by [`Drop`]
    pub fn close_session(&self) -> SftpResult<()> {
        if self.tx.is_closed() {
            return Ok(());
        }

        Ok(self.tx.send(Bytes::new())?)
    }

    pub async fn init(&self) -> SftpResult<Version> {
        let result = self.request(None, Init::default().into()).await?;
        if let Packet::Version(version) = result {
            Ok(version)
        } else {
            Err(Error::UnexpectedPacket)
        }
    }

    pub async fn open<T: Into<String>>(
        &self,
        filename: T,
        flags: OpenFlags,
        attrs: FileAttributes,
    ) -> SftpResult<Handle> {
        if self
            .limits
            .open_handles
            .is_some_and(|h| self.handles.load(Ordering::SeqCst) >= h)
        {
            return Err(Error::Limited("handle limit reached".to_owned()));
        }

        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                Open {
                    id,
                    filename: filename.into(),
                    pflags: flags,
                    attrs,
                }
                .into(),
            )
            .await?;

        if let Packet::Handle(_) = result {
            self.handles.fetch_add(1, Ordering::SeqCst);
        }

        into_with_status!(result, Handle)
    }

    pub async fn close<H: Into<String>>(&self, handle: H) -> SftpResult<Status> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                Close {
                    id,
                    handle: handle.into(),
                }
                .into(),
            )
            .await?;

        if let Packet::Status(status) = &result {
            if status.status_code == StatusCode::Ok
                && self
                    .handles
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |h| {
                        if h > 0 {
                            Some(h - 1)
                        } else {
                            None
                        }
                    })
                    .is_err()
            {
                warn!("attempt to close more handles than exist");
            }
        }

        into_status!(result)
    }

    /// Sends a close packet without awaiting the server's acknowledgement.
    pub(crate) fn close_nowait(&self, handle: String) -> SftpResult<PendingRequest> {
        let id = self.use_next_id();
        self.send(Some(id), Close { id, handle }.into())
    }

    pub async fn read<H: Into<String>>(
        &self,
        handle: H,
        offset: u64,
        len: u32,
    ) -> SftpResult<Data> {
        if self.limits.read_len.is_some_and(|r| len as u64 > r) {
            return Err(Error::Limited("read limit reached".to_owned()));
        }

        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                Read {
                    id,
                    handle: handle.into(),
                    offset,
                    len,
                }
                .into(),
            )
            .await?;

        let data = into_with_status!(result, Data)?;
        if data.data.len() as u64 > u64::from(len) {
            return Err(Error::UnexpectedBehavior(
                "data exceeds requested read length".to_owned(),
            ));
        }
        Ok(data)
    }

    pub async fn write<H: Into<String>>(
        &self,
        handle: H,
        offset: u64,
        data: Vec<u8>,
    ) -> SftpResult<Status> {
        if self.limits.write_len.is_some_and(|w| data.len() as u64 > w) {
            return Err(Error::Limited("write limit reached".to_owned()));
        }

        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                Write {
                    id,
                    handle: handle.into(),
                    offset,
                    data,
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    /// Sends a write packet without awaiting the server's acknowledgement.
    pub(crate) fn write_nowait(
        &self,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> SftpResult<PendingRequest> {
        if self.limits.write_len.is_some_and(|w| data.len() as u64 > w) {
            return Err(Error::Limited("write limit reached".to_owned()));
        }

        let id = self.use_next_id();
        self.send(
            Some(id),
            Write {
                id,
                handle,
                offset,
                data,
            }
            .into(),
        )
    }

    pub async fn lstat<P: Into<String>>(&self, path: P) -> SftpResult<Attrs> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                Lstat {
                    id,
                    path: path.into(),
                }
                .into(),
            )
            .await?;

        into_with_status!(result, Attrs)
    }

    pub async fn fstat<H: Into<String>>(&self, handle: H) -> SftpResult<Attrs> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                Fstat {
                    id,
                    handle: handle.into(),
                }
                .into(),
            )
            .await?;

        into_with_status!(result, Attrs)
    }

    pub async fn setstat<P: Into<String>>(
        &self,
        path: P,
        attrs: FileAttributes,
    ) -> SftpResult<Status> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                SetStat {
                    id,
                    path: path.into(),
                    attrs,
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    pub async fn fsetstat<H: Into<String>>(
        &self,
        handle: H,
        attrs: FileAttributes,
    ) -> SftpResult<Status> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                FSetStat {
                    id,
                    handle: handle.into(),
                    attrs,
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    pub async fn opendir<P: Into<String>>(&self, path: P) -> SftpResult<Handle> {
        if self
            .limits
            .open_handles
            .is_some_and(|h| self.handles.load(Ordering::SeqCst) >= h)
        {
            return Err(Error::Limited("Handle limit reached".to_owned()));
        }

        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                OpenDir {
                    id,
                    path: path.into(),
                }
                .into(),
            )
            .await?;

        if let Packet::Handle(_) = result {
            self.handles.fetch_add(1, Ordering::SeqCst);
        }

        into_with_status!(result, Handle)
    }

    pub async fn readdir<H: Into<String>>(&self, handle: H) -> SftpResult<Name> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                ReadDir {
                    id,
                    handle: handle.into(),
                }
                .into(),
            )
            .await?;

        into_with_status!(result, Name)
    }

    pub async fn remove<T: Into<String>>(&self, filename: T) -> SftpResult<Status> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                Remove {
                    id,
                    filename: filename.into(),
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    pub async fn mkdir<P: Into<String>>(
        &self,
        path: P,
        attrs: FileAttributes,
    ) -> SftpResult<Status> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                MkDir {
                    id,
                    path: path.into(),
                    attrs,
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    pub async fn rmdir<P: Into<String>>(&self, path: P) -> SftpResult<Status> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                RmDir {
                    id,
                    path: path.into(),
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    pub async fn realpath<P: Into<String>>(&self, path: P) -> SftpResult<Name> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                RealPath {
                    id,
                    path: path.into(),
                }
                .into(),
            )
            .await?;

        into_with_status!(result, Name)
    }

    pub async fn stat<P: Into<String>>(&self, path: P) -> SftpResult<Attrs> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                Stat {
                    id,
                    path: path.into(),
                }
                .into(),
            )
            .await?;

        into_with_status!(result, Attrs)
    }

    pub async fn rename<O, N>(&self, oldpath: O, newpath: N) -> SftpResult<Status>
    where
        O: Into<String>,
        N: Into<String>,
    {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                Rename {
                    id,
                    oldpath: oldpath.into(),
                    newpath: newpath.into(),
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    pub async fn readlink<P: Into<String>>(&self, path: P) -> SftpResult<Name> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                ReadLink {
                    id,
                    path: path.into(),
                }
                .into(),
            )
            .await?;

        into_with_status!(result, Name)
    }

    pub async fn symlink<P, T>(&self, path: P, target: T) -> SftpResult<Status>
    where
        P: Into<String>,
        T: Into<String>,
    {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                Symlink {
                    id,
                    linkpath: path.into(),
                    targetpath: target.into(),
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    /// Equivalent to `SSH_FXP_EXTENDED`. Allows protocol expansion.
    /// The extension can return any packet, so it's not specific
    pub async fn extended<R: Into<String>>(&self, request: R, data: Vec<u8>) -> SftpResult<Packet> {
        let id = self.use_next_id();
        self.request(
            Some(id),
            Extended {
                id,
                request: request.into(),
                data,
            }
            .into(),
        )
        .await
    }

    pub async fn limits(&self) -> SftpResult<LimitsExtension> {
        match self.extended(extensions::LIMITS, vec![]).await? {
            Packet::ExtendedReply(reply) => {
                Ok(de::from_bytes::<LimitsExtension>(&mut reply.data.into())?)
            }
            Packet::Status(status) if status.status_code != StatusCode::Ok => {
                Err(Error::Status(status))
            }
            _ => Err(Error::UnexpectedPacket),
        }
    }

    pub async fn hardlink<O, N>(&self, oldpath: O, newpath: N) -> SftpResult<Status>
    where
        O: Into<String>,
        N: Into<String>,
    {
        let result = self
            .extended(
                extensions::HARDLINK,
                HardlinkExtension {
                    oldpath: oldpath.into(),
                    newpath: newpath.into(),
                }
                .try_into()?,
            )
            .await?;

        into_status!(result)
    }

    pub async fn fsync<H: Into<String>>(&self, handle: H) -> SftpResult<Status> {
        let result = self
            .extended(
                extensions::FSYNC,
                FsyncExtension {
                    handle: handle.into(),
                }
                .try_into()?,
            )
            .await?;

        into_status!(result)
    }

    pub async fn statvfs<P>(&self, path: P) -> SftpResult<Statvfs>
    where
        P: Into<String>,
    {
        let result = self
            .extended(
                extensions::STATVFS,
                StatvfsExtension { path: path.into() }.try_into()?,
            )
            .await?;

        match result {
            Packet::ExtendedReply(reply) => Ok(de::from_bytes::<Statvfs>(&mut reply.data.into())?),
            Packet::Status(status) if status.status_code != StatusCode::Ok => {
                Err(Error::Status(status))
            }
            _ => Err(Error::UnexpectedPacket),
        }
    }

    /// Expands `~`/`~user` and canonicalizes the path.
    /// Replies in the same format as [`RawSftpSession::realpath`]
    pub async fn expand_path<P: Into<String>>(&self, path: P) -> SftpResult<Name> {
        let result = self
            .extended(
                extensions::EXPAND_PATH,
                ExpandPathExtension { path: path.into() }.try_into()?,
            )
            .await?;

        into_with_status!(result, Name)
    }
}

impl Drop for RawSftpSession {
    fn drop(&mut self) {
        let _ = self.close_session();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::{poll_fn, Future},
        task::Poll,
    };

    // Test the shared bookkeeping without worker tasks, sockets or files.
    fn queues() -> (RawSftpSession, SessionInner, mpsc::UnboundedReceiver<Bytes>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let requests = Arc::new(HashMap::new());
        let closed = Arc::new(AtomicBool::new(false));
        let inner = SessionInner {
            version: Some(3),
            requests: requests.clone(),
            closed: closed.clone(),
        };
        let session = RawSftpSession {
            tx,
            requests,
            closed,
            next_req_id: AtomicU32::new(1),
            handles: AtomicU64::new(0),
            timeout: AtomicU64::new(30),
            limits: Limits::default(),
        };
        (session, inner, rx)
    }

    fn data(id: u32) -> Packet {
        Data {
            id,
            data: b"ok".to_vec(),
        }
        .into()
    }

    #[tokio::test]
    async fn dropping_an_async_request_removes_its_reply_slot() {
        let (session, _inner, mut wire) = queues();
        let mut read = Box::pin(session.read("fixture", 0, 2));
        poll_fn(|cx| {
            assert!(read.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert!(
            wire.try_recv().is_ok(),
            "request was accepted for transmission"
        );
        assert_eq!(session.requests.len(), 1);
        drop(read);
        assert!(
            session.requests.is_empty(),
            "cancelled future must release its slot immediately"
        );
    }

    #[test]
    fn dropping_nowait_requests_removes_their_reply_slots() {
        let (session, _inner, mut wire) = queues();
        let write = session
            .write_nowait("fixture".into(), 0, b"ok".to_vec())
            .unwrap();
        let close = session.close_nowait("fixture".into()).unwrap();
        assert_eq!(session.requests.len(), 2);
        drop(write);
        assert_eq!(
            session.requests.len(),
            1,
            "abandoned write ack must release its slot"
        );
        drop(close);
        assert!(
            session.requests.is_empty(),
            "discarded file-drop close ack must release its slot"
        );
        assert!(wire.try_recv().is_ok());
        assert!(
            wire.try_recv().is_ok(),
            "dropping a reply does not retract the queued request"
        );
    }

    #[tokio::test]
    async fn timeout_removes_the_slot_and_a_late_reply_preserves_live_requests() {
        let (session, mut inner, _wire) = queues();
        session.set_timeout(0);
        assert!(matches!(
            session.read("fixture", 0, 2).await,
            Err(Error::Timeout)
        ));
        assert!(session.requests.is_empty());
        let mut live = Box::pin(session.read("fixture", 2, 2));
        session.set_timeout(30);
        poll_fn(|cx| {
            assert!(live.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert_eq!(session.requests.len(), 1);
        inner
            .reply(Some(1), data(1))
            .expect("late reply is discarded");
        inner
            .reply(Some(999), data(999))
            .expect("unmatched reply cannot affect another request");
        assert_eq!(session.requests.len(), 1);
        inner.reply(Some(2), data(2)).unwrap();
        assert!(session.requests.is_empty());
        assert_eq!(live.await.unwrap().data, b"ok");
    }

    #[test]
    fn retiring_an_old_receiver_preserves_a_reused_live_id() {
        let (session, _inner, _wire) = queues();
        let old = session.close_nowait("fixture".into()).unwrap();
        session.next_req_id.store(1, Ordering::Relaxed);
        let new = session.close_nowait("fixture".into()).unwrap();
        drop(old);
        assert_eq!(session.requests.len(), 1);
        drop(new);
        assert!(session.requests.is_empty());
    }

    #[test]
    fn initialization_errors_still_fail_closed() {
        let (session, mut inner, _wire) = queues();
        assert!(inner
            .reply(
                None,
                Version {
                    version: 3,
                    extensions: Default::default()
                }
                .into()
            )
            .is_err());
        inner.version = None;
        assert!(
            inner.reply(Some(99), data(99)).is_err(),
            "responses before VERSION are invalid"
        );
        let read = session.write_nowait("fixture".into(), 0, vec![]).unwrap();
        assert!(inner.reply(Some(1), data(1)).is_err());
        drop(read);
        assert!(session.requests.is_empty());
    }
}
