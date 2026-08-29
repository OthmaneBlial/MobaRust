//! Versioned, secret-free persistence for saved session definitions.
//!
//! This crate deliberately stores only [`mobarust_core::SessionRecord`] data.
//! Credential references are safe identifiers; credential material belongs to
//! `mobarust-vault` and never enters this file.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use mobarust_core::{SessionId, SessionRecord};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

const CURRENT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("could not read session store {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("session store {path} contains invalid data: {source}")]
    Decode {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("session store {path} uses unsupported schema version {version}")]
    UnsupportedSchema { path: PathBuf, version: u32 },
    #[error("saved session is invalid: {0}")]
    InvalidSession(#[from] mobarust_core::SessionValidationError),
    #[error("could not serialize session store: {0}")]
    Encode(serde_json::Error),
    #[error("could not write session store {path}: {source}")]
    Write { path: PathBuf, source: io::Error },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreFile {
    schema_version: u32,
    sessions: Vec<SessionRecord>,
}

/// A serialized session catalog protected by an in-process mutex in the
/// desktop layer. The store itself remains synchronous so each mutation has a
/// clear durable completion point before its command returns.
#[derive(Debug)]
pub struct SessionStore {
    path: PathBuf,
    sessions: Vec<SessionRecord>,
}

impl SessionStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        let sessions = if path.exists() {
            let bytes = fs::read(&path).map_err(|source| StoreError::Read {
                path: path.clone(),
                source,
            })?;
            let file: StoreFile =
                serde_json::from_slice(&bytes).map_err(|source| StoreError::Decode {
                    path: path.clone(),
                    source,
                })?;
            if file.schema_version != CURRENT_SCHEMA_VERSION {
                return Err(StoreError::UnsupportedSchema {
                    path,
                    version: file.schema_version,
                });
            }
            for session in &file.sessions {
                session.validate()?;
            }
            file.sessions
        } else {
            Vec::new()
        };

        Ok(Self { path, sessions })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn list(&self) -> &[SessionRecord] {
        &self.sessions
    }

    pub fn save(&mut self, session: SessionRecord) -> Result<SessionRecord, StoreError> {
        session.validate()?;
        if let Some(existing) = self.sessions.iter_mut().find(|item| item.id == session.id) {
            *existing = session.clone();
        } else {
            self.sessions.push(session.clone());
        }
        self.persist()?;
        Ok(session)
    }

    pub fn delete(&mut self, id: SessionId) -> Result<bool, StoreError> {
        let original_len = self.sessions.len();
        self.sessions.retain(|session| session.id != id);
        let deleted = self.sessions.len() != original_len;
        if deleted {
            self.persist()?;
        }
        Ok(deleted)
    }

    fn persist(&self) -> Result<(), StoreError> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|source| StoreError::Write {
            path: parent.to_path_buf(),
            source,
        })?;

        let bytes = serde_json::to_vec_pretty(&StoreFile {
            schema_version: CURRENT_SCHEMA_VERSION,
            sessions: self.sessions.clone(),
        })
        .map_err(StoreError::Encode)?;

        let file_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("sessions.json");
        let temporary_path = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
        let mut temporary = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary_path)
            .map_err(|source| StoreError::Write {
                path: temporary_path.clone(),
                source,
            })?;

        let write_result = (|| -> io::Result<()> {
            temporary.write_all(&bytes)?;
            temporary.sync_all()?;
            drop(temporary);
            replace_file(&temporary_path, &self.path)
        })();

        if let Err(source) = write_result {
            let _ = fs::remove_file(&temporary_path);
            return Err(StoreError::Write {
                path: self.path.clone(),
                source,
            });
        }

        Ok(())
    }
}

fn replace_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(windows)]
    if destination.exists() {
        fs::remove_file(destination)?;
    }
    fs::rename(temporary, destination)
}

impl SessionStore {
    pub fn into_sessions(self) -> Vec<SessionRecord> {
        self.sessions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mobarust_core::{AuthMethod, Protocol, SessionRecord};
    use tempfile::tempdir;

    fn remote_session() -> SessionRecord {
        SessionRecord {
            id: SessionId::new(),
            name: "Production bastion".into(),
            protocol: Protocol::Ssh,
            hostname: "bastion.example.test".into(),
            port: 22,
            username: Some("ops".into()),
            auth: AuthMethod::Password {
                credential_ref: "session-password".into(),
            },
            known_hosts_path: None,
            pinned_fingerprint: None,
            folder: Some("Production".into()),
            tags: vec!["prod".into()],
            favorite: true,
            startup_directory: None,
            startup_command: None,
            environment: Vec::new(),
            jump_hosts: Vec::new(),
            notes: None,
        }
    }

    #[test]
    fn round_trip_persists_session_references_without_secret_material() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("sessions.json");
        let mut store = SessionStore::open(&path).unwrap();
        let session = remote_session();
        store.save(session.clone()).unwrap();

        let json = fs::read_to_string(&path).unwrap();
        assert!(json.contains("session-password"));
        assert!(!json.contains("super-secret"));

        let reopened = SessionStore::open(&path).unwrap();
        assert_eq!(reopened.list(), &[session]);
    }

    #[test]
    fn corrupt_or_unknown_store_is_not_silently_replaced() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("sessions.json");
        fs::write(
            &path,
            br#"{"schema_version":1,"sessions":[],"unknown":true}"#,
        )
        .unwrap();
        let error = SessionStore::open(&path).unwrap_err();
        assert!(matches!(error, StoreError::Decode { .. }));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            r#"{"schema_version":1,"sessions":[],"unknown":true}"#
        );
    }

    #[test]
    fn delete_is_idempotent_and_durable() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("sessions.json");
        let mut store = SessionStore::open(&path).unwrap();
        let session = remote_session();
        let id = session.id;
        store.save(session).unwrap();
        assert!(store.delete(id).unwrap());
        assert!(!store.delete(id).unwrap());
        assert!(SessionStore::open(&path).unwrap().list().is_empty());
    }

    #[test]
    fn schema_one_sessions_without_host_trust_fields_still_migrate_in_memory() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("sessions.json");
        let session = remote_session();
        let mut serialized = serde_json::to_value(&session).unwrap();
        let object = serialized.as_object_mut().unwrap();
        object.remove("known_hosts_path");
        object.remove("pinned_fingerprint");
        let file = serde_json::json!({ "schema_version": 1, "sessions": [serialized] });
        fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();

        let reopened = SessionStore::open(&path).unwrap();
        assert_eq!(reopened.list()[0].known_hosts_path, None);
        assert_eq!(reopened.list()[0].pinned_fingerprint, None);
    }

    #[test]
    fn credential_references_use_stable_frontend_safe_names() {
        let value = serde_json::to_value(AuthMethod::Password {
            credential_ref: "prod-password".into(),
        })
        .unwrap();
        assert_eq!(value["kind"], "password");
        assert_eq!(value["credentialRef"], "prod-password");
        assert!(value.get("credential_ref").is_none());
    }
}
