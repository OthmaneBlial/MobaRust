//! Rust-owned Telnet transport.
//!
//! Telnet is intentionally kept separate from SSH: it is a legacy, plaintext
//! protocol. The adapter negotiates only a small, explicit set of options,
//! bounds subnegotiation frames, and exposes raw terminal bytes to the native
//! caller for rendering. It never claims SSH-level confidentiality.

use std::collections::{HashSet, VecDeque};
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use encoding_rs::WINDOWS_1252;
use mobarust_core::{ConnectionEvent, ConnectionLifecycle, ConnectionState};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::Mutex as AsyncMutex;

pub const DEFAULT_TELNET_PORT: u16 = 23;

const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;

const ECHO: u8 = 1;
const SUPPRESS_GO_AHEAD: u8 = 3;
const TERMINAL_TYPE: u8 = 24;
const NAWS: u8 = 31;
const TERMINAL_TYPE_IS: u8 = 0;

const MAX_SUBNEGOTIATION_BYTES: usize = 4096;
const MAX_READ_BYTES: usize = 16 * 1024;

#[derive(Debug, Error)]
pub enum TelnetError {
    #[error("Telnet host and port are required")]
    InvalidOptions,
    #[error("Telnet operation timed out")]
    Timeout,
    #[error("Telnet host could not be resolved")]
    DnsFailure,
    #[error("Telnet connection was refused")]
    ConnectionRefused,
    #[error("Telnet host is unreachable")]
    HostUnreachable,
    #[error("Telnet network connection failed")]
    ConnectionFailed,
    #[error("Telnet I/O failed: {0}")]
    Io(#[source] io::Error),
    #[error("Telnet protocol frame is invalid: {0}")]
    Protocol(String),
    #[error("Telnet connection is closed")]
    Closed,
    #[error("Telnet connection was cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TelnetEncoding {
    #[default]
    Utf8,
    Windows1252,
}

impl TelnetEncoding {
    pub fn decode(self, bytes: &[u8]) -> String {
        match self {
            Self::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
            Self::Windows1252 => WINDOWS_1252.decode(bytes).0.into_owned(),
        }
    }

