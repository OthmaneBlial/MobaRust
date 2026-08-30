//! Local TLS surface for the isolated IronRDP experiment.
//!
//! The published IronRDP TLS crate currently accepts every server certificate.
//! This small compatibility crate keeps the API consumed by `ironrdp-client`
//! but uses `rustls-platform-verifier` instead. It is deliberately scoped to
//! the helper workspace and is not a claim that RDP is production-ready.

#![forbid(unsafe_code)]

#[cfg(feature = "rustls")]
#[path = "rustls.rs"]
mod implementation;

#[cfg(feature = "stub")]
#[path = "stub.rs"]
mod implementation;

#[cfg(any(
    not(any(feature = "stub", feature = "rustls")),
    all(feature = "stub", feature = "rustls"),
))]
compile_error!("select exactly one TLS backend: `rustls` or `stub`");

#[cfg(any(feature = "stub", feature = "rustls"))]
pub use implementation::{TlsStream, negotiated, upgrade};

/// Validate a destination using the same native TLS server-name parser used by
/// [`upgrade`]. This is intentionally a pure pre-connect check so callers can
/// reject malformed metadata before opening a socket.
#[cfg(any(feature = "stub", feature = "rustls"))]
pub fn validate_server_name(value: &str) -> std::io::Result<()> {
    implementation::validate_server_name(value)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NegotiatedTls {
    pub version: Option<String>,
    pub cipher_suite: Option<String>,
}

#[cfg(feature = "rustls")]
pub fn extract_tls_server_public_key(cert: &x509_cert::Certificate) -> Option<&[u8]> {
    cert.tbs_certificate
        .subject_public_key_info
        .subject_public_key
        .as_bytes()
}

#[cfg(feature = "stub")]
pub fn extract_tls_server_public_key(_cert: &()) -> Option<&[u8]> {
    None
}
