//! Bounded in-memory peers; no sockets, credentials or filesystem operations.

use std::{
    future::{Future, poll_fn},
    pin::Pin,
    task::Poll,
    time::Duration,
};

use russh_sftp::client::{Config, RawSftpSession, SftpSession, fs::File};
use tokio::io::{
    AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt, AsyncWriteExt, DuplexStream, ReadBuf,
};

const DEADLINE: Duration = Duration::from_secs(2);

async fn read_init(peer: &mut DuplexStream) {
    let mut init = [0; 9];
    peer.read_exact(&mut init).await.unwrap();
    assert_eq!(init, [0, 0, 0, 5, 1, 0, 0, 0, 3]);
}

fn version_packet(length: u32) -> Vec<u8> {
    assert!(length >= 14);
    let mut packet = length.to_be_bytes().to_vec();
    packet.extend_from_slice(&[2, 0, 0, 0, 3]);
    // One extension, with a one-byte name and padding in its value.
    packet.extend_from_slice(&1_u32.to_be_bytes());
    packet.push(b'x');
    packet.extend_from_slice(&(length - 14).to_be_bytes());
    packet.resize(length as usize + 4, b'p');
    packet
}

#[tokio::test]
async fn configured_packet_limit_rejects_a_valid_oversized_version() {
    for length in [32, 33] {
        let (stream, mut peer) = tokio::io::duplex(128);
        let session = RawSftpSession::new_with_config(
            stream,
            Config {
                max_packet_len: 32,
                request_timeout_secs: 30,
                ..Config::default()
            },
        );
        let response = async {
            read_init(&mut peer).await;
            peer.write_all(&version_packet(length)).await.unwrap();
        };
        let (result, ()) =
            tokio::time::timeout(DEADLINE, async { tokio::join!(session.init(), response) })
                .await
                .expect("bounded VERSION is accepted or refused without a request timeout");
        assert_eq!(result.is_ok(), length == 32, "VERSION length {length}");
        session.close_session().unwrap();
        let mut tail = Vec::new();
        tokio::time::timeout(DEADLINE, peer.read_to_end(&mut tail))
            .await
            .expect("SFTP stream closes while the session value is still alive")
            .unwrap();
    }
}

#[tokio::test]
async fn oversized_header_and_malformed_frame_settle_pending_requests() {
    let oversized = (Config::default().max_packet_len + 1).to_be_bytes();
    // The peer sends no oversized payload. A malformed zero-length frame is
    // followed by a valid VERSION, which must never revive the failed session.
    let mut malformed = 0_u32.to_be_bytes().to_vec();
    malformed.extend_from_slice(&version_packet(32));
    for response in [oversized.to_vec(), malformed, Vec::new()] {
        let (stream, mut peer) = tokio::io::duplex(128);
        let session = RawSftpSession::new_with_config(
            stream,
            Config {
                request_timeout_secs: 30,
                ..Config::default()
            },
        );
        let server = async {
            read_init(&mut peer).await;
            peer.write_all(&response).await.unwrap();
            if response.is_empty() {
                peer.shutdown().await.unwrap();
            }
        };
        let (result, ()) =
            tokio::time::timeout(DEADLINE, async { tokio::join!(session.init(), server) })
                .await
                .expect("invalid frames must settle the waiting request immediately");
        assert!(result.is_err());
        let mut tail = Vec::new();
        tokio::time::timeout(DEADLINE, peer.read_to_end(&mut tail))
            .await
            .expect("both stream workers stop without dropping the session")
            .unwrap();
        assert!(session.init().await.is_err(), "failed session stays closed");
    }
}

#[test]
fn impossible_sequence_count_is_rejected_before_the_visitor() {
    struct Count;
    impl<'de> serde::Deserialize<'de> for Count {
        fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = Count;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("a sequence")
                }
                fn visit_seq<A: serde::de::SeqAccess<'de>>(
                    self,
                    _: A,
                ) -> Result<Self::Value, A::Error> {
                    // A real Vec visitor can reserve from the advertised count.
                    Ok(Count)
                }
            }
            de.deserialize_seq(Visitor)
        }
    }
    let mut bytes = u32::MAX.to_be_bytes().to_vec().into();
    assert!(russh_sftp::de::from_bytes::<Count>(&mut bytes).is_err());
}

#[tokio::test]
async fn data_cannot_exceed_the_requested_read_length() {
    for length in [4_u32, 5] {
        let (stream, mut peer) = tokio::io::duplex(128);
        let session = RawSftpSession::new(stream);
        let server = async {
            read_init(&mut peer).await;
            peer.write_all(&version_packet(32)).await.unwrap();
            let request_len = peer.read_u32().await.unwrap();
            let mut request = vec![0; request_len as usize];
            peer.read_exact(&mut request).await.unwrap();
            assert_eq!(&request[..5], &[5, 0, 0, 0, 1]);
            assert_eq!(&request[request.len() - 4..], &4_u32.to_be_bytes());
            let mut response = (9 + length).to_be_bytes().to_vec();
            response.extend_from_slice(&[103, 0, 0, 0, 1]);
            response.extend_from_slice(&length.to_be_bytes());
            response.resize(13 + length as usize, b'x');
            peer.write_all(&response).await.unwrap();
        };
        let client = async {
            session.init().await.unwrap();
            session.read("fixture", 0, 4).await
        };
        let (result, ()) = tokio::time::timeout(DEADLINE, async { tokio::join!(client, server) })
            .await
            .unwrap();
        assert_eq!(result.is_ok(), length == 4, "DATA length {length}");
        session.close_session().unwrap();
    }
}

