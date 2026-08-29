//! Native credential storage kept behind a small Rust-only boundary.
//!
//! Session records should store [`CredentialId`] values, never password or key
//! material. The desktop frontend can ask native code to use a reference, but
//! it does not receive the secret as part of the normal session snapshot.

use std::fmt;

use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

const DEFAULT_SERVICE: &str = "com.othmane.mobarust";

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

/// Convert an owned secret to a zeroizing temporary for a native operation.
pub fn zeroizing_secret(value: String) -> Zeroizing<String> {
    Zeroizing::new(value)
}

#[cfg(test)]
mod tests {
    use super::{CredentialId, PlatformVault, SecretMaterial};

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
}
