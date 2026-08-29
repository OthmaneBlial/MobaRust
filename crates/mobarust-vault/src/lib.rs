//! Native credential storage kept behind a small Rust-only boundary.
//!
//! Session records should store [`CredentialId`] values, never password or key
//! material. The desktop frontend can ask native code to use a reference, but
//! it does not receive the secret as part of the normal session snapshot.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use aes_gcm::aead::consts::U12;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

const DEFAULT_SERVICE: &str = "com.othmane.mobarust";
const PORTABLE_MAGIC: &[u8; 8] = b"MRPVLT01";
const PORTABLE_SALT_BYTES: usize = 16;
const PORTABLE_NONCE_BYTES: usize = 12;
const PORTABLE_KEY_BYTES: usize = 32;
const PORTABLE_HEADER_BYTES: usize =
    PORTABLE_MAGIC.len() + PORTABLE_SALT_BYTES + PORTABLE_NONCE_BYTES;
const PORTABLE_MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
const PORTABLE_MAX_CREDENTIALS: usize = 4096;
const PORTABLE_MAX_SECRET_BYTES: usize = 1024 * 1024;
const PORTABLE_ARGON_MEMORY_KIB: u32 = 64 * 1024;
const PORTABLE_ARGON_ITERATIONS: u32 = 3;
const PORTABLE_ARGON_PARALLELISM: u32 = 1;

/// An opaque, non-secret identifier used by a session to refer to vault data.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct CredentialId(String);

impl CredentialId {
    /// Validate an identifier before it reaches a platform credential store.
    pub fn new(value: impl Into<String>) -> Result<Self, VaultError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));

        if !valid {
            return Err(VaultError::InvalidCredentialId);
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CredentialId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("CredentialId")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for CredentialId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Secret material that is scrubbed when the native boundary drops it.
pub struct SecretMaterial(Zeroizing<String>);

impl SecretMaterial {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    /// Adopt a deserialized zeroizing string without creating a second
    /// plaintext allocation at the native command boundary.
    pub fn from_zeroizing(value: Zeroizing<String>) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretMaterial(<redacted>)")
    }
}