    pub fn encode(self, text: &str) -> Vec<u8> {
        match self {
            Self::Utf8 => text.as_bytes().to_vec(),
            Self::Windows1252 => WINDOWS_1252.encode(text).0.into_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelnetOptions {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub terminal: String,
    #[serde(default)]
    pub encoding: TelnetEncoding,
    #[serde(default = "default_columns")]
    pub columns: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout: Duration,
    #[serde(default = "default_operation_timeout")]
    pub operation_timeout: Duration,
}

impl TelnetOptions {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            terminal: "xterm-256color".into(),
            encoding: TelnetEncoding::default(),
            columns: default_columns(),
            rows: default_rows(),
            connect_timeout: default_connect_timeout(),
            operation_timeout: default_operation_timeout(),
        }
    }

    pub fn validate(&self) -> Result<(), TelnetError> {
        if self.host.trim().is_empty()
            || self.host.contains('\0')
            || self.port == 0
            || self.terminal.trim().is_empty()
            || self.terminal.len() > 128
            || self
                .terminal
                .bytes()
                .any(|byte| byte.is_ascii_control() || byte == IAC)
            || self.columns == 0
            || self.rows == 0
            || self.connect_timeout.is_zero()
            || self.operation_timeout.is_zero()
        {
            return Err(TelnetError::InvalidOptions);
        }
        Ok(())
    }
}

impl Default for TelnetOptions {
    fn default() -> Self {
        Self::new("127.0.0.1", DEFAULT_TELNET_PORT)
    }
}

fn default_columns() -> u16 {
    120
}

fn default_rows() -> u16 {
    32
}

fn default_connect_timeout() -> Duration {
    Duration::from_secs(12)
}

fn default_operation_timeout() -> Duration {
    Duration::from_secs(5)
}

fn map_connect_error(error: io::Error) -> TelnetError {
    match error.kind() {
        io::ErrorKind::NotFound => TelnetError::DnsFailure,
        io::ErrorKind::ConnectionRefused => TelnetError::ConnectionRefused,
        io::ErrorKind::NetworkUnreachable | io::ErrorKind::HostUnreachable => {
            TelnetError::HostUnreachable
        }
        io::ErrorKind::TimedOut => TelnetError::Timeout,
        _ => TelnetError::ConnectionFailed,
    }
}

/// A bounded retry policy for explicitly requested reconnect attempts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TelnetReconnectPolicy {
    pub max_attempts: u8,
    pub initial_backoff: Duration,
}

impl Default for TelnetReconnectPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(250),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecodeState {
    Data,
    Iac,
    Command(u8),
    Subnegotiation,
    SubnegotiationIac,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelnetCodecOutput {
    pub data: Vec<u8>,
    pub responses: Vec<Vec<u8>>,
}

/// Incremental Telnet framing and option negotiation.
///
/// The codec is intentionally independently testable. It preserves partial
/// IAC sequences between reads and never lets a subnegotiation frame grow
/// beyond [`MAX_SUBNEGOTIATION_BYTES`].
#[derive(Debug, Clone)]
pub struct TelnetCodec {
    state: DecodeState,
    subnegotiation: Vec<u8>,
    remote_options: HashSet<u8>,
    local_options: HashSet<u8>,
}

impl Default for TelnetCodec {
    fn default() -> Self {
        Self {
            state: DecodeState::Data,
            subnegotiation: Vec::new(),
            remote_options: HashSet::new(),
            local_options: HashSet::new(),
        }
    }
}

impl TelnetCodec {
    pub fn feed(
        &mut self,
        input: &[u8],
        options: &TelnetOptions,
    ) -> Result<TelnetCodecOutput, TelnetError> {
        let mut output = TelnetCodecOutput {
            data: Vec::new(),
            responses: Vec::new(),
        };

        for &byte in input {
            match self.state {
                DecodeState::Data => {
                    if byte == IAC {
                        self.state = DecodeState::Iac;
                    } else {
                        output.data.push(byte);
                    }
                }
                DecodeState::Iac => match byte {
                    IAC => {
                        output.data.push(IAC);
                        self.state = DecodeState::Data;
                    }
                    WILL | WONT | DO | DONT => self.state = DecodeState::Command(byte),
                    SB => {
                        self.subnegotiation.clear();
                        self.state = DecodeState::Subnegotiation;
                    }
                    _ => self.state = DecodeState::Data,
                },
                DecodeState::Command(command) => {
                    self.negotiate(command, byte, options, &mut output.responses);
                    self.state = DecodeState::Data;
                }
                DecodeState::Subnegotiation => {
                    if byte == IAC {
                        self.state = DecodeState::SubnegotiationIac;
                    } else {
                        self.push_subnegotiation(byte)?;
                    }
                }
                DecodeState::SubnegotiationIac => match byte {
                    SE => {
                        self.state = DecodeState::Data;
                        self.subnegotiation.clear();
                    }
                    IAC => {
                        self.push_subnegotiation(IAC)?;
                        self.state = DecodeState::Subnegotiation;
                    }
                    _ => {
                        self.state = DecodeState::Data;
                    }
                },
            }
        }

        Ok(output)
    }

    fn push_subnegotiation(&mut self, byte: u8) -> Result<(), TelnetError> {
        if self.subnegotiation.len() >= MAX_SUBNEGOTIATION_BYTES {
            return Err(TelnetError::Protocol(
                "subnegotiation frame exceeds 4096 bytes".into(),
            ));
        }
        self.subnegotiation.push(byte);
        Ok(())
    }

