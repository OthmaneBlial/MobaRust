use std::{
    collections::VecDeque,
    future::{self, Future},
    io::{self, SeekFrom},
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncSeek, AsyncWrite, AsyncWriteExt, ReadBuf},
    sync::oneshot,
};

use super::Metadata;
use crate::{
    client::{
        error::Error,
        rawsession::{PendingRequest, SftpResult},
        session::Features,
        RawSftpSession,
    },
    protocol::{Packet, StatusCode},
};

type StateFn<T> = Option<Pin<Box<dyn Future<Output = io::Result<T>> + Send + Sync + 'static>>>;

// read packet overhead: type(1) + id(4) + data_len(4)
const READ_OVERHEAD_LENGTH: u32 = 9;
// write packet overhead excluding handle: type(1) + id(4) + handle_len(4) + offset(8) + data_len(4)
const WRITE_OVERHEAD_LENGTH: u32 = 21;

struct FileState {
    f_read: StateFn<Option<Vec<u8>>>,
    read_buffer: io::Cursor<Vec<u8>>,
    f_seek: StateFn<u64>,
    f_flush: StateFn<()>,
    f_shutdown: StateFn<()>,
    shutdown_error: Option<io::Error>,
    write_acks: VecDeque<PendingRequest>,
}

/// Provides high-level methods for interaction with a remote file.
///
/// In order to properly close the handle, [`File::close`] or
/// [`shutdown`](tokio::io::AsyncWriteExt::shutdown) on a file should be called.
/// Also implement [`AsyncSeek`] and other async i/o implementations.
///
/// Drop queues one close unless closure completed or CLOSE is already pending.
/// It does not await pending write errors or the close status. Once shutdown is
/// polled, new handle operations refuse; a cancelled shutdown can still resume.
///
/// # Weakness
/// Using [`SeekFrom::End`] is costly and time-consuming because we need to
/// request the actual file size from the remote server.
pub struct File {
    session: Arc<RawSftpSession>,
    handle: String,
    state: FileState,
    pos: u64,
    closed: bool,
    closing: bool,
    shutdown_failed: bool,
    features: Features,
}

impl File {
    pub(crate) fn new(session: Arc<RawSftpSession>, handle: String, features: Features) -> Self {
        Self {
            session,
            handle,
            state: FileState {
                f_read: None,
                read_buffer: io::Cursor::default(),
                f_seek: None,
                f_flush: None,
                f_shutdown: None,
                shutdown_error: None,
                write_acks: VecDeque::with_capacity(features.max_concurrent_writes),
            },
            pos: 0,
            closed: false,
            closing: false,
            shutdown_failed: false,
            features,
        }
    }

    fn ensure_open(&self) -> io::Result<()> {
        if self.closing || self.closed {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "SFTP file is closing or closed",
            ));
        }
        Ok(())
    }

    /// Queries metadata about the remote file.
    pub async fn metadata(&self) -> SftpResult<Metadata> {
        self.ensure_open().map_err(Error::from)?;
        Ok(self.session.fstat(self.handle.as_str()).await?.attrs)
    }

    /// Sets metadata for a remote file.
    pub async fn set_metadata(&self, metadata: Metadata) -> SftpResult<()> {
        self.ensure_open().map_err(Error::from)?;
        self.session
            .fsetstat(self.handle.as_str(), metadata)
            .await
            .map(|_| ())
    }

    /// Attempts to sync all data.
    ///
    /// If the server does not support `fsync@openssh.com` sending the request will
    /// be omitted, but will still pseudo-successfully
    pub async fn sync_all(&self) -> SftpResult<()> {
        self.ensure_open().map_err(Error::from)?;
        if !self.features.fsync {
            return Ok(());
        }

        self.session.fsync(self.handle.as_str()).await.map(|_| ())
    }

    /// Closes the file waiting for all pending writes and the close itself
    /// to be confirmed by the remote party.
    /// Equivalent to [`shutdown`](tokio::io::AsyncWriteExt::shutdown)
    pub async fn close(mut self) -> io::Result<()> {
        self.shutdown().await
    }
}

fn check_write_result(
    result: Result<SftpResult<Packet>, oneshot::error::RecvError>,
) -> io::Result<()> {
    match result {
        Err(_) => Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "write channel closed",
        )),
        Ok(Ok(Packet::Status(s))) if s.status_code == StatusCode::Ok => Ok(()),
        Ok(Ok(Packet::Status(s))) => Err(io::Error::other(Error::Status(s))),
        Ok(Ok(_)) => Err(io::Error::other(Error::UnexpectedPacket)),
        Ok(Err(e)) => Err(io::Error::other(e)),
    }
}