impl Drop for SecretMaterial {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("credential reference must contain only letters, numbers, '.', '_' or '-'")]
    InvalidCredentialId,
    #[error("native credential store error: {0}")]
    Backend(#[from] keyring::Error),
    #[error("portable vault passphrase cannot be empty")]
    EmptyPassphrase,
    #[error("portable vault file is too large")]
    PortableFileTooLarge,
    #[error("portable vault contains too many credentials")]
    PortableCredentialLimit,
    #[error("portable vault credential secret is too large")]
    PortableSecretTooLarge,
    #[error("portable vault credential is missing: {0}")]
    PortableCredentialMissing(String),
    #[error("portable vault passphrase is incorrect or the file is corrupt")]
    PortableAuthenticationFailed,
    #[error("portable vault format is invalid: {0}")]
    PortableFormat(String),
    #[error("portable vault key derivation failed: {0}")]
    PortableKeyDerivation(#[source] argon2::Error),
    #[error("portable vault encryption failed")]
    PortableEncryption,
    #[error("portable vault randomness failed: {0}")]
    PortableRandomness(#[source] getrandom::Error),
    #[error("portable vault file operation failed for {path}: {source}")]
    PortableIo { path: PathBuf, source: io::Error },
    #[error("portable vault already exists: {0}")]
    PortableAlreadyExists(PathBuf),
    #[error("portable vault state is unavailable: {0}")]
    PortableStateUnavailable(String),
}

/// Access to the platform credential store selected by `keyring`.
///
/// On macOS this is Keychain, on Windows Credential Manager, and on supported
/// Unix desktops Secret Service. Portable encrypted storage is intentionally a
/// separate backend and is not silently substituted here.
#[derive(Clone, Debug)]
pub struct PlatformVault {
    service: String,
}

impl Default for PlatformVault {
    fn default() -> Self {
        Self::new(DEFAULT_SERVICE)
    }
}

impl PlatformVault {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    pub fn put(
        &self,
        credential_id: &CredentialId,
        secret: &SecretMaterial,
    ) -> Result<(), VaultError> {
        let entry = keyring::Entry::new(&self.service, credential_id.as_str())?;
        entry.set_password(secret.as_str())?;
        Ok(())
    }

    pub fn get(&self, credential_id: &CredentialId) -> Result<SecretMaterial, VaultError> {
        let entry = keyring::Entry::new(&self.service, credential_id.as_str())?;
        let value = entry.get_password()?;
        Ok(SecretMaterial::new(value))
    }

    pub fn delete(&self, credential_id: &CredentialId) -> Result<(), VaultError> {
        let entry = keyring::Entry::new(&self.service, credential_id.as_str())?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(VaultError::Backend(error)),
        }
    }

    /// Check whether the platform store can be constructed without retrieving
    /// any user credential. This deliberately does not expose a secret.
    pub fn entry(&self, credential_id: &CredentialId) -> Result<(), VaultError> {
        let _entry = keyring::Entry::new(&self.service, credential_id.as_str())?;
        Ok(())
    }
}

/// A narrowly-scoped interface that keeps vault access mockable in higher
/// layers without making the frontend aware of platform credential APIs.
pub trait CredentialStore {
    fn put(&self, credential_id: &CredentialId, secret: &SecretMaterial) -> Result<(), VaultError>;
    fn get(&self, credential_id: &CredentialId) -> Result<SecretMaterial, VaultError>;
    fn delete(&self, credential_id: &CredentialId) -> Result<(), VaultError>;
}

/// Read-only credential lookup used by native protocol adapters. Keeping this
/// trait separate lets an explicitly unlocked portable vault participate in a
/// connection without widening the frontend IPC surface.
pub trait CredentialLookup: Send + Sync {
    fn get(&self, credential_id: &CredentialId) -> Result<SecretMaterial, VaultError>;
}

impl CredentialStore for PlatformVault {
    fn put(&self, credential_id: &CredentialId, secret: &SecretMaterial) -> Result<(), VaultError> {
        Self::put(self, credential_id, secret)
    }

    fn get(&self, credential_id: &CredentialId) -> Result<SecretMaterial, VaultError> {
        Self::get(self, credential_id)
    }

    fn delete(&self, credential_id: &CredentialId) -> Result<(), VaultError> {
        Self::delete(self, credential_id)
    }
}

impl CredentialLookup for PlatformVault {
    fn get(&self, credential_id: &CredentialId) -> Result<SecretMaterial, VaultError> {
        Self::get(self, credential_id)
    }
}

impl CredentialLookup for PortableVault {
    fn get(&self, credential_id: &CredentialId) -> Result<SecretMaterial, VaultError> {
        Self::get(self, credential_id)
    }
}

/// Convert an owned secret to a zeroizing temporary for a native operation.
pub fn zeroizing_secret(value: String) -> Zeroizing<String> {
    Zeroizing::new(value)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PortablePayload {
    schema_version: u32,
    credentials: BTreeMap<String, String>,
}

impl Drop for PortablePayload {
    fn drop(&mut self) {
        for secret in self.credentials.values_mut() {
            secret.zeroize();
        }
    }
}

/// An explicitly unlocked, encrypted vault for portable distributions.
///
/// The file is AES-256-GCM authenticated encryption with an Argon2id-derived
/// key. The passphrase is never written to disk and the derived key is
/// zeroized when this value is dropped. This backend is intentionally not
/// substituted for the platform vault; callers must opt into unlock and
/// handle the locked/unlocked lifecycle explicitly.
pub struct PortableVault {
    path: PathBuf,
    salt: [u8; PORTABLE_SALT_BYTES],
    key: Zeroizing<[u8; PORTABLE_KEY_BYTES]>,
    entries: BTreeMap<String, SecretMaterial>,
}

impl fmt::Debug for PortableVault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PortableVault")
            .field("path", &self.path)
            .field("credential_count", &self.entries.len())
            .finish()
    }
}