    fn negotiate(
        &mut self,
        command: u8,
        option: u8,
        settings: &TelnetOptions,
        responses: &mut Vec<Vec<u8>>,
    ) {
        match command {
            WILL => {
                if accepts_remote_option(option) {
                    if self.remote_options.insert(option) {
                        responses.push(vec![IAC, DO, option]);
                    }
                } else {
                    responses.push(vec![IAC, DONT, option]);
                }
            }
            WONT => {
                self.remote_options.remove(&option);
                responses.push(vec![IAC, DONT, option]);
            }
            DO => {
                if supports_local_option(option) {
                    if self.local_options.insert(option) {
                        responses.push(vec![IAC, WILL, option]);
                        match option {
                            TERMINAL_TYPE => responses.push(terminal_type_response(settings)),
                            NAWS => responses.push(naws_response(settings)),
                            _ => {}
                        }
                    }
                } else {
                    responses.push(vec![IAC, WONT, option]);
                }
            }
            DONT => {
                self.local_options.remove(&option);
                responses.push(vec![IAC, WONT, option]);
            }
            _ => {}
        }
    }
}

fn accepts_remote_option(option: u8) -> bool {
    matches!(option, ECHO | SUPPRESS_GO_AHEAD)
}

fn supports_local_option(option: u8) -> bool {
    matches!(option, SUPPRESS_GO_AHEAD | TERMINAL_TYPE | NAWS)
}

fn terminal_type_response(options: &TelnetOptions) -> Vec<u8> {
    let mut response = vec![IAC, SB, TERMINAL_TYPE, TERMINAL_TYPE_IS];
    response.extend_from_slice(options.terminal.as_bytes());
    response.extend_from_slice(&[IAC, SE]);
    response
}

fn naws_response(options: &TelnetOptions) -> Vec<u8> {
    let mut response = vec![IAC, SB, NAWS];
    response.extend_from_slice(&options.columns.to_be_bytes());
    response.extend_from_slice(&options.rows.to_be_bytes());
    response.extend_from_slice(&[IAC, SE]);
    response
}

/// A connected, plaintext Telnet stream.
pub struct TelnetConnection {
    options: TelnetOptions,
    reader: OwnedReadHalf,
    writer: Arc<AsyncMutex<OwnedWriteHalf>>,
    codec: TelnetCodec,
    pending: VecDeque<u8>,
    lifecycle: Mutex<ConnectionLifecycle>,
}

impl std::fmt::Debug for TelnetConnection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TelnetConnection")
            .field("host", &self.options.host)
            .field("port", &self.options.port)
            .field("encoding", &self.options.encoding)
            .field("state", &self.state())
            .finish()
    }
}

impl TelnetConnection {
    pub async fn connect(options: TelnetOptions) -> Result<Self, TelnetError> {
        options.validate()?;
        let stream = tokio::time::timeout(
            options.connect_timeout,
            TcpStream::connect((options.host.as_str(), options.port)),
        )
        .await
        .map_err(|_| TelnetError::Timeout)?
        .map_err(map_connect_error)?;
        stream.set_nodelay(true).map_err(TelnetError::Io)?;

        let (reader, writer) = stream.into_split();
        let mut lifecycle = ConnectionLifecycle::new();
        lifecycle
            .apply(ConnectionEvent::BeginConnect)
            .expect("Telnet must begin in Created");
        lifecycle
            .apply(ConnectionEvent::BeginAuthentication)
            .expect("Telnet negotiation follows connect");
        lifecycle
            .apply(ConnectionEvent::AuthenticationSucceeded)
            .expect("Telnet negotiation completes the connection setup");

        let connection = Self {
            options,
            reader,
            writer: Arc::new(AsyncMutex::new(writer)),
            codec: TelnetCodec::default(),
            pending: VecDeque::new(),
            lifecycle: Mutex::new(lifecycle),
        };
        connection.send_initial_capabilities().await?;
        Ok(connection)
    }

    pub async fn connect_with_retries(
        options: TelnetOptions,
        policy: TelnetReconnectPolicy,
    ) -> Result<Self, TelnetError> {
        if policy.max_attempts == 0 || policy.initial_backoff.is_zero() {
            return Err(TelnetError::InvalidOptions);
        }
        let mut last_error = None;
        for attempt in 0..policy.max_attempts {
            match Self::connect(options.clone()).await {
                Ok(connection) => return Ok(connection),
                Err(error) => last_error = Some(error),
            }
            if attempt + 1 < policy.max_attempts {
                let multiplier = 1_u32 << attempt.min(7);
                tokio::time::sleep(policy.initial_backoff.saturating_mul(multiplier)).await;
            }
        }
        Err(last_error.unwrap_or(TelnetError::Cancelled))
    }

    pub fn state(&self) -> ConnectionState {
        self.lifecycle
            .lock()
            .expect("Telnet lifecycle lock poisoned")
            .state()
    }

    pub fn encoding(&self) -> TelnetEncoding {
        self.options.encoding
    }

