use std::io::{Read, Write};
use std::time::Duration;

use mobarust_serial::{SerialConnection, SerialError, SerialOptions};
use serialport::{SerialPort, TTYPort};
use tokio::time::timeout;

fn fixture_connection() -> (TTYPort, SerialConnection) {
    let (master, slave) = TTYPort::pair().expect("create disposable serial PTY pair");
    let device = slave.name().expect("read disposable PTY name");
    let mut options = SerialOptions::new(device, 115_200);
    options.io_timeout = Duration::from_millis(100);
    options.open_timeout = Duration::from_secs(2);
    let connection = SerialConnection::from_open_port(options, Box::new(slave))
        .expect("adopt disposable PTY through serial transport");
    (master, connection)
}

#[tokio::test]
async fn round_trips_through_a_disposable_pseudo_terminal() {
    let (mut master, connection) = fixture_connection();

    master
        .write_all(b"device-to-host")
        .expect("write from disposable device");
    let received = connection
        .read(64)
        .await
        .expect("read from disposable device");
    assert_eq!(received, b"device-to-host");

    let read_task = tokio::task::spawn_blocking(move || {
        let mut reader = master
            .try_clone()
            .expect("clone disposable PTY master for reading");
        let mut received = vec![0_u8; b"host-to-device".len()];
        reader
            .read_exact(&mut received)
            .expect("read device output");
        received
    });
    assert_eq!(
        connection
            .write(b"host-to-device")
            .await
            .expect("write to disposable device"),
        b"host-to-device".len()
    );
    assert_eq!(read_task.await.expect("join PTY reader"), b"host-to-device");
    connection.close().await.expect("close disposable PTY");
}

#[tokio::test]
async fn disappearing_pseudo_terminal_is_reported_as_device_loss() {
    let (master, connection) = fixture_connection();
    drop(master);

    let result = timeout(Duration::from_secs(2), connection.read(64))
        .await
        .expect("device loss should not hang");
    assert!(matches!(
        result,
        Err(SerialError::DeviceDisconnected { operation: "read" })
    ));
    connection.cancel().await.expect("cancel lost PTY");
}