impl PortableVault {
    /// Create a new encrypted portable vault. Existing files are never
    /// overwritten by this constructor.
    pub fn create(
        path: impl Into<PathBuf>,
        passphrase: &SecretMaterial,
    ) -> Result<Self, VaultError> {
        let path = path.into();
        if path.exists() {
            return Err(VaultError::PortableAlreadyExists(path));
        }
        let salt = random_bytes::<PORTABLE_SALT_BYTES>()?;
        let key = derive_portable_key(passphrase, &salt)?;
        let vault = Self {
            path,
            salt,
            key,
            entries: BTreeMap::new(),
        };
        vault.persist()?;
        Ok(vault)
    }

    /// Unlock an existing portable vault. Authentication failures do not
    /// distinguish a wrong passphrase from tampered ciphertext.
    pub fn open(path: impl Into<PathBuf>, passphrase: &SecretMaterial) -> Result<Self, VaultError> {
        let path = path.into();
        let bytes = fs::read(&path).map_err(|source| VaultError::PortableIo {
            path: path.clone(),
            source,
        })?;
        if bytes.len() > PORTABLE_MAX_FILE_BYTES {
            return Err(VaultError::PortableFileTooLarge);
        }
        if bytes.len() < PORTABLE_HEADER_BYTES {
            return Err(VaultError::PortableFormat("header is truncated".into()));
        }
        if &bytes[..PORTABLE_MAGIC.len()] != PORTABLE_MAGIC {
            return Err(VaultError::PortableFormat("magic is invalid".into()));
        }
        let mut salt = [0_u8; PORTABLE_SALT_BYTES];
        let salt_start = PORTABLE_MAGIC.len();
        salt.copy_from_slice(&bytes[salt_start..salt_start + PORTABLE_SALT_BYTES]);
        let nonce_start = salt_start + PORTABLE_SALT_BYTES;
        let nonce = &bytes[nonce_start..PORTABLE_HEADER_BYTES];
        let key = derive_portable_key(passphrase, &salt)?;
        let cipher = cipher_from_key(&key)?;
        let nonce = Nonce::<U12>::try_from(nonce)
            .map_err(|_| VaultError::PortableFormat("nonce is invalid".into()))?;
        let plaintext = cipher
            .decrypt(&nonce, &bytes[PORTABLE_HEADER_BYTES..])
            .map(Zeroizing::new)
            .map_err(|_| VaultError::PortableAuthenticationFailed)?;
        let entries = decode_portable_entries(&plaintext)?;
        Ok(Self {
            path,
            salt,
            key,
            entries,
        })
    }