#[tokio::test]
async fn failed_reader_retires_a_backpressured_writer() {
    let (stream, mut peer) = tokio::io::duplex(64);
    let session = RawSftpSession::new(stream);
    let server = async {
        read_init(&mut peer).await;
        peer.write_all(&version_packet(32)).await.unwrap();
        // Consume just one header byte; the write fills the remaining bounded
        // pipe, so its worker cannot finish without cancellation.
        let _ = peer.read_u8().await.unwrap();
        let oversized = Config::default().max_packet_len + 1;
        peer.write_all(&oversized.to_be_bytes()).await.unwrap();
    };
    let client = async {
        session.init().await.unwrap();
        session.write("fixture", 0, vec![b'x'; 4096]).await
    };
    let (result, ()) = tokio::time::timeout(DEADLINE, async { tokio::join!(client, server) })
        .await
        .expect("reader failure cancels the blocked writer and its pending request");
    assert!(result.is_err());
    let mut tail = Vec::new();
    tokio::time::timeout(DEADLINE, peer.read_to_end(&mut tail))
        .await
        .expect("blocked write does not retain the stream")
        .unwrap();
    assert!(tail.len() <= 64);
}

async fn request(peer: &mut DuplexStream) -> Vec<u8> {
    let length = peer.read_u32().await.unwrap();
    assert!(length <= 256, "fixture request stays small");
    let mut packet = vec![0; length as usize];
    peer.read_exact(&mut packet).await.unwrap();
    packet
}

async fn reply(peer: &mut DuplexStream, kind: u8, id: &[u8], data: &[u8]) {
    let mut payload = vec![kind];
    payload.extend_from_slice(id);
    payload.extend_from_slice(data);
    peer.write_u32(payload.len() as u32).await.unwrap();
    peer.write_all(&payload).await.unwrap();
}

async fn data_reply(peer: &mut DuplexStream, req: &[u8], data: &[u8]) {
    let mut payload = (data.len() as u32).to_be_bytes().to_vec();
    payload.extend_from_slice(data);
    reply(peer, 103, &req[1..5], &payload).await;
}

async fn status_reply(peer: &mut DuplexStream, req: &[u8], code: u32) {
    let mut payload = code.to_be_bytes().to_vec();
    payload.extend_from_slice(&[0; 8]); // Empty message and language tag.
    reply(peer, 101, &req[1..5], &payload).await;
}

fn read_request(req: &[u8], offset: u64, length: u32) {
    assert_eq!(req[0], 5);
    assert_eq!(&req[req.len() - 12..req.len() - 4], &offset.to_be_bytes());
    assert_eq!(&req[req.len() - 4..], &length.to_be_bytes());
}

async fn memory_file(limits: Option<(u64, u64)>) -> (SftpSession, File, DuplexStream) {
    memory_file_with_config(limits, 128, "fixture").await
}

async fn memory_file_with_config(
    limits: Option<(u64, u64)>,
    max_packet_len: u32,
    file_handle: &str,
) -> (SftpSession, File, DuplexStream) {
    memory_file_with_write_limit(
        limits.map(|(packet, read)| (packet, read, 0)),
        max_packet_len,
        file_handle,
    )
    .await
}

async fn memory_file_with_write_limit(
    limits: Option<(u64, u64, u64)>,
    max_packet_len: u32,
    file_handle: &str,
) -> (SftpSession, File, DuplexStream) {
    memory_file_with_options(limits, max_packet_len, file_handle, false).await
}

