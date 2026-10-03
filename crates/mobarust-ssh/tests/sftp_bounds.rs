//! Bounded in-memory peers; no sockets, credentials or filesystem operations.

use std::time::Duration;

use russh_sftp::client::{Config, RawSftpSession};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

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
