use std::io;

use rustls_platform_verifier::ConfigVerifierExt as _;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _};
use tokio_rustls::rustls;
use tokio_rustls::rustls::pki_types::ServerName;

pub type TlsStream<S> = tokio_rustls::client::TlsStream<S>;

fn server_name_from_target(value: &str) -> io::Result<ServerName<'static>> {
    ServerName::try_from(value.to_owned())
        .map_err(|error| io::Error::other(format!("invalid RDP server name: {error}")))
}

pub fn validate_server_name(value: &str) -> io::Result<()> {
    server_name_from_target(value).map(|_| ())
}

pub async fn upgrade<S>(
    stream: S,
    server_name: &str,
) -> io::Result<(TlsStream<S>, x509_cert::Certificate)>
where
    S: Unpin + AsyncRead + AsyncWrite,
{
    let mut config = rustls::ClientConfig::with_platform_verifier()
        .map_err(|error| io::Error::other(format!("platform certificate verifier: {error}")))?;
    config.resumption = rustls::client::Resumption::disabled();

    let server_name = server_name_from_target(server_name)?;
    let mut tls_stream = tokio_rustls::TlsConnector::from(std::sync::Arc::new(config))
        .connect(server_name, stream)
        .await?;
    tls_stream.flush().await?;

    let certificate = {
        use x509_cert::der::Decode as _;

        let der = tls_stream
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|certificates| certificates.first())
            .ok_or_else(|| io::Error::other("peer certificate is missing"))?;
        x509_cert::Certificate::from_der(der).map_err(io::Error::other)?
    };

    Ok((tls_stream, certificate))
}

pub fn negotiated<S>(stream: &TlsStream<S>) -> super::NegotiatedTls {
    let (_, connection) = stream.get_ref();
    super::NegotiatedTls {
        version: connection
            .protocol_version()
            .map(|version| format!("{version:?}")),
        cipher_suite: connection
            .negotiated_cipher_suite()
            .map(|suite| format!("{:?}", suite.suite())),
    }
}