async fn memory_file_with_options(
    limits: Option<(u64, u64, u64)>,
    max_packet_len: u32,
    file_handle: &str,
    fsync: bool,
) -> (SftpSession, File, DuplexStream) {
    let (stream, mut peer) = tokio::io::duplex(1024);
    let server = async {
        read_init(&mut peer).await;
        let mut extensions = Vec::new();
        if fsync {
            let name = b"fsync@openssh.com";
            extensions.extend_from_slice(&(name.len() as u32).to_be_bytes());
            extensions.extend_from_slice(name);
            extensions.extend_from_slice(&1_u32.to_be_bytes());
            extensions.push(b'1');
        }
        if limits.is_some() {
            let name = b"limits@openssh.com";
            extensions.extend_from_slice(&(name.len() as u32).to_be_bytes());
            extensions.extend_from_slice(name);
            extensions.extend_from_slice(&1_u32.to_be_bytes());
            extensions.push(b'1');
        }
        reply(&mut peer, 2, &3_u32.to_be_bytes(), &extensions).await;
        if let Some((packet, read, write)) = limits {
            let req = request(&mut peer).await;
            assert_eq!(req[0], 200);
            let mut payload = Vec::new();
            for value in [packet, read, write, 0] {
                payload.extend_from_slice(&value.to_be_bytes());
            }
            reply(&mut peer, 201, &req[1..5], &payload).await;
        }
        let open = request(&mut peer).await;
        assert_eq!(open[0], 3);
        let mut handle = (file_handle.len() as u32).to_be_bytes().to_vec();
        handle.extend_from_slice(file_handle.as_bytes());
        reply(&mut peer, 102, &open[1..5], &handle).await;
        peer
    };
    let client = async {
        let session = SftpSession::new_with_config(
            stream,
            Config {
                max_packet_len,
                request_timeout_secs: 30,
                ..Config::default()
            },
        )
        .await
        .unwrap();
        let flags = russh_sftp::protocol::OpenFlags::READ | russh_sftp::protocol::OpenFlags::WRITE;
        let file = session.open_with_flags("fixture", flags).await.unwrap();
        (session, file)
    };
    let ((session, file), peer) =
        tokio::time::timeout(DEADLINE, async { tokio::join!(client, server) })
            .await
            .unwrap();
    (session, file, peer)
}

async fn close_peer(peer: &mut DuplexStream) {
    let close = request(peer).await;
    assert_eq!(close[0], 4, "no unexpected extra read or write");
    status_reply(peer, &close, 0).await;
    let mut tail = Vec::new();
    peer.read_to_end(&mut tail).await.unwrap();
    assert!(tail.is_empty());
}

async fn file_position(file: &mut File) -> u64 {
    // poll_complete without start_seek queries the current position without
    // introducing a seek that could discard the read buffer under test.
    poll_fn(|cx| Pin::new(&mut *file).poll_complete(cx))
        .await
        .unwrap()
}

