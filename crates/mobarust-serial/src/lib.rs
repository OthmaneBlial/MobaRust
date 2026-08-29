//! Rust-owned serial terminal transport.
//!
//! Opening a serial device is an explicit native operation. The crate never
//! enumerates devices implicitly and its tests do not open `/dev`, USB serial
//! adapters, or any other real hardware.

use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mobarust_core::{ConnectionEvent, ConnectionLifecycle, ConnectionState};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_READ_BYTES: usize = 64 * 1024;

#[derive(Debug, Error)]
pub enum SerialError {
    #[error("serial device and baud rate are required")]
    InvalidOptions,
    #[error("serial device could not be opened: {0}")]
    Open(String),
    #[error("serial device disconnected during {operation}: {detail}")]
    DeviceDisconnected {
        operation: &'static str,
        detail: String,
    },
    #[error("serial I/O failed during {operation}: {detail}")]
    Io {
        operation: &'static str,
        detail: String,
    },
    #[error("serial operation timed out")]
    Timeout,
    #[error("serial connection is closed")]
    Closed,
    #[error("serial connection was cancelled")]
    Cancelled,
    #[error("serial lifecycle error: {0}")]
    Lifecycle(String),
    #[error("serial worker failed: {0}")]
    Worker(String),
}

impl SerialError {
    fn is_device_loss(&self) -> bool {
        matches!(self, Self::DeviceDisconnected { .. })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SerialDataBits {
    Five,
    Six,
    Seven,
    #[default]
    Eight,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SerialStopBits {
    #[default]
    One,
    Two,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SerialParity {
    #[default]
    None,
    Odd,
    Even,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SerialFlowControl {
    #[default]
    None,
    Software,
    Hardware,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LineEnding {
    None,
    #[default]
    CrLf,
    Cr,
    Lf,
}

impl LineEnding {
    pub fn suffix(self) -> &'static [u8] {
        match self {
            Self::None => b"",
            Self::CrLf => b"\r\n",
            Self::Cr => b"\r",
            Self::Lf => b"\n",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SerialOptions {
    pub device: String,
    pub baud_rate: u32,
    #[serde(default)]
    pub data_bits: SerialDataBits,
    #[serde(default)]
    pub stop_bits: SerialStopBits,
    #[serde(default)]
    pub parity: SerialParity,
    #[serde(default)]
    pub flow_control: SerialFlowControl,
    #[serde(default)]
    pub line_ending: LineEnding,
    #[serde(default = "default_io_timeout")]
    pub io_timeout: Duration,
    #[serde(default = "default_open_timeout")]
    pub open_timeout: Duration,
}

impl SerialOptions {
    pub fn new(device: impl Into<String>, baud_rate: u32) -> Self {
        Self {
            device: device.into(),
            baud_rate,
            data_bits: SerialDataBits::default(),
            stop_bits: SerialStopBits::default(),
            parity: SerialParity::default(),
            flow_control: SerialFlowControl::default(),
            line_ending: LineEnding::default(),
            io_timeout: default_io_timeout(),
            open_timeout: default_open_timeout(),
        }
    }

    pub fn validate(&self) -> Result<(), SerialError> {
        if self.device.trim().is_empty()
            || self.device.contains('\0')
            || Path::new(&self.device)
                .to_str()
                .is_some_and(|path| path.chars().any(char::is_control))
            || self.baud_rate == 0
            || self.io_timeout.is_zero()
            || self.open_timeout.is_zero()
        {
            return Err(SerialError::InvalidOptions);
        }
        Ok(())
    }

    pub fn frame_text(&self, text: &str) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(text.len() + self.line_ending.suffix().len());
        bytes.extend_from_slice(text.as_bytes());
        bytes.extend_from_slice(self.line_ending.suffix());
        bytes
    }
}

impl Default for SerialOptions {
    fn default() -> Self {
        Self::new("", 115_200)
    }
}

fn default_io_timeout() -> Duration {
    Duration::from_millis(250)
}

fn default_open_timeout() -> Duration {
    Duration::from_secs(5)
}

type PortHandle = Arc<Mutex<Option<Box<dyn serialport::SerialPort>>>>;

/// A native serial connection. All blocking driver calls run outside the
/// async executor and are bounded by the driver's configured I/O timeout.
pub struct SerialConnection {
    options: SerialOptions,
    port: PortHandle,
    lifecycle: Mutex<ConnectionLifecycle>,
}

impl std::fmt::Debug for SerialConnection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SerialConnection")
            .field("device", &self.options.device)
            .field("baud_rate", &self.options.baud_rate)
            .field("state", &self.state())
            .finish()
    }
}

impl SerialConnection {
    pub async fn connect(options: SerialOptions) -> Result<Self, SerialError> {
        options.validate()?;
        let open_options = options.clone();
        let worker = tokio::task::spawn_blocking(move || open_port(&open_options));
        let port = tokio::time::timeout(options.open_timeout, worker)
            .await
            .map_err(|_| SerialError::Timeout)?
            .map_err(|error| SerialError::Worker(error.to_string()))??;

        let mut lifecycle = ConnectionLifecycle::new();
        lifecycle
            .apply(ConnectionEvent::BeginConnect)
            .map_err(|error| SerialError::Lifecycle(error.to_string()))?;
        lifecycle
            .apply(ConnectionEvent::BeginAuthentication)
            .map_err(|error| SerialError::Lifecycle(error.to_string()))?;
        lifecycle
            .apply(ConnectionEvent::AuthenticationSucceeded)
            .map_err(|error| SerialError::Lifecycle(error.to_string()))?;

        Ok(Self {
            options,
            port: Arc::new(Mutex::new(Some(port))),
            lifecycle: Mutex::new(lifecycle),
        })
    }

    pub fn state(&self) -> ConnectionState {
        self.lifecycle
            .lock()
            .expect("serial lifecycle lock poisoned")
            .state()
    }

    pub fn options(&self) -> &SerialOptions {
        &self.options
    }

    pub async fn read(&self, maximum: usize) -> Result<Vec<u8>, SerialError> {
        if self.state() != ConnectionState::Connected {
            return Err(SerialError::Closed);
        }
        let maximum = maximum.clamp(1, MAX_READ_BYTES);
        let port = Arc::clone(&self.port);
        let operation = tokio::task::spawn_blocking(move || {
            let mut guard = port
                .lock()
                .map_err(|_| SerialError::Worker("serial port lock poisoned".into()))?;
            let Some(port) = guard.as_mut() else {
                return Err(SerialError::Closed);
            };
            let mut buffer = vec![0_u8; maximum];
            match std::io::Read::read(&mut **port, &mut buffer) {
                Ok(read) => {
                    buffer.truncate(read);
                    Ok(buffer)
                }
                Err(error) => Err(classify_io_error("read", error)),
            }
        });
        let result = tokio::time::timeout(self.options.io_timeout.saturating_mul(2), operation)
            .await
            .map_err(|_| SerialError::Timeout)?
            .map_err(|error| SerialError::Worker(error.to_string()))?;
        if let Err(error) = &result
            && error.is_device_loss()
        {
            self.mark_lost();
        }
        result
    }

    pub async fn write(&self, bytes: &[u8]) -> Result<usize, SerialError> {
        if self.state() != ConnectionState::Connected {
            return Err(SerialError::Closed);
        }
        let bytes = bytes.to_vec();
        let length = bytes.len();
        let port = Arc::clone(&self.port);
        let operation = tokio::task::spawn_blocking(move || {
            let mut guard = port
                .lock()
                .map_err(|_| SerialError::Worker("serial port lock poisoned".into()))?;
            let Some(port) = guard.as_mut() else {
                return Err(SerialError::Closed);
            };
            std::io::Write::write_all(&mut **port, &bytes)
                .map(|()| length)
                .map_err(|error| classify_io_error("write", error))
        });
        let result = tokio::time::timeout(self.options.io_timeout.saturating_mul(2), operation)
            .await
            .map_err(|_| SerialError::Timeout)?
            .map_err(|error| SerialError::Worker(error.to_string()))?;
        if let Err(error) = &result
            && error.is_device_loss()
        {
            self.mark_lost();
        }
        result
    }

    pub async fn write_text(&self, text: &str) -> Result<usize, SerialError> {
        self.write(&self.options.frame_text(text)).await
    }

    pub async fn reconnect(&self) -> Result<(), SerialError> {
        self.lifecycle
            .lock()
            .expect("serial lifecycle lock poisoned")
            .apply(ConnectionEvent::BeginReconnect)
            .map_err(|error| SerialError::Lifecycle(error.to_string()))?;
        let options = self.options.clone();
        let open_options = options.clone();
        let worker = tokio::task::spawn_blocking(move || open_port(&open_options));
        let new_port = match tokio::time::timeout(options.open_timeout, worker).await {
            Err(_) => {
                self.mark_failed();
                return Err(SerialError::Timeout);
            }
            Ok(Err(error)) => {
                self.mark_failed();
                return Err(SerialError::Worker(error.to_string()));
            }
            Ok(Ok(Err(error))) => {
                self.mark_failed();
                return Err(error);
            }
            Ok(Ok(Ok(port))) => port,
        };
        *self
            .port
            .lock()
            .map_err(|_| SerialError::Worker("serial port lock poisoned".into()))? = Some(new_port);
        let mut lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| SerialError::Worker("serial lifecycle lock poisoned".into()))?;
        lifecycle
            .apply(ConnectionEvent::BeginAuthentication)
            .map_err(|error| SerialError::Lifecycle(error.to_string()))?;
        lifecycle
            .apply(ConnectionEvent::AuthenticationSucceeded)
            .map_err(|error| SerialError::Lifecycle(error.to_string()))?;
        Ok(())
    }

    pub fn mark_lost(&self) {
        if let Ok(mut lifecycle) = self.lifecycle.lock() {
            let _ = lifecycle.apply(ConnectionEvent::ConnectionLost);
        }
    }

    fn mark_failed(&self) {
        if let Ok(mut lifecycle) = self.lifecycle.lock() {
            let _ = lifecycle.apply(ConnectionEvent::Fail);
        }
    }

    pub async fn cancel(&self) -> Result<(), SerialError> {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| SerialError::Worker("serial lifecycle lock poisoned".into()))?;
        if matches!(
            lifecycle.state(),
            ConnectionState::Connected
                | ConnectionState::Reconnecting
                | ConnectionState::Connecting
                | ConnectionState::Authenticating
        ) {
            lifecycle
                .apply(ConnectionEvent::Cancel)
                .map_err(|error| SerialError::Lifecycle(error.to_string()))?;
        }
        drop(lifecycle);
        self.take_port()?;
        Ok(())
    }

    pub async fn close(&self) -> Result<(), SerialError> {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| SerialError::Worker("serial lifecycle lock poisoned".into()))?;
        if matches!(
            lifecycle.state(),
            ConnectionState::Connected
                | ConnectionState::Reconnecting
                | ConnectionState::Connecting
                | ConnectionState::Authenticating
        ) {
            lifecycle
                .apply(ConnectionEvent::DisconnectRequested)
                .map_err(|error| SerialError::Lifecycle(error.to_string()))?;
        }
        drop(lifecycle);
        self.take_port()?;
        let mut lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| SerialError::Worker("serial lifecycle lock poisoned".into()))?;
        if lifecycle.state() == ConnectionState::Disconnecting {
            lifecycle
                .apply(ConnectionEvent::Disconnected)
                .map_err(|error| SerialError::Lifecycle(error.to_string()))?;
        }
        Ok(())
    }

    fn take_port(&self) -> Result<(), SerialError> {
        self.port
            .lock()
            .map_err(|_| SerialError::Worker("serial port lock poisoned".into()))?
            .take();
        Ok(())
    }
}

fn open_port(options: &SerialOptions) -> Result<Box<dyn serialport::SerialPort>, SerialError> {
    let builder = serialport::new(&options.device, options.baud_rate)
        .data_bits(options.data_bits.into())
        .stop_bits(options.stop_bits.into())
        .parity(options.parity.into())
        .flow_control(options.flow_control.into())
        .timeout(options.io_timeout);
    builder
        .open()
        .map_err(|error| SerialError::Open(error.to_string()))
}

fn classify_io_error(operation: &'static str, error: io::Error) -> SerialError {
    let detail = error.to_string();
    if matches!(
        error.kind(),
        io::ErrorKind::NotConnected
            | io::ErrorKind::UnexpectedEof
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::NotFound
    ) {
        SerialError::DeviceDisconnected { operation, detail }
    } else {
        SerialError::Io { operation, detail }
    }
}

impl From<SerialDataBits> for serialport::DataBits {
    fn from(value: SerialDataBits) -> Self {
        match value {
            SerialDataBits::Five => Self::Five,
            SerialDataBits::Six => Self::Six,
            SerialDataBits::Seven => Self::Seven,
            SerialDataBits::Eight => Self::Eight,
        }
    }
}

impl From<SerialStopBits> for serialport::StopBits {
    fn from(value: SerialStopBits) -> Self {
        match value {
            SerialStopBits::One => Self::One,
            SerialStopBits::Two => Self::Two,
        }
    }
}

impl From<SerialParity> for serialport::Parity {
    fn from(value: SerialParity) -> Self {
        match value {
            SerialParity::None => Self::None,
            SerialParity::Odd => Self::Odd,
            SerialParity::Even => Self::Even,
        }
    }
}

impl From<SerialFlowControl> for serialport::FlowControl {
    fn from(value: SerialFlowControl) -> Self {
        match value {
            SerialFlowControl::None => Self::None,
            SerialFlowControl::Software => Self::Software,
            SerialFlowControl::Hardware => Self::Hardware,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_cover_serial_parameters_without_opening_a_device() {
        let mut options = SerialOptions::new("/temporary/fixture-device", 115_200);
        options.data_bits = SerialDataBits::Seven;
        options.stop_bits = SerialStopBits::Two;
        options.parity = SerialParity::Even;
        options.flow_control = SerialFlowControl::Hardware;
        options.line_ending = LineEnding::Lf;
        assert!(options.validate().is_ok());
        assert_eq!(options.frame_text("status"), b"status\n");
    }

    #[test]
    fn invalid_device_options_are_rejected_before_any_open_attempt() {
        let mut options = SerialOptions::new("/temporary/fixture-device", 115_200);
        options.device = "bad\npath".into();
        assert!(matches!(
            options.validate(),
            Err(SerialError::InvalidOptions)
        ));
        options.device = "/temporary/fixture-device".into();
        options.baud_rate = 0;
        assert!(matches!(
            options.validate(),
            Err(SerialError::InvalidOptions)
        ));
    }

    #[test]
    fn device_loss_is_a_distinct_recoverable_error() {
        let error = classify_io_error("read", io::Error::from(io::ErrorKind::NotConnected));
        assert!(matches!(
            error,
            SerialError::DeviceDisconnected {
                operation: "read",
                ..
            }
        ));
    }

    #[test]
    fn lifecycle_can_cancel_a_serial_connection_without_hardware() {
        let mut lifecycle = ConnectionLifecycle::new();
        lifecycle.apply(ConnectionEvent::BeginConnect).unwrap();
        lifecycle
            .apply(ConnectionEvent::BeginAuthentication)
            .unwrap();
        lifecycle
            .apply(ConnectionEvent::AuthenticationSucceeded)
            .unwrap();
        lifecycle.apply(ConnectionEvent::Cancel).unwrap();
        assert_eq!(lifecycle.state(), ConnectionState::Cancelled);
    }
}
