//! This crate contains methods to deal with SSH keys, as defined in
//! crate Russh. This includes in particular various functions for
//! opening key files, deciphering encrypted keys, and dealing with
//! agents.
//!

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::string::FromUtf8Error;

use aes::cipher::inout::PadError;
use data_encoding::BASE64_MIME;
use thiserror::Error;

use crate::helpers::EncodedExt;

pub mod key;
pub use key::PrivateKeyWithHashAlg;

mod format;
pub use format::*;
// Reexports
pub use signature;
pub use ssh_encoding;
pub use ssh_key::{self, Algorithm, Certificate, EcdsaCurve, HashAlg, PrivateKey, PublicKey};

/// OpenSSH agent protocol implementation
pub mod agent;

#[cfg(not(target_arch = "wasm32"))]
pub mod known_hosts;

#[cfg(not(target_arch = "wasm32"))]
pub use known_hosts::{check_known_hosts, check_known_hosts_path};

pub use crate::cert::PublicKeyOrCertificate;

#[derive(Debug, Error)]
pub enum Error {
    /// The key could not be read, for an unknown reason
    #[error("Could not read key")]
    CouldNotReadKey,
    /// The type of the key is unsupported
    #[error("Unsupported key type {}", key_type_string)]
    UnsupportedKeyType {
        key_type_string: String,
        key_type_raw: Vec<u8>,
    },
    /// The type of the key is unsupported
    #[error("Invalid Ed25519 key data")]
    Ed25519KeyError(#[from] ed25519_dalek::SignatureError),
    /// The type of the key is unsupported
    #[error("Invalid ECDSA key data")]
    EcdsaKeyError(#[from] p256::elliptic_curve::Error),
    /// The key is encrypted (should supply a password?)
    #[error("The key is encrypted")]
    KeyIsEncrypted,
    /// The key contents are inconsistent
    #[error("The key is corrupt")]
    KeyIsCorrupt,
    /// Home directory could not be found
    #[error("No home directory found")]
    NoHomeDir,
    /// The server key has changed
    #[error("The server key changed at line {}", line)]
    KeyChanged { line: usize },
    /// The key uses an unsupported algorithm
    #[error("Unknown key algorithm: {0}")]
    UnknownAlgorithm(::pkcs8::ObjectIdentifier),
    /// Index out of bounds
    #[error("Index out of bounds")]
    IndexOutOfBounds,
    /// Unknown signature type
    #[error("Unknown signature type: {}", sig_type)]
    UnknownSignatureType { sig_type: String },
    #[error("Invalid signature")]
    InvalidSignature,
    #[error("Invalid parameters")]
    InvalidParameters,
    /// Agent protocol error
    #[error("Agent protocol error")]
    AgentProtocolError,
    #[error("Agent failure")]
    AgentFailure,
    #[error(transparent)]
    IO(#[from] std::io::Error),

    #[cfg(feature = "rsa")]
    #[error("Rsa: {0}")]
    Rsa(#[from] rsa::Error),

    #[error(transparent)]
    Pad(#[from] PadError),

    #[error(transparent)]
    Unpad(#[from] aes::cipher::block_padding::Error),

    #[error("Base64 decoding error: {0}")]
    Decode(#[from] data_encoding::DecodeError),
    #[error("Der: {0}")]
    Der(#[from] der::Error),
    #[error("Spki: {0}")]
    Spki(#[from] spki::Error),
    #[cfg(feature = "rsa")]
    #[error("Pkcs1: {0}")]
    Pkcs1(#[from] pkcs1::Error),
    #[error("Pkcs8: {0}")]
    Pkcs8(#[from] ::pkcs8::Error),
    #[error("Sec1: {0}")]
    Sec1(#[from] sec1::Error),

    #[error("SshKey: {0}")]
    SshKey(#[from] ssh_key::Error),
    #[error("SshEncoding: {0}")]
    SshEncoding(#[from] ssh_encoding::Error),

    #[error("Environment variable `{0}` not found")]
    EnvVar(&'static str),
    #[error(
        "Unable to connect to ssh-agent. The environment variable `SSH_AUTH_SOCK` was set, but it \
         points to a nonexistent file or directory."
    )]
    BadAuthSock,

    #[error(transparent)]
    Utf8(#[from] FromUtf8Error),

    #[error("ASN1 decoding error: {0}")]
    #[cfg(feature = "legacy-ed25519-pkcs8-parser")]
    LegacyASN1(::yasna::ASN1Error),

    #[cfg(windows)]
    #[error("Pageant: {0}")]
    Pageant(#[from] pageant::Error),
}

#[cfg(feature = "legacy-ed25519-pkcs8-parser")]
impl From<yasna::ASN1Error> for Error {
    fn from(e: yasna::ASN1Error) -> Error {
        Error::LegacyASN1(e)
    }
}

/// Load a public key from a file. Ed25519, EC-DSA and RSA keys are supported.
///
/// ```
/// russh::keys::load_public_key("../files/id_ed25519.pub").unwrap();
/// ```
pub fn load_public_key<P: AsRef<Path>>(path: P) -> Result<ssh_key::PublicKey, Error> {
    let mut pubkey = String::new();
    let mut file = File::open(path.as_ref())?;
    file.read_to_string(&mut pubkey)?;

    let mut split = pubkey.split_whitespace();
    match (split.next(), split.next()) {
        (Some(_), Some(key)) => parse_public_key_base64(key),
        (Some(key), None) => parse_public_key_base64(key),
        _ => Err(Error::CouldNotReadKey),
    }
}

/// Reads a public key from the standard encoding. In some cases, the
/// encoding is prefixed with a key type identifier and a space (such
/// as `ssh-ed25519 AAAAC3N...`).
///
/// ```
/// russh::keys::parse_public_key_base64("AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ").is_ok();
/// ```
pub fn parse_public_key_base64(key: &str) -> Result<ssh_key::PublicKey, Error> {
    let base = BASE64_MIME.decode(key.as_bytes())?;
    key::parse_public_key(&base)
}

pub trait PublicKeyBase64 {
    /// Create the base64 part of the public key blob.
    fn public_key_bytes(&self) -> Vec<u8>;
    fn public_key_base64(&self) -> String {
        let mut s = BASE64_MIME.encode(&self.public_key_bytes());
        assert_eq!(s.pop(), Some('\n'));
        assert_eq!(s.pop(), Some('\r'));
        s.replace("\r\n", "")
    }
}

impl PublicKeyBase64 for ssh_key::PublicKey {
    fn public_key_bytes(&self) -> Vec<u8> {
        self.key_data().encoded().unwrap_or_default()
    }
}

impl PublicKeyBase64 for PrivateKey {
    fn public_key_bytes(&self) -> Vec<u8> {
        self.public_key().public_key_bytes()
    }
}

/// Load a secret key, deciphering it with the supplied password if necessary.
pub fn load_secret_key<P: AsRef<Path>>(
    secret_: P,
    password: Option<&str>,
) -> Result<PrivateKey, Error> {
    let mut secret_file = std::fs::File::open(secret_)?;
    let mut secret = String::new();
    secret_file.read_to_string(&mut secret)?;
    decode_secret_key(&secret, password)
}

/// Load a openssh certificate
pub fn load_openssh_certificate<P: AsRef<Path>>(cert_: P) -> Result<Certificate, ssh_key::Error> {
    let mut cert_file = std::fs::File::open(cert_)?;
    let mut cert = String::new();
    cert_file.read_to_string(&mut cert)?;

    Certificate::from_openssh(&cert)
}

fn is_base64_char(c: char) -> bool {
    c.is_ascii_lowercase()
        || c.is_ascii_uppercase()
        || c.is_ascii_digit()
        || c == '/'
        || c == '+'
        || c == '='
}
