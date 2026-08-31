use std::io;
#[cfg(feature = "fixture-ca")]
use std::io::BufReader;
#[cfg(feature = "fixture-ca")]
use std::path::Path;

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

#[cfg(feature = "fixture-ca")]
fn fixture_root_store() -> io::Result<Option<rustls::RootCertStore>> {
    let Some(path) = std::env::var_os("MOBARUST_RDP_FIXTURE_CA") else {
        return Ok(None);
    };
    let path = Path::new(&path);
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| io::Error::other(format!("inspect test-only RDP fixture CA: {error}")))?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::other(
            "test-only RDP fixture CA must be a regular file",
        ));
    }
    let temporary_root = std::env::temp_dir()
        .canonicalize()
        .map_err(|error| io::Error::other(format!("resolve test temporary directory: {error}")))?;
    let path = path
        .canonicalize()
        .map_err(|error| io::Error::other(format!("resolve test-only RDP fixture CA: {error}")))?;
    if !path.starts_with(&temporary_root) {
        return Err(io::Error::other(
            "test-only RDP fixture CA must be inside the temporary directory",
        ));
    }
    let file = std::fs::File::open(&path)
        .map_err(|error| io::Error::other(format!("open test-only RDP fixture CA: {error}")))?;
    let mut reader = BufReader::new(file);
    let mut roots = rustls::RootCertStore::empty();
    let mut count = 0usize;
    for certificate in rustls_pemfile::certs(&mut reader) {
        let certificate = certificate
            .map_err(|error| io::Error::other(format!("read test-only RDP fixture CA: {error}")))?;
        roots
            .add(certificate)
            .map_err(|error| io::Error::other(format!("add test-only RDP fixture CA: {error}")))?;
        count += 1;
    }
    if count == 0 {
        return Err(io::Error::other(
            "test-only RDP fixture CA contains no certificates",
        ));
    }
    Ok(Some(roots))
}

#[cfg(feature = "fixture-ca")]
fn client_config() -> io::Result<rustls::ClientConfig> {
    if let Some(roots) = fixture_root_store()? {
        // This branch is compiled only for the explicit local RDP fixture
        // feature. It is never enabled by the packaged helper and is only
        // selected when the test supplies a CA file in its private temp dir.
        return Ok(rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth());
    }
    platform_client_config()
}

#[cfg(not(feature = "fixture-ca"))]
fn client_config() -> io::Result<rustls::ClientConfig> {
    platform_client_config()
}

fn platform_client_config() -> io::Result<rustls::ClientConfig> {
    rustls::ClientConfig::with_platform_verifier()
        .map_err(|error| io::Error::other(format!("platform certificate verifier: {error}")))
}

#[cfg(feature = "fixture-ca")]
pub fn install_fixture_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

pub async fn upgrade<S>(
    stream: S,
    server_name: &str,
) -> io::Result<(TlsStream<S>, x509_cert::Certificate)>
where
    S: Unpin + AsyncRead + AsyncWrite,
{
    let mut config = client_config()?;
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