    pub async fn read(&mut self, destination: &mut [u8]) -> Result<usize, TelnetError> {
        if destination.is_empty() {
            return Ok(0);
        }
        if self.state() != ConnectionState::Connected {
            return if self.state() == ConnectionState::Reconnecting {
                Ok(0)
            } else {
                Err(TelnetError::Closed)
            };
        }

        loop {
            if !self.pending.is_empty() {
                return Ok(pop_pending(&mut self.pending, destination));
            }

            let mut raw = vec![0_u8; MAX_READ_BYTES];
            let read = self.reader.read(&mut raw).await.map_err(TelnetError::Io)?;
            if read == 0 {
                self.lifecycle
                    .lock()
                    .expect("Telnet lifecycle lock poisoned")
                    .apply(ConnectionEvent::ConnectionLost)
                    .map_err(|error| TelnetError::Protocol(error.to_string()))?;
                return Ok(0);
            }
            let decoded = self.codec.feed(&raw[..read], &self.options)?;
            for response in decoded.responses {
                self.write_control(response).await?;
            }
            self.pending.extend(decoded.data);
        }
    }

    pub async fn read_text(&mut self, destination: &mut String) -> Result<usize, TelnetError> {
        let mut bytes = vec![0_u8; MAX_READ_BYTES.min(8192)];
        let read = self.read(&mut bytes).await?;
        if read > 0 {
            destination.push_str(&self.options.encoding.decode(&bytes[..read]));
        }
        Ok(read)
    }

    pub async fn write(&self, bytes: &[u8]) -> Result<usize, TelnetError> {
        if self.state() != ConnectionState::Connected {
            return Err(TelnetError::Closed);
        }
        let mut escaped = Vec::with_capacity(bytes.len());
        for &byte in bytes {
            escaped.push(byte);
            if byte == IAC {
                escaped.push(IAC);
            }
        }
        self.write_control(escaped).await?;
        Ok(bytes.len())
    }

    pub async fn write_text(&self, text: &str) -> Result<usize, TelnetError> {
        let bytes = self.options.encoding.encode(text);
        self.write(&bytes).await
    }

    pub async fn resize(&mut self, columns: u16, rows: u16) -> Result<(), TelnetError> {
        if columns == 0 || rows == 0 {
            return Err(TelnetError::InvalidOptions);
        }
        self.options.columns = columns;
        self.options.rows = rows;
        if self.codec.local_options.contains(&NAWS) {
            self.write_control(naws_response(&self.options)).await?;
        }
        Ok(())
    }

    pub async fn reconnect(&mut self) -> Result<(), TelnetError> {
        if self.state() == ConnectionState::Connected {
            self.lifecycle
                .lock()
                .expect("Telnet lifecycle lock poisoned")
                .apply(ConnectionEvent::ConnectionLost)
                .map_err(|error| TelnetError::Protocol(error.to_string()))?;
        }
        self.lifecycle
            .lock()
            .expect("Telnet lifecycle lock poisoned")
            .apply(ConnectionEvent::BeginReconnect)
            .map_err(|error| TelnetError::Protocol(error.to_string()))?;

        let result = tokio::time::timeout(
            self.options.connect_timeout,
            TcpStream::connect((self.options.host.as_str(), self.options.port)),
        )
        .await
        .map_err(|_| TelnetError::Timeout)
        .and_then(|result| result.map_err(map_connect_error));
        let stream = match result {
            Ok(stream) => stream,
            Err(error) => {
                let _ = self
                    .lifecycle
                    .lock()
                    .expect("Telnet lifecycle lock poisoned")
                    .apply(ConnectionEvent::Fail);
                return Err(error);
            }
        };
        stream.set_nodelay(true).map_err(TelnetError::Io)?;
        let (reader, writer) = stream.into_split();
        self.reader = reader;
        self.writer = Arc::new(AsyncMutex::new(writer));
        self.codec = TelnetCodec::default();
        self.pending.clear();
        self.lifecycle
            .lock()
            .expect("Telnet lifecycle lock poisoned")
            .apply(ConnectionEvent::BeginAuthentication)
            .map_err(|error| TelnetError::Protocol(error.to_string()))?;
        self.lifecycle
            .lock()
            .expect("Telnet lifecycle lock poisoned")
            .apply(ConnectionEvent::AuthenticationSucceeded)
            .map_err(|error| TelnetError::Protocol(error.to_string()))?;
        self.send_initial_capabilities().await
    }