#[tokio::test]
async fn file_relative_seeks_preserve_the_full_unsigned_offset_range() {
    use std::io::{ErrorKind, SeekFrom};
    let (session, mut file, mut peer) = memory_file(None).await;
    let client = async {
        for (start, delta, expected) in [
            (u64::MAX, 0, Some(u64::MAX)),
            (u64::MAX, 1, None),
            (u64::MAX, i64::MIN, Some(i64::MAX as u64)),
            (1_u64 << 63, i64::MAX, Some(u64::MAX)),
            (0, -1, None),
            (0, i64::MIN, None),
            (1_u64 << 63, i64::MIN, Some(0)),
        ] {
            file.seek(SeekFrom::Start(start)).await.unwrap();
            let result = file.seek(SeekFrom::Current(delta)).await;
            match expected {
                Some(position) => assert_eq!(result.unwrap(), position),
                None => assert_eq!(result.unwrap_err().kind(), ErrorKind::InvalidInput),
            }
            assert_eq!(file_position(&mut file).await, expected.unwrap_or(start));
        }
        file.close().await.unwrap();
        session.close().await.unwrap();
    };
    tokio::time::timeout(DEADLINE, async {
        tokio::join!(client, close_peer(&mut peer))
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn failed_end_seeks_retire_the_request_and_allow_a_fresh_seek() {
    use std::io::{ErrorKind, SeekFrom};
    let (session, mut file, mut peer) = memory_file(None).await;
    let server = async {
        for case in 0..5 {
            let stat = request(&mut peer).await;
            assert_eq!(stat[0], 8, "only end-relative seeks request FSTAT");
            if case == 0 {
                status_reply(&mut peer, &stat, 4).await;
            } else {
                let mut attrs = if case == 1 {
                    0_u32.to_be_bytes().to_vec()
                } else {
                    1_u32.to_be_bytes().to_vec()
                };
                if case >= 2 {
                    attrs.extend_from_slice(&u64::MAX.to_be_bytes());
                }
                reply(&mut peer, 105, &stat[1..5], &attrs).await;
            }
        }
        close_peer(&mut peer).await;
    };
    let client = async {
        for (delta, expected) in [
            (0, Err(ErrorKind::Other)),
            (0, Err(ErrorKind::Other)),
            (1, Err(ErrorKind::InvalidInput)),
            (0, Ok(u64::MAX)),
            (i64::MIN, Ok(i64::MAX as u64)),
        ] {
            file.seek(SeekFrom::Start(7)).await.unwrap();
            let result = file.seek(SeekFrom::End(delta)).await;
            match expected {
                Ok(position) => assert_eq!(result.unwrap(), position),
                Err(kind) => assert_eq!(result.unwrap_err().kind(), kind),
            }
            assert_eq!(file_position(&mut file).await, expected.unwrap_or(7));
            assert_eq!(file.seek(SeekFrom::Start(9)).await.unwrap(), 9);
        }
        file.close().await.unwrap();
        session.close().await.unwrap();
    };
    tokio::time::timeout(DEADLINE, async { tokio::join!(client, server) })
        .await
        .expect("failed metadata and overflowing seeks must permit recovery");
}

#[tokio::test]
async fn file_io_never_wraps_a_seeked_offset_or_sends_unrepresentable_data() {
    use std::io::{ErrorKind, SeekFrom};
    for writing in [false, true] {
        let (session, mut file, mut peer) = memory_file(None).await;
        let server = async {
            let req = request(&mut peer).await;
            if writing {
                assert_eq!(req[0], 6);
                assert_eq!(&req[16..24], &(u64::MAX - 1).to_be_bytes());
                assert_eq!(&req[24..28], &1_u32.to_be_bytes());
                assert_eq!(&req[28..], b"x");
                status_reply(&mut peer, &req, 0).await;
            } else {
                read_request(&req, u64::MAX - 1, 1);
                data_reply(&mut peer, &req, b"x").await;
            }
            close_peer(&mut peer).await;
        };
        let client = async {
            file.seek(SeekFrom::Start(u64::MAX - 1)).await.unwrap();
            if writing {
                assert_eq!(file.write(b"xy").await.unwrap(), 1);
                file.flush().await.unwrap();
                assert_eq!(
                    file.write(b"y").await.unwrap_err().kind(),
                    ErrorKind::InvalidInput
                );
            } else {
                let mut bytes = [0; 2];
                assert_eq!(file.read(&mut bytes).await.unwrap(), 1);
                assert_eq!(bytes[0], b'x');
                assert_eq!(
                    file.read(&mut bytes).await.unwrap_err().kind(),
                    ErrorKind::InvalidInput
                );
            }
            assert_eq!(file_position(&mut file).await, u64::MAX);
            empty_file_io(&mut file).await;
            assert_eq!(
                file.seek(SeekFrom::Current(-1)).await.unwrap(),
                u64::MAX - 1
            );
            file.close().await.unwrap();
            session.close().await.unwrap();
        };
        tokio::time::timeout(DEADLINE, async { tokio::join!(client, server) })
            .await
            .expect("offset exhaustion must fail before any additional wire request");
    }
}

async fn cancel_pending_read(file: &mut File, observed: tokio::sync::oneshot::Receiver<()>) {
    let mut original = [0; 4];
    let read = file.read(&mut original);
    tokio::pin!(read);
    tokio::select! {
        _ = observed => {}
        result = &mut read => panic!("peer has not replied: {result:?}"),
    }
}

async fn empty_file_io(file: &mut File) {
    let mut empty = [];
    let mut buf = ReadBuf::new(&mut empty);
    poll_fn(|cx| {
        assert!(matches!(
            Pin::new(&mut *file).poll_read(cx, &mut buf),
            Poll::Ready(Ok(()))
        ));
        Poll::Ready(())
    })
    .await;
    assert_eq!(file.write(&[]).await.unwrap(), 0);
}

#[tokio::test]
async fn cancelled_file_read_resumes_in_smaller_buffers_without_losing_bytes() {
    let (session, mut file, mut peer) = memory_file(None).await;
    let (observed, observation) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let server = async {
        let req = request(&mut peer).await;
        read_request(&req, 0, 4);
        observed.send(()).unwrap();
        released.await.unwrap();
        data_reply(&mut peer, &req, b"abcd").await;
        let req = request(&mut peer).await;
        read_request(&req, 4, 4);
        status_reply(&mut peer, &req, 1).await;
        close_peer(&mut peer).await;
    };
    let client = async {
        cancel_pending_read(&mut file, observation).await;
        release.send(()).unwrap();
        let mut one = [0];
        assert_eq!(file.read(&mut one).await.unwrap(), 1);
        assert_eq!(&one, b"a");
        assert_eq!(file_position(&mut file).await, 1);
        empty_file_io(&mut file).await;
        assert_eq!(file_position(&mut file).await, 1);
        let mut two = [0; 2];
        file.read_exact(&mut two).await.unwrap();
        assert_eq!(&two, b"bc");
        assert_eq!(file_position(&mut file).await, 3);
        let mut four = [0; 4];
        assert_eq!(file.read(&mut four).await.unwrap(), 1);
        assert_eq!(four[0], b'd');
        assert_eq!(file_position(&mut file).await, 4);
        assert_eq!(file.read(&mut four).await.unwrap(), 0);
        file.close().await.unwrap();
        session.close().await.unwrap();
    };
    tokio::time::timeout(DEADLINE, async { tokio::join!(client, server) })
        .await
        .expect("smaller reads consume the original reply exactly once");
}

#[tokio::test]
async fn empty_file_read_does_not_start_or_consume_a_request() {
    let (session, mut file, mut peer) = memory_file(None).await;
    let client = async {
        empty_file_io(&mut file).await;
        assert_eq!(file_position(&mut file).await, 0);
        file.close().await.unwrap();
        session.close().await.unwrap();
    };
    tokio::time::timeout(DEADLINE, async {
        tokio::join!(client, close_peer(&mut peer))
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn negotiated_file_reads_fit_both_client_and_server_packet_limits() {
    for (packet, read, expected) in [
        (256, 512, 119),
        (64, 512, 55),
        (256, 8, 8),
        (1_u64 << 32, 512, 119),
    ] {
        let (session, mut file, mut peer) = memory_file(Some((packet, read))).await;
        let server = async {
            let req = request(&mut peer).await;
            read_request(&req, 0, expected);
            data_reply(&mut peer, &req, &vec![b'x'; expected as usize]).await;
            close_peer(&mut peer).await;
        };
        let client = async {
            let mut buffer = [0; 256];
            assert_eq!(file.read(&mut buffer).await.unwrap(), expected as usize);
            assert!(buffer[..expected as usize].iter().all(|byte| *byte == b'x'));
            file.close().await.unwrap();
            session.close().await.unwrap();
        };
        tokio::time::timeout(DEADLINE, async { tokio::join!(client, server) })
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn file_read_refuses_a_packet_budget_without_room_for_data() {
    // A nine-byte HANDLE with an empty opaque handle fits this deliberately
    // tiny configuration; DATA has the same overhead and no payload budget.
    let (session, mut file, mut peer) = memory_file_with_config(None, 9, "").await;
    let server = async {
        let close = request(&mut peer).await;
        assert_eq!(close[0], 4, "zero-length READ must not look like file EOF");
        let mut tail = Vec::new();
        peer.read_to_end(&mut tail).await.unwrap();
        assert!(tail.is_empty());
    };
    let client = async {
        let mut byte = [0];
        assert_eq!(
            file.read(&mut byte).await.unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        // STATUS cannot fit either. Retire the fixture stream rather than
        // inventing a successful handle-close reply above the configured cap.
        drop(file);
        session.close().await.unwrap();
    };
    tokio::time::timeout(DEADLINE, async { tokio::join!(client, server) })
        .await
        .unwrap();
}

#[tokio::test]
async fn seek_and_write_retire_pending_and_buffered_read_data() {
    for (seek, buffered) in [(true, false), (true, true), (false, false), (false, true)] {
        let (session, mut file, mut peer) = memory_file(None).await;
        let (observed, observation) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let position = if seek {
            8
        } else if buffered {
            3
        } else {
            2
        };
        let server = async {
            let req = request(&mut peer).await;
            read_request(&req, 0, 4);
            observed.send(()).unwrap();
            released.await.unwrap();
            data_reply(&mut peer, &req, b"abcd").await;
            if !seek {
                let write = request(&mut peer).await;
                assert_eq!(write[0], 6);
                let offset = 9 + 7; // type/id, handle length, handle.
                assert_eq!(
                    &write[offset..offset + 8],
                    &(if buffered { 1_u64 } else { 0 }).to_be_bytes()
                );
                assert_eq!(&write[offset + 12..], b"XY");
                status_reply(&mut peer, &write, 0).await;
            }
            let req = request(&mut peer).await;
            read_request(&req, position, 2);
            data_reply(&mut peer, &req, b"ne").await;
            close_peer(&mut peer).await;
        };
        let client = async {
            cancel_pending_read(&mut file, observation).await;
            empty_file_io(&mut file).await;
            release.send(()).unwrap();
            if buffered {
                let mut one = [0];
                file.read_exact(&mut one).await.unwrap();
                assert_eq!(&one, b"a");
            }
            if seek {
                assert_eq!(
                    file.seek(std::io::SeekFrom::Start(position)).await.unwrap(),
                    position
                );
            } else {
                file.write_all(b"XY").await.unwrap();
                file.flush().await.unwrap();
            }
            let mut two = [0; 2];
            file.read_exact(&mut two).await.unwrap();
            assert_eq!(&two, b"ne");
            assert_eq!(file_position(&mut file).await, position + 2);
            file.close().await.unwrap();
            session.close().await.unwrap();
        };
        tokio::time::timeout(DEADLINE, async { tokio::join!(client, server) })
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn shutdown_retires_pending_and_buffered_file_reads() {
    for buffered in [false, true] {
        let (session, mut file, mut peer) = memory_file(None).await;
        let (observed, observation) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let server = async {
            let req = request(&mut peer).await;
            read_request(&req, 0, 4);
            observed.send(()).unwrap();
            released.await.unwrap();
            data_reply(&mut peer, &req, b"abcd").await;
            close_peer(&mut peer).await;
        };
        let client = async {
            cancel_pending_read(&mut file, observation).await;
            release.send(()).unwrap();
            if buffered {
                let mut one = [0];
                file.read_exact(&mut one).await.unwrap();
                assert_eq!(&one, b"a");
            }
            file.shutdown().await.unwrap();
            let mut two = [0; 2];
            assert_eq!(
                file.read(&mut two).await.unwrap_err().kind(),
                std::io::ErrorKind::BrokenPipe
            );
            drop(file);
            session.close().await.unwrap();
        };
        tokio::time::timeout(DEADLINE, async { tokio::join!(client, server) })
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn cancelled_and_timed_out_raw_reads_accept_late_replies_without_closing_the_stream() {
    use std::future::Future;
    for cancelled in [false, true] {
        let (stream, mut peer) = tokio::io::duplex(128);
        let session = RawSftpSession::new(stream);
        tokio::time::timeout(DEADLINE, async {
            let server = async {
                read_init(&mut peer).await;
                peer.write_all(&version_packet(32)).await.unwrap();
            };
            let (init, ()) = tokio::join!(session.init(), server);
            init.unwrap();
            if cancelled {
                let mut read = Box::pin(session.read("fixture", 0, 2));
                poll_fn(|cx| {
                    assert!(read.as_mut().poll(cx).is_pending());
                    Poll::Ready(())
                })
                .await;
                drop(read);
            } else {
                session.set_timeout(0);
                assert!(matches!(
                    session.read("fixture", 0, 2).await,
                    Err(russh_sftp::client::error::Error::Timeout)
                ));
                session.set_timeout(30);
            }
            let retired = request(&mut peer).await;
            read_request(&retired, 0, 2);
            let server = async {
                let live = request(&mut peer).await;
                read_request(&live, 2, 2);
                assert_ne!(&live[1..5], &retired[1..5]);
                data_reply(&mut peer, &retired, b"xx").await;
                data_reply(&mut peer, &live, b"ok").await;
            };
            let (result, ()) = tokio::join!(session.read("fixture", 2, 2), server);
            assert_eq!(
                result
                    .expect("late response must not close a healthy stream")
                    .data,
                b"ok"
            );
            session.close_session().unwrap();
            let mut tail = Vec::new();
            peer.read_to_end(&mut tail).await.unwrap();
            assert!(tail.is_empty());
        })
        .await
        .expect("late-response recovery remains bounded");
    }
}

#[tokio::test]
async fn pre_version_and_duplicate_version_replies_still_close_the_stream() {
    for duplicate in [false, true] {
        let (stream, mut peer) = tokio::io::duplex(128);
        let session = RawSftpSession::new(stream);
        tokio::time::timeout(DEADLINE, async {
            let server = async {
                read_init(&mut peer).await;
                if duplicate {
                    peer.write_all(&version_packet(32)).await.unwrap();
                    let pending = request(&mut peer).await;
                    read_request(&pending, 0, 2);
                    peer.write_all(&version_packet(32)).await.unwrap();
                } else {
                    // A valid ordinary response cannot precede VERSION.
                    reply(&mut peer, 103, &1_u32.to_be_bytes(), &[0; 4]).await;
                }
            };
            let client = async {
                let result = if duplicate {
                    session.init().await.unwrap();
                    session.read("fixture", 0, 2).await.map(|_| ())
                } else {
                    session.init().await.map(|_| ())
                };
                assert!(result.is_err());
                assert!(session.read("fixture", 0, 2).await.is_err());
            };
            tokio::join!(client, server);
            let mut tail = Vec::new();
            peer.read_to_end(&mut tail).await.unwrap();
            assert!(
                tail.is_empty(),
                "both workers close while session remains alive"
            );
        })
        .await
        .expect("initialization protocol errors fail closed promptly");
    }
}

#[tokio::test]
async fn negotiated_file_writes_fit_packet_and_data_limits_without_losing_bytes() {
    for (packet_limit, write_limit, chunk_limit) in [
        (512_u64, 512_u64, 100_usize),
        (64, 512, 32),
        (512, 8, 8),
        (1_u64 << 32, 512, 100),
        (64, 0, 32),
    ] {
        let (session, mut file, mut peer) =
            memory_file_with_write_limit(Some((packet_limit, 0, write_limit)), 128, "fixture")
                .await;
        let source: Vec<u8> = (0..256).map(|i| i as u8).collect();
        let server = async {
            let mut received = Vec::new();
            while received.len() < source.len() {
                let req = request(&mut peer).await;
                assert_eq!(req[0], 6);
                assert!(req.len() <= 128, "WRITE body fits the client packet budget");
                assert!(
                    req.len() as u64 + 4 <= packet_limit,
                    "encoded WRITE fits the raw server limit"
                );
                assert_eq!(&req[5..9], &7_u32.to_be_bytes());
                assert_eq!(&req[9..16], b"fixture");
                assert_eq!(&req[16..24], &(received.len() as u64).to_be_bytes());
                let count = u32::from_be_bytes(req[24..28].try_into().unwrap()) as usize;
                assert_eq!(count, chunk_limit.min(source.len() - received.len()));
                assert_eq!(req.len(), 28 + count);
                received.extend_from_slice(&req[28..]);
                status_reply(&mut peer, &req, 0).await;
            }
            assert_eq!(received, source);
            close_peer(&mut peer).await;
        };
        let client = async {
            let first = file.write(&source).await.unwrap();
            assert_eq!(
                first, chunk_limit,
                "oversized advertised data limit must not replace the packet budget"
            );
            file.write_all(&source[first..]).await.unwrap();
            assert_eq!(file_position(&mut file).await, source.len() as u64);
            file.close().await.unwrap();
            session.close().await.unwrap();
        };
        tokio::time::timeout(DEADLINE, async { tokio::join!(client, server) })
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn file_write_refuses_a_budget_without_room_for_data() {
    let (session, mut file, mut peer) = memory_file_with_config(None, 9, "").await;
    tokio::time::timeout(DEADLINE, async {
        let client = async {
            assert_eq!(
                file.write(b"x").await.unwrap_err().kind(),
                std::io::ErrorKind::InvalidInput
            );
            assert_eq!(file_position(&mut file).await, 0);
            assert_eq!(file.write(&[]).await.unwrap(), 0);
            drop(file);
            session.close().await.unwrap();
        };
        let server = async {
            let close = request(&mut peer).await;
            assert_eq!(close[0], 4, "no zero-length WRITE is sent");
            let mut tail = Vec::new();
            peer.read_to_end(&mut tail).await.unwrap();
            assert!(tail.is_empty());
        };
        tokio::join!(client, server);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn shutdown_drains_failed_writes_and_keeps_the_first_status_after_close() {
    for close_status in [0, 4] {
        let (session, mut file, mut peer) = memory_file(None).await;
        let server = async {
            let first = request(&mut peer).await;
            let second = request(&mut peer).await;
            assert_eq!(first[0], 6);
            assert_eq!(second[0], 6);
            assert_eq!(&first[28..], b"one");
            assert_eq!(&second[28..], b"two");
            status_reply(&mut peer, &first, 3).await;
            status_reply(&mut peer, &second, 4).await;
            let close = request(&mut peer).await;
            assert_eq!(close[0], 4, "failed writes still require CLOSE");
            status_reply(&mut peer, &close, close_status).await;
            let mut tail = Vec::new();
            peer.read_to_end(&mut tail).await.unwrap();
            assert!(
                tail.is_empty(),
                "no duplicated close or data after shutdown"
            );
        };
        let client = async {
            file.write_all(b"one").await.unwrap();
            file.write_all(b"two").await.unwrap();
            let error = file.shutdown().await.unwrap_err();
            let source = error
                .get_ref()
                .unwrap()
                .downcast_ref::<russh_sftp::client::error::Error>()
                .unwrap();
            assert!(
                matches!(source, russh_sftp::client::error::Error::Status(status)
                if status.status_code == russh_sftp::protocol::StatusCode::PermissionDenied)
            );
            assert_eq!(
                file.write(b"x").await.unwrap_err().kind(),
                std::io::ErrorKind::BrokenPipe
            );
            drop(file);
            session.close().await.unwrap();
        };
        tokio::time::timeout(DEADLINE, async { tokio::join!(client, server) })
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn shutdown_handle_retirement_blocks_wire_operations_and_duplicate_close() {
    let mut failures = Vec::new();
    for (pending, close_status) in [(false, 0), (false, 3), (true, 0), (true, 3)] {
        let (session, mut file, mut peer) =
            memory_file_with_options(None, 128, "fixture", true).await;
        let (close_seen, seen) = tokio::sync::oneshot::channel();
        let server = async {
            // Positive controls prove these APIs and the fsync extension work
            // before shutdown. The guard must not disable a live file.
            let metadata = request(&mut peer).await;
            assert_eq!(metadata[0], 8);
            let mut attrs = 1_u32.to_be_bytes().to_vec();
            attrs.extend_from_slice(&0_u64.to_be_bytes());
            reply(&mut peer, 105, &metadata[1..5], &attrs).await;
            for expected in [10, 200, 200] {
                let req = request(&mut peer).await;
                assert_eq!(req[0], expected);
                status_reply(&mut peer, &req, 0).await;
            }
            let close = request(&mut peer).await;
            assert_eq!(close[0], 4);
            close_seen.send(()).unwrap();
            if !pending {
                status_reply(&mut peer, &close, close_status).await;
            }
            let mut unexpected = Vec::new();
            loop {
                let req = request(&mut peer).await;
                if req[0] == 17 {
                    // The session-level marker flushes every earlier request,
                    // without relying on a timing-based quiet period.
                    if pending {
                        status_reply(&mut peer, &close, close_status).await;
                    }
                    status_reply(&mut peer, &req, 4).await;
                    break;
                }
                unexpected.push(req[0]);
                status_reply(&mut peer, &req, if req[0] == 4 { close_status } else { 4 }).await;
            }
            let mut tail = Vec::new();
            peer.read_to_end(&mut tail).await.unwrap();
            assert!(tail.is_empty());
            unexpected
        };
        let client = async {
            assert_eq!(file.metadata().await.unwrap().size, Some(0));
            file.set_metadata(russh_sftp::client::fs::Metadata::empty())
                .await
                .unwrap();
            file.sync_all().await.unwrap();
            file.flush().await.unwrap();
            if pending {
                let mut shutdown = Box::pin(file.shutdown());
                poll_fn(|cx| {
                    assert!(shutdown.as_mut().poll(cx).is_pending());
                    Poll::Ready(())
                })
                .await;
                drop(shutdown);
            } else {
                assert_eq!(file.shutdown().await.is_ok(), close_status == 0);
            }
            seen.await.unwrap();
            let mut refused = vec![
                matches!(
                    file.metadata().await,
                    Err(russh_sftp::client::error::Error::IO(_))
                ),
                matches!(
                    file.set_metadata(russh_sftp::client::fs::Metadata::empty())
                        .await,
                    Err(russh_sftp::client::error::Error::IO(_))
                ),
                matches!(
                    file.sync_all().await,
                    Err(russh_sftp::client::error::Error::IO(_))
                ),
            ];
            let mut byte = [0];
            refused.push(matches!(file.read(&mut byte).await, Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe));
            refused.push(matches!(file.write(b"x").await, Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe));
            refused.push(matches!(file.flush().await, Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe));
            refused.push(matches!(file.seek(std::io::SeekFrom::End(0)).await, Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe));
            assert_eq!(file.read(&mut []).await.unwrap(), 0);
            assert_eq!(file.write(&[]).await.unwrap(), 0);
            let position = file_position(&mut file).await;
            if !pending {
                let retry = file.shutdown().await;
                refused.push(if close_status == 0 {
                    retry.is_ok()
                } else {
                    matches!(retry, Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe)
                });
            }
            assert!(session.metadata("marker").await.is_err());
            if pending {
                // Cancelling the borrowed future preserves its one in-flight
                // close; resuming it still observes the original status.
                assert_eq!(file.shutdown().await.is_ok(), close_status == 0);
            }
            drop(file);
            session.close().await.unwrap();
            (refused, position)
        };
        let ((refused, position), unexpected) =
            tokio::time::timeout(DEADLINE, async { tokio::join!(client, server) })
                .await
                .unwrap();
        if refused.iter().any(|refused| !refused) || position != 0 || !unexpected.is_empty() {
            failures.push(format!("pending={pending}, close_status={close_status}, refused={refused:?}, position={position}, extra packets={unexpected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "retired handle operations: {failures:?}"
    );
}

#[tokio::test]
async fn cancelled_shutdown_drop_sends_exactly_one_close_in_each_phase() {
    let mut failures = Vec::new();
    for draining_write in [false, true] {
        let (session, mut file, mut peer) = memory_file(None).await;
        let server = async {
            let write = if draining_write {
                let req = request(&mut peer).await;
                assert_eq!(req[0], 6);
                Some(req)
            } else {
                None
            };
            let mut closes = 0;
            loop {
                let req = request(&mut peer).await;
                if req[0] == 17 {
                    status_reply(&mut peer, &req, 4).await;
                    break;
                }
                assert_eq!(req[0], 4);
                closes += 1;
                if let Some(write) = &write {
                    status_reply(&mut peer, write, 0).await;
                }
                status_reply(&mut peer, &req, 0).await;
            }
            let mut tail = Vec::new();
            peer.read_to_end(&mut tail).await.unwrap();
            assert!(tail.is_empty());
            closes
        };
        let client = async {
            if draining_write {
                file.write_all(b"x").await.unwrap();
            }
            let mut shutdown = Box::pin(file.shutdown());
            poll_fn(|cx| {
                assert!(shutdown.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            drop(shutdown);
            if draining_write {
                assert!(
                    matches!(file.write(b"y").await, Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe)
                );
                assert_eq!(file_position(&mut file).await, 1);
            }
            drop(file);
            assert!(session.metadata("marker").await.is_err());
            session.close().await.unwrap();
        };
        let ((), closes) = tokio::time::timeout(DEADLINE, async { tokio::join!(client, server) })
            .await
            .unwrap();
        if closes != 1 {
            failures.push(format!(
                "draining_write={draining_write}: CLOSE count={closes}"
            ));
        }
    }
    assert!(failures.is_empty(), "cancelled drop closes: {failures:?}");
}

#[tokio::test]
async fn closed_file_refuses_nonempty_writes_without_changing_position() {
    let (session, mut file, mut peer) = memory_file(None).await;
    tokio::time::timeout(DEADLINE, async {
        let server = close_peer(&mut peer);
        let client = async {
            file.shutdown().await.unwrap();
            assert_eq!(
                file.write(b"x").await.unwrap_err().kind(),
                std::io::ErrorKind::BrokenPipe
            );
            assert_eq!(file_position(&mut file).await, 0);
            assert_eq!(file.write(&[]).await.unwrap(), 0);
            drop(file);
            session.close().await.unwrap();
        };
        tokio::join!(client, server);
    })
    .await
    .unwrap();
}