fn poll_oldest_write(
    pending: &mut VecDeque<PendingRequest>,
    cx: &mut Context<'_>,
) -> Option<Poll<io::Result<()>>> {
    let rx = pending.front_mut()?;
    Some(match Pin::new(rx).poll(cx) {
        Poll::Pending => Poll::Pending,
        Poll::Ready(r) => {
            pending.pop_front();
            Poll::Ready(check_write_result(r))
        }
    })
}

fn poll_drain_writes(
    pending: &mut VecDeque<PendingRequest>,
    cx: &mut Context<'_>,
) -> Poll<io::Result<()>> {
    while let Some(poll) = poll_oldest_write(pending, cx) {
        ready!(poll)?;
    }
    Poll::Ready(Ok(()))
}

impl Drop for File {
    fn drop(&mut self) {
        // While draining writes, no CLOSE exists yet: Drop must still send it.
        // Once the close future exists, it already owns the one queued request.
        if self.closed || self.state.f_shutdown.is_some() {
            return;
        }

        let _ = self.session.close_nowait(std::mem::take(&mut self.handle));
    }
}

fn seek_position(base: u64, delta: i64) -> io::Result<u64> {
    base.checked_add_signed(delta).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "SFTP seek position out of range",
        )
    })
}

impl AsyncRead for File {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let file = self.get_mut();
        file.ensure_open()?;
        if file.pos == u64::MAX {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SFTP file position leaves no room for data",
            )));
        }
        if file.state.read_buffer.position() == file.state.read_buffer.get_ref().len() as u64 {
            let poll = Pin::new(match file.state.f_read.as_mut() {
                Some(f) => f,
                None => {
                    let session = file.session.clone();
                    let packet_read_len =
                        file.features
                            .max_packet_len
                            .saturating_sub(READ_OVERHEAD_LENGTH) as u64;
                    let max_read_len = file
                        .features
                        .limits
                        .and_then(|l| l.read_len)
                        .unwrap_or(packet_read_len)
                        .min(packet_read_len)
                        .min(u64::MAX - file.pos) as usize;
                    if max_read_len == 0 {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "SFTP packet limit leaves no room for file data",
                        )));
                    }

                    let file_handle = file.handle.clone();

                    let offset = file.pos;
                    let len = usize::min(buf.remaining(), max_read_len);

                    file.state.f_read.get_or_insert(Box::pin(async move {
                        let result = session.read(file_handle, offset, len as u32).await;
                        match result {
                            Ok(data) => Ok(Some(data.data)),
                            Err(Error::Status(status)) if status.status_code == StatusCode::Eof => {
                                Ok(None)
                            }
                            Err(e) => Err(io::Error::other(e)),
                        }
                    }))
                }
            })
            .poll(cx);

            if poll.is_ready() {
                file.state.f_read = None;
            }

            match ready!(poll)? {
                None => return Poll::Ready(Ok(())),
                Some(data) => {
                    // A cancelled caller can resume with less space than the
                    // original request. Retain its unconsumed reply, not its buffer.
                    file.state.read_buffer = io::Cursor::new(data);
                }
            }
        }
        let filled = buf.filled().len();
        ready!(Pin::new(&mut file.state.read_buffer).poll_read(cx, buf))?;
        file.pos += (buf.filled().len() - filled) as u64;
        if file.state.read_buffer.position() == file.state.read_buffer.get_ref().len() as u64 {
            file.state.read_buffer = io::Cursor::default();
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncSeek for File {
    fn start_seek(mut self: Pin<&mut Self>, position: io::SeekFrom) -> io::Result<()> {
        if matches!(position, SeekFrom::End(_)) {
            self.ensure_open()?;
        }
        if self.state.f_seek.is_some() {
            return Err(io::Error::other(
                "other file operation is pending, call poll_complete before start_seek",
            ));
        }

        self.state.f_seek = Some(match position {
            SeekFrom::Start(pos) => Box::pin(future::ready(Ok(pos))),
            SeekFrom::Current(pos) => Box::pin(future::ready(Ok(seek_position(self.pos, pos)?))),
            SeekFrom::End(pos) => {
                let session = self.session.clone();
                let file_handle = self.handle.clone();

                Box::pin(async move {
                    let result = session.fstat(file_handle).await.map_err(io::Error::other)?;
                    match result.attrs.size {
                        Some(size) => seek_position(size, pos),
                        None => Err(io::Error::other("file size unknown")),
                    }
                })
            }
        });

        Ok(())
    }

    fn poll_complete(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        match self.state.f_seek.as_mut() {
            None => Poll::Ready(Ok(self.pos)),
            Some(f) => {
                let position = ready!(Pin::new(f).poll(cx));
                self.state.f_seek = None;
                self.pos = position?;
                self.state.f_read = None;
                self.state.read_buffer = io::Cursor::default();
                Poll::Ready(Ok(self.pos))
            }
        }
    }
}

impl AsyncWrite for File {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        self.ensure_open()?;
        if self.pos == u64::MAX {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SFTP file position leaves no room for data",
            )));
        }
        if self.state.write_acks.len() >= self.features.max_concurrent_writes {
            if let Some(poll) = poll_oldest_write(&mut self.state.write_acks, cx) {
                ready!(poll)?;
            }
        }

        let overhead = u64::from(WRITE_OVERHEAD_LENGTH) + self.handle.len() as u64;
        let packet_write_len = u64::from(self.features.max_packet_len).saturating_sub(overhead);
        let limits = self.features.limits.unwrap_or_default();
        // Raw requests enforce the server limit on the complete encoded packet,
        // including its four-byte length prefix; the client budget is payload-only.
        let server_write_len = limits
            .packet_len
            .map(|limit| limit.saturating_sub(overhead + 4))
            .unwrap_or(packet_write_len);
        let max_write_len = limits
            .write_len
            .unwrap_or(packet_write_len)
            .min(packet_write_len)
            .min(server_write_len)
            .min(u64::MAX - self.pos) as usize;
        if max_write_len == 0 {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SFTP packet limit leaves no room for file data",
            )));
        }

        let len = usize::min(buf.len(), max_write_len);
        let data = buf[..len].to_vec();
        let handle = self.handle.clone();
        let offset = self.pos;

        match self.session.write_nowait(handle, offset, data) {
            Ok(rx) => {
                self.state.f_read = None;
                self.state.read_buffer = io::Cursor::default();
                self.pos += len as u64;
                self.state.write_acks.push_back(rx);
                Poll::Ready(Ok(len))
            }
            Err(e) => Poll::Ready(Err(io::Error::other(e))),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        self.ensure_open()?;
        ready!(poll_drain_writes(&mut self.state.write_acks, cx))?;

        if !self.features.fsync {
            return Poll::Ready(Ok(()));
        }

        let poll = Pin::new(match self.state.f_flush.as_mut() {
            Some(f) => f,
            None => {
                let session = self.session.clone();
                let file_handle = self.handle.clone();

                self.state.f_flush.get_or_insert(Box::pin(async move {
                    session
                        .fsync(file_handle)
                        .await
                        .map(|_| ())
                        .map_err(io::Error::other)
                }))
            }
        })
        .poll(cx);

        if poll.is_ready() {
            self.state.f_flush = None;
        }

        poll
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        if self.closed {
            return Poll::Ready(if self.shutdown_failed {
                Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "SFTP file shutdown already failed",
                ))
            } else {
                Ok(())
            });
        }
        if !self.closing {
            self.closing = true;
            // Retire borrowed operations before draining writes. A cancelled
            // caller must not resume another handle request after CLOSE starts.
            self.state.f_read = None;
            self.state.read_buffer = io::Cursor::default();
            self.state.f_seek = None;
            self.state.f_flush = None;
        }
        // A failed WRITE must not bypass the CLOSE acknowledgement. Retain the
        // first failure across polls while retiring all outstanding writes.
        while let Some(poll) = poll_oldest_write(&mut self.state.write_acks, cx) {
            if let Err(error) = ready!(poll) {
                if self.state.shutdown_error.is_none() {
                    self.state.shutdown_error = Some(error);
                }
            }
        }

        let result = ready!(Pin::new(match self.state.f_shutdown.as_mut() {
            Some(f) => f,
            None => {
                let session = self.session.clone();
                let file_handle = self.handle.clone();

                self.state.f_shutdown.get_or_insert(Box::pin(async move {
                    session.close(file_handle).await.map_err(io::Error::other)?;
                    Ok(())
                }))
            }
        })
        .poll(cx));

        self.state.f_shutdown = None;
        self.closed = true;
        self.state.f_read = None;
        self.state.read_buffer = io::Cursor::default();
        let result = self.state.shutdown_error.take().map_or(result, Err);
        self.shutdown_failed = result.is_err();
        Poll::Ready(result)
    }
}