    pub async fn cancel(&mut self) -> Result<(), TelnetError> {
        if matches!(
            self.state(),
            ConnectionState::Connected
                | ConnectionState::Reconnecting
                | ConnectionState::Connecting
                | ConnectionState::Authenticating
        ) {
            self.lifecycle
                .lock()
                .expect("Telnet lifecycle lock poisoned")
                .apply(ConnectionEvent::Cancel)
                .map_err(|error| TelnetError::Protocol(error.to_string()))?;
        }
        self.shutdown_writer().await
    }

    pub async fn close(&mut self) -> Result<(), TelnetError> {
        if matches!(
            self.state(),
            ConnectionState::Connected
                | ConnectionState::Reconnecting
                | ConnectionState::Connecting
                | ConnectionState::Authenticating
        ) {
            self.lifecycle
                .lock()
                .expect("Telnet lifecycle lock poisoned")
                .apply(ConnectionEvent::DisconnectRequested)
                .map_err(|error| TelnetError::Protocol(error.to_string()))?;
        }
        let result = self.shutdown_writer().await;
        if self.state() == ConnectionState::Disconnecting {
            self.lifecycle
                .lock()
                .expect("Telnet lifecycle lock poisoned")
                .apply(ConnectionEvent::Disconnected)
                .map_err(|error| TelnetError::Protocol(error.to_string()))?;
        }
        result
    }

    async fn send_initial_capabilities(&self) -> Result<(), TelnetError> {
        self.write_control(vec![
            IAC,
            WILL,
            SUPPRESS_GO_AHEAD,
            IAC,
            WILL,
            TERMINAL_TYPE,
            IAC,
            WILL,
            NAWS,
        ])
        .await
    }

    async fn write_control(&self, bytes: Vec<u8>) -> Result<(), TelnetError> {
        tokio::time::timeout(self.options.operation_timeout, async {
            let mut writer = self.writer.lock().await;
            writer.write_all(&bytes).await.map_err(TelnetError::Io)
        })
        .await
        .map_err(|_| TelnetError::Timeout)?
    }

    async fn shutdown_writer(&self) -> Result<(), TelnetError> {
        tokio::time::timeout(self.options.operation_timeout, async {
            let mut writer = self.writer.lock().await;
            writer.shutdown().await.map_err(TelnetError::Io)
        })
        .await
        .map_err(|_| TelnetError::Timeout)?
    }
}