    pub fn list_ids(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    pub fn get(&self, credential_id: &CredentialId) -> Result<SecretMaterial, VaultError> {
        self.entries
            .get(credential_id.as_str())
            .map(|secret| SecretMaterial::new(secret.as_str()))
            .ok_or_else(|| VaultError::PortableCredentialMissing(credential_id.to_string()))
    }

    pub fn put(
        &mut self,
        credential_id: &CredentialId,
        secret: SecretMaterial,
    ) -> Result<(), VaultError> {
        validate_portable_secret(&secret)?;
        let key = credential_id.as_str().to_owned();
        let previous = self.entries.insert(key.clone(), secret);
        if let Err(error) = self.persist() {
            match previous {
                Some(value) => {
                    self.entries.insert(key, value);
                }
                None => {
                    self.entries.remove(&key);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn delete(&mut self, credential_id: &CredentialId) -> Result<bool, VaultError> {
        let key = credential_id.as_str().to_owned();
        let Some(previous) = self.entries.remove(&key) else {
            return Ok(false);
        };
        if let Err(error) = self.persist() {
            self.entries.insert(key, previous);
            return Err(error);
        }
        Ok(true)
    }

    fn persist(&self) -> Result<(), VaultError> {
        if self.entries.len() > PORTABLE_MAX_CREDENTIALS {
            return Err(VaultError::PortableCredentialLimit);
        }
        for secret in self.entries.values() {
            validate_portable_secret(secret)?;
        }
        let payload = PortablePayload {
            schema_version: 1,
            credentials: self
                .entries
                .iter()
                .map(|(id, secret)| (id.clone(), secret.as_str().to_owned()))
                .collect(),
        };
        let plaintext = Zeroizing::new(
            serde_json::to_vec(&payload)
                .map_err(|error| VaultError::PortableFormat(error.to_string()))?,
        );
        drop(payload);
        if plaintext.len() > PORTABLE_MAX_FILE_BYTES - PORTABLE_HEADER_BYTES {
            return Err(VaultError::PortableFileTooLarge);
        }

        let nonce = random_bytes::<PORTABLE_NONCE_BYTES>()?;
        let cipher = cipher_from_key(&self.key)?;
        let nonce = Nonce::<U12>::try_from(nonce.as_slice())
            .map_err(|_| VaultError::PortableFormat("nonce is invalid".into()))?;
        let ciphertext = cipher
            .encrypt(&nonce, plaintext.as_ref())
            .map_err(|_| VaultError::PortableEncryption)?;
        let mut file = Vec::with_capacity(PORTABLE_HEADER_BYTES + ciphertext.len());
        file.extend_from_slice(PORTABLE_MAGIC);
        file.extend_from_slice(&self.salt);
        file.extend_from_slice(&nonce);
        file.extend_from_slice(&ciphertext);
        atomic_write_private(&self.path, &file)
    }
}

fn validate_portable_secret(secret: &SecretMaterial) -> Result<(), VaultError> {
    if secret.as_str().is_empty() {
        return Err(VaultError::EmptyPassphrase);
    }
    if secret.as_str().len() > PORTABLE_MAX_SECRET_BYTES {
        return Err(VaultError::PortableSecretTooLarge);
    }
    Ok(())
}

fn derive_portable_key(
    passphrase: &SecretMaterial,
    salt: &[u8; PORTABLE_SALT_BYTES],
) -> Result<Zeroizing<[u8; PORTABLE_KEY_BYTES]>, VaultError> {
    if passphrase.as_str().is_empty() {
        return Err(VaultError::EmptyPassphrase);
    }
    let params = Params::new(
        PORTABLE_ARGON_MEMORY_KIB,
        PORTABLE_ARGON_ITERATIONS,
        PORTABLE_ARGON_PARALLELISM,
        Some(PORTABLE_KEY_BYTES),
    )
    .map_err(VaultError::PortableKeyDerivation)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0_u8; PORTABLE_KEY_BYTES]);
    argon
        .hash_password_into(passphrase.as_str().as_bytes(), salt, key.as_mut())
        .map_err(VaultError::PortableKeyDerivation)?;
    Ok(key)
}

fn cipher_from_key(key: &[u8; PORTABLE_KEY_BYTES]) -> Result<Aes256Gcm, VaultError> {
    let key = Key::<Aes256Gcm>::try_from(key.as_slice())
        .map_err(|_| VaultError::PortableFormat("key length is invalid".into()))?;
    Ok(Aes256Gcm::new(&key))
}

fn decode_portable_entries(
    plaintext: &[u8],
) -> Result<BTreeMap<String, SecretMaterial>, VaultError> {
    let mut payload: PortablePayload = serde_json::from_slice(plaintext)
        .map_err(|error| VaultError::PortableFormat(error.to_string()))?;
    let schema_version = payload.schema_version;
    let credentials = std::mem::take(&mut payload.credentials);
    if schema_version != 1 {
        return Err(VaultError::PortableFormat(
            "unsupported schema version".into(),
        ));
    }
    if credentials.len() > PORTABLE_MAX_CREDENTIALS {
        return Err(VaultError::PortableCredentialLimit);
    }
    let mut entries = BTreeMap::new();
    for (id, secret) in credentials {
        let credential_id = CredentialId::new(id)?;
        let secret = SecretMaterial::new(secret);
        validate_portable_secret(&secret)?;
        entries.insert(credential_id.to_string(), secret);
    }
    Ok(entries)
}

fn random_bytes<const N: usize>() -> Result<[u8; N], VaultError> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes).map_err(VaultError::PortableRandomness)?;
    Ok(bytes)
}

fn atomic_write_private(path: &Path, bytes: &[u8]) -> Result<(), VaultError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| VaultError::PortableIo {
        path: parent.to_path_buf(),
        source,
    })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("portable-vault.bin");
    let temporary_path = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut temporary = options
        .open(&temporary_path)
        .map_err(|source| VaultError::PortableIo {
            path: temporary_path.clone(),
            source,
        })?;
    let write_result = (|| -> io::Result<()> {
        temporary.write_all(bytes)?;
        temporary.sync_all()?;
        drop(temporary);
        #[cfg(windows)]
        if path.exists() {
            fs::remove_file(path)?;
        }
        fs::rename(&temporary_path, path)
    })();
    if let Err(source) = write_result {
        let _ = fs::remove_file(&temporary_path);
        return Err(VaultError::PortableIo {
            path: path.to_path_buf(),
            source,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{CredentialId, PlatformVault, PortableVault, SecretMaterial, VaultError};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn credential_ids_are_strict_and_non_secret() {
        assert!(CredentialId::new("session-01-password").is_ok());
        assert!(CredentialId::new("session/01").is_err());
        assert!(CredentialId::new("").is_err());
        assert!(CredentialId::new("password=secret").is_err());
    }

    #[test]
    fn secret_debug_is_redacted() {
        let secret = SecretMaterial::new("do-not-log-this");
        assert_eq!(format!("{secret:?}"), "SecretMaterial(<redacted>)");
    }

    #[test]
    fn default_vault_has_stable_service_namespace() {
        assert_eq!(
            format!("{:?}", PlatformVault::default()),
            "PlatformVault { service: \"com.othmane.mobarust\" }"
        );
    }

    #[test]
    fn portable_vault_encrypts_and_round_trips_without_system_credentials() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("portable-vault.bin");
        let passphrase = SecretMaterial::new("fixture-passphrase");
        let credential_id = CredentialId::new("fixture-password").unwrap();
        let secret_value = "fixture-only-secret";

        let mut vault = PortableVault::create(&path, &passphrase).unwrap();
        vault
            .put(&credential_id, SecretMaterial::new(secret_value))
            .unwrap();
        assert_eq!(vault.list_ids(), vec!["fixture-password"]);
        drop(vault);

        let bytes = fs::read(&path).unwrap();
        assert!(
            !bytes
                .windows(secret_value.len())
                .any(|window| window == secret_value.as_bytes())
        );
        assert!(
            !bytes
                .windows(passphrase.as_str().len())
                .any(|window| window == passphrase.as_str().as_bytes())
        );

        let vault = PortableVault::open(&path, &passphrase).unwrap();
        assert_eq!(vault.get(&credential_id).unwrap().as_str(), secret_value);
        assert!(matches!(
            PortableVault::open(&path, &SecretMaterial::new("wrong-passphrase")),
            Err(VaultError::PortableAuthenticationFailed)
        ));
    }

    #[test]
    fn portable_vault_rejects_tampering_and_preserves_delete_semantics() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("portable-vault.bin");
        let passphrase = SecretMaterial::new("fixture-passphrase");
        let credential_id = CredentialId::new("fixture-token").unwrap();
        let mut vault = PortableVault::create(&path, &passphrase).unwrap();
        vault
            .put(&credential_id, SecretMaterial::new("token-value"))
            .unwrap();
        assert!(vault.delete(&credential_id).unwrap());
        assert!(!vault.delete(&credential_id).unwrap());
        drop(vault);

        let mut bytes = fs::read(&path).unwrap();
        let last = bytes.last_mut().unwrap();
        *last ^= 0x01;
        fs::write(&path, bytes).unwrap();
        assert!(matches!(
            PortableVault::open(&path, &passphrase),
            Err(VaultError::PortableAuthenticationFailed)
        ));
    }
}
