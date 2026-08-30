use std::io;

use tokio::io::{AsyncRead, AsyncWrite};

pub type TlsStream<S> = S;

pub fn validate_server_name(_value: &str) -> io::Result<()> {
    Err(io::Error::other("TLS stub backend is not available"))
}

pub async fn upgrade<S>(_stream: S, _server_name: &str) -> io::Result<(TlsStream<S>, ())>
where
    S: Unpin + AsyncRead + AsyncWrite,
{
    Err(io::Error::other("TLS stub backend is not available"))
}

pub fn negotiated<S>(_stream: &TlsStream<S>) -> super::NegotiatedTls {
    super::NegotiatedTls::default()
}