fn pop_pending(pending: &mut VecDeque<u8>, destination: &mut [u8]) -> usize {
    let count = pending.len().min(destination.len());
    for byte in &mut destination[..count] {
        *byte = pending
            .pop_front()
            .expect("count is bounded by the pending buffer length");
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    #[test]
    fn codec_filters_iac_and_answers_supported_options() {
        let options = TelnetOptions::new("fixture", 23);
        let mut codec = TelnetCodec::default();
        let output = codec
            .feed(&[IAC, DO, TERMINAL_TYPE, b'o', IAC, IAC, b'k'], &options)
            .unwrap();

        assert_eq!(output.data, b"o\xffk");
        assert!(
            output
                .responses
                .iter()
                .any(|response| response.starts_with(&[IAC, WILL, TERMINAL_TYPE]))
        );
        assert!(output.responses.iter().any(|response| {
            response.starts_with(&[IAC, SB, TERMINAL_TYPE, TERMINAL_TYPE_IS])
                && response.ends_with(&[IAC, SE])
        }));
    }

    #[test]
    fn codec_preserves_partial_control_frames_and_bounds_subnegotiation() {
        let options = TelnetOptions::default();
        let mut codec = TelnetCodec::default();
        assert!(codec.feed(&[IAC, DO], &options).unwrap().data.is_empty());
        let output = codec.feed(&[TERMINAL_TYPE, b'x'], &options).unwrap();
        assert_eq!(output.data, b"x");

        let oversized = vec![b'x'; MAX_SUBNEGOTIATION_BYTES + 1];
        let mut frame = vec![IAC, SB, TERMINAL_TYPE];
        frame.extend_from_slice(&oversized);
        assert!(matches!(
            codec.feed(&frame, &options),
            Err(TelnetError::Protocol(_))
        ));
    }

    #[test]
    fn options_reject_control_characters_and_invalid_dimensions() {
        let mut options = TelnetOptions::new("fixture", 23);
        options.terminal = "xterm\n".into();
        assert!(matches!(
            options.validate(),
            Err(TelnetError::InvalidOptions)
        ));
        options.terminal = "xterm".into();
        options.columns = 0;
        assert!(matches!(
            options.validate(),
            Err(TelnetError::InvalidOptions)
        ));
    }

    #[test]
    fn connect_errors_are_typed_without_raw_host_details() {
        assert!(matches!(
            map_connect_error(io::Error::from(io::ErrorKind::ConnectionRefused)),
            TelnetError::ConnectionRefused
        ));
        assert_eq!(
            map_connect_error(io::Error::other("private-host-detail")).to_string(),
            "Telnet network connection failed"
        );
        assert!(
            !map_connect_error(io::Error::other("private-host-detail"))
                .to_string()
                .contains("private-host-detail")
        );
    }

    #[test]
    fn local_fixture_negotiates_terminal_and_round_trips_plaintext() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut initial = [0_u8; 64];
                let initial_len = socket.read(&mut initial).await.unwrap();
                assert!(
                    initial[..initial_len]
                        .windows(3)
                        .any(|window| { window == [IAC, WILL, TERMINAL_TYPE] })
                );
                socket
                    .write_all(&[
                        IAC,
                        DO,
                        TERMINAL_TYPE,
                        IAC,
                        DO,
                        NAWS,
                        b'w',
                        b'e',
                        b'l',
                        b'c',
                        b'o',
                        b'm',
                        b'e',
                        b' ',
                        IAC,
                        IAC,
                        b'!',
                    ])
                    .await
                    .unwrap();

                let mut negotiation = Vec::new();
                while !(negotiation
                    .windows(3)
                    .any(|window| window == [IAC, WILL, TERMINAL_TYPE])
                    && negotiation
                        .windows(4)
                        .any(|window| window == [IAC, SB, TERMINAL_TYPE, TERMINAL_TYPE_IS]))
                {
                    let mut chunk = [0_u8; 256];
                    let chunk_len = socket.read(&mut chunk).await.unwrap();
                    assert!(chunk_len > 0);
                    negotiation.extend_from_slice(&chunk[..chunk_len]);
                }

                socket.write_all(b"ready\n").await.unwrap();
                let mut input = Vec::new();
                while !input.windows(6).any(|window| window == b"ping\r\n") {
                    let mut chunk = [0_u8; 32];
                    let chunk_len = socket.read(&mut chunk).await.unwrap();
                    assert!(chunk_len > 0);
                    input.extend_from_slice(&chunk[..chunk_len]);
                }
                socket.write_all(b"pong\n").await.unwrap();
            });

            let mut options = TelnetOptions::new(address.ip().to_string(), address.port());
            options.terminal = "mobarust-test".into();
            let mut connection = TelnetConnection::connect(options).await.unwrap();

            let mut received = Vec::new();
            while !received.ends_with(b"ready\n") {
                let mut chunk = [0_u8; 64];
                let chunk_len = connection.read(&mut chunk).await.unwrap();
                assert!(chunk_len > 0);
                received.extend_from_slice(&chunk[..chunk_len]);
            }
            assert_eq!(received, b"welcome \xff!ready\n");
            assert_eq!(connection.write(b"ping\r\n").await.unwrap(), 6);

            let mut response = String::new();
            connection.read_text(&mut response).await.unwrap();
            assert_eq!(response, "pong\n");
            server.await.unwrap();
            connection.close().await.unwrap();
            assert_eq!(connection.state(), ConnectionState::Disconnected);
        });
    }

    #[test]
    fn close_and_cancel_are_explicit_lifecycle_operations() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (_socket, _) = listener.accept().await.unwrap();
                tokio::time::sleep(Duration::from_secs(1)).await;
            });
            let options = TelnetOptions::new(address.ip().to_string(), address.port());
            let mut connection = TelnetConnection::connect(options).await.unwrap();
            connection.cancel().await.unwrap();
            assert_eq!(connection.state(), ConnectionState::Cancelled);
            server.abort();
        });
    }
}
