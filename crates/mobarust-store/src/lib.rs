//! Versioned, secret-free persistence for saved session definitions.
//!
//! This crate deliberately stores only [`mobarust_core::SessionRecord`] data.
//! Credential references are safe identifiers; credential material belongs to
//! `mobarust-vault` and never enters this file.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use mobarust_core::{AppSettings, AuthMethod, Protocol, SessionId, SessionRecord};
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
    #[error("could not read OpenSSH config {path}: {source}")]
    ImportRead { path: PathBuf, source: io::Error },
    #[error("settings file {path} contains invalid data: {source}")]
    SettingsDecode {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("settings file {path} uses unsupported schema version {version}")]
    SettingsUnsupportedSchema { path: PathBuf, version: u32 },
    #[error("settings are invalid: {0}")]
    InvalidSettings(#[from] mobarust_core::SettingsValidationError),
    #[error("could not serialize settings: {0}")]
    SettingsEncode(serde_json::Error),
    #[error("could not write settings file {path}: {source}")]
    SettingsWrite { path: PathBuf, source: io::Error },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreFile {
    schema_version: u32,
    sessions: Vec<SessionRecord>,
}

const CURRENT_SETTINGS_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsFile {
    schema_version: u32,
    #[serde(default)]
    settings: AppSettings,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenSshImportReport {
    pub source: String,
    pub imported: Vec<SessionRecord>,
    pub skipped_hosts: Vec<String>,
    pub unsupported_directives: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionImportReport {
    pub imported_count: usize,
    pub skipped: Vec<String>,
}

#[derive(Debug, Default)]
struct OpenSshHostBlock {
    patterns: Vec<String>,
    options: Vec<(String, String)>,
}

/// A serialized session catalog protected by an in-process mutex in the
/// desktop layer. The store itself remains synchronous so each mutation has a
/// clear durable completion point before its command returns.
#[derive(Debug)]
pub struct SessionStore {
    path: PathBuf,
    sessions: Vec<SessionRecord>,
}

/// Versioned, non-secret application preferences stored separately from the
/// session catalog. This separation makes it impossible for a settings export
/// to accidentally become a credential export.
#[derive(Debug)]
pub struct SettingsStore {
    path: PathBuf,
    settings: AppSettings,
}

impl SettingsStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        let settings = if path.exists() {
            let bytes = fs::read(&path).map_err(|source| StoreError::SettingsWrite {
                path: path.clone(),
                source,
            })?;
            let file: SettingsFile =
                serde_json::from_slice(&bytes).map_err(|source| StoreError::SettingsDecode {
                    path: path.clone(),
                    source,
                })?;
            if file.schema_version != CURRENT_SETTINGS_SCHEMA_VERSION {
                return Err(StoreError::SettingsUnsupportedSchema {
                    path,
                    version: file.schema_version,
                });
            }
            file.settings.validate()?;
            file.settings
        } else {
            AppSettings::default()
        };

        Ok(Self { path, settings })
    }

    pub fn get(&self) -> &AppSettings {
        &self.settings
    }

    pub fn save(&mut self, settings: AppSettings) -> Result<AppSettings, StoreError> {
        settings.validate()?;
        self.settings = settings.clone();
        self.persist()?;
        Ok(settings)
    }

    pub fn reset(&mut self) -> Result<AppSettings, StoreError> {
        self.save(AppSettings::default())
    }

    fn persist(&self) -> Result<(), StoreError> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|source| StoreError::SettingsWrite {
            path: parent.to_path_buf(),
            source,
        })?;
        let bytes = serde_json::to_vec_pretty(&SettingsFile {
            schema_version: CURRENT_SETTINGS_SCHEMA_VERSION,
            settings: self.settings.clone(),
        })
        .map_err(StoreError::SettingsEncode)?;
        let file_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("settings.json");
        let temporary_path = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
        let mut temporary = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary_path)
            .map_err(|source| StoreError::SettingsWrite {
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
            return Err(StoreError::SettingsWrite {
                path: self.path.clone(),
                source,
            });
        }
        Ok(())
    }
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

    pub fn set_favorite(&mut self, id: SessionId, favorite: bool) -> Result<bool, StoreError> {
        let Some(session) = self.sessions.iter_mut().find(|session| session.id == id) else {
            return Ok(false);
        };
        session.favorite = favorite;
        self.persist()?;
        Ok(true)
    }

    /// Serializes only the versioned session catalog. The format contains
    /// credential references, never credential material.
    pub fn export_json(&self) -> Result<String, StoreError> {
        serde_json::to_string_pretty(&StoreFile {
            schema_version: CURRENT_SCHEMA_VERSION,
            sessions: self.sessions.clone(),
        })
        .map_err(StoreError::Encode)
    }

    /// Merges a previously exported secret-free catalog. A session ID is the
    /// stable merge key; malformed or unknown-schema input fails before the
    /// current store is changed.
    pub fn import_json(&mut self, json: &str) -> Result<SessionImportReport, StoreError> {
        let source = PathBuf::from("<session-import>");
        let file: StoreFile =
            serde_json::from_str(json).map_err(|decode_error| StoreError::Decode {
                path: source.clone(),
                source: decode_error,
            })?;
        if file.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(StoreError::UnsupportedSchema {
                path: source,
                version: file.schema_version,
            });
        }
        let mut imported_count = 0;
        let mut skipped = Vec::new();
        let mut changed = false;
        for session in file.sessions {
            if let Err(error) = session.validate() {
                skipped.push(format!("{}: {error}", session.name));
                continue;
            }
            imported_count += 1;
            if let Some(existing) = self.sessions.iter_mut().find(|item| item.id == session.id) {
                *existing = session;
            } else {
                self.sessions.push(session);
            }
            changed = true;
        }
        if changed {
            self.persist()?;
        }
        Ok(SessionImportReport {
            imported_count,
            skipped,
        })
    }

    /// Imports the commonly used, secret-free connection fields from an
    /// OpenSSH config. Unsupported directives are returned in the report so a
    /// user can see the boundary instead of receiving a misleadingly partial
    /// profile.
    pub fn import_openssh_config(
        &mut self,
        path: impl Into<PathBuf>,
    ) -> Result<OpenSshImportReport, StoreError> {
        let path = path.into();
        let contents = fs::read_to_string(&path).map_err(|source| StoreError::ImportRead {
            path: path.clone(),
            source,
        })?;
        let (blocks, unsupported_directives) = parse_openssh_config(&contents);
        let mut imported = Vec::new();
        let mut skipped_hosts = Vec::new();
        let mut seen = HashSet::new();

        for alias in blocks
            .iter()
            .flat_map(|block| block.patterns.iter())
            .filter(|pattern| is_exact_host_pattern(pattern))
        {
            if !seen.insert(alias.clone()) {
                continue;
            }
            let options = effective_options(&blocks, alias);
            let hostname = options
                .get("hostname")
                .cloned()
                .filter(|value| !value.trim().is_empty() && value != "%h")
                .unwrap_or_else(|| alias.clone());
            let port = match options.get("port") {
                None => 22,
                Some(value) => match value.parse::<u16>() {
                    Ok(port) if port > 0 => port,
                    _ => {
                        skipped_hosts.push(format!("{alias} (invalid Port)"));
                        continue;
                    }
                },
            };
            let identity = options
                .get("identityfile")
                .map(|value| strip_quotes(value.trim()).to_owned())
                .filter(|value| !value.is_empty());
            let username = options
                .get("user")
                .map(|value| strip_quotes(value.trim()).to_owned())
                .filter(|value| !value.is_empty());
            let jump_hosts = options
                .get("proxyjump")
                .into_iter()
                .flat_map(|value| value.split(','))
                .map(str::trim)
                .filter(|value| !value.is_empty() && *value != "none")
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let notes = options.get("serveraliveinterval").map(|interval| {
                format!(
                    "Imported from OpenSSH; ServerAliveInterval={}",
                    strip_quotes(interval.trim())
                )
            });
            imported.push(SessionRecord {
                id: SessionId::new(),
                name: alias.clone(),
                protocol: Protocol::Ssh,
                hostname,
                port,
                username,
                auth: identity.map_or(AuthMethod::Agent, |key_ref| AuthMethod::PrivateKey {
                    key_ref,
                    credential_ref: None,
                }),
                known_hosts_path: None,
                pinned_fingerprint: None,
                folder: Some("Imported / OpenSSH".into()),
                tags: vec!["imported".into(), "openssh".into()],
                favorite: false,
                startup_directory: None,
                startup_command: None,
                environment: Vec::new(),
                jump_hosts,
                notes,
            });
        }

        for session in &imported {
            session.validate()?;
        }
        if !imported.is_empty() {
            for imported_session in &mut imported {
                if let Some(existing) = self.sessions.iter_mut().find(|existing| {
                    existing.protocol == imported_session.protocol
                        && existing.name == imported_session.name
                }) {
                    imported_session.id = existing.id;
                    *existing = imported_session.clone();
                } else {
                    self.sessions.push(imported_session.clone());
                }
            }
            self.persist()?;
        }
        Ok(OpenSshImportReport {
            source: path.to_string_lossy().into_owned(),
            imported,
            skipped_hosts,
            unsupported_directives,
        })
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

fn parse_openssh_config(contents: &str) -> (Vec<OpenSshHostBlock>, Vec<String>) {
    let mut blocks = Vec::new();
    let mut current = OpenSshHostBlock {
        patterns: vec!["*".into()],
        options: Vec::new(),
    };
    let mut unsupported = Vec::new();
    let mut seen_unsupported = HashSet::new();

    for raw_line in contents.lines() {
        let line = raw_line
            .split_once('#')
            .map_or(raw_line, |(line, _)| line)
            .trim();
        if line.is_empty() {
            continue;
        }
        let Some((directive, value)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let directive = directive.to_ascii_lowercase();
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if directive == "host" {
            blocks.push(current);
            current = OpenSshHostBlock {
                patterns: value.split_whitespace().map(str::to_owned).collect(),
                options: Vec::new(),
            };
        } else if matches!(
            directive.as_str(),
            "hostname" | "user" | "port" | "identityfile" | "proxyjump" | "serveraliveinterval"
        ) {
            current.options.push((directive, value.to_owned()));
        } else if seen_unsupported.insert(directive.clone()) {
            unsupported.push(directive);
        }
    }
    blocks.push(current);
    (blocks, unsupported)
}

fn effective_options(blocks: &[OpenSshHostBlock], alias: &str) -> BTreeMap<String, String> {
    let mut options = BTreeMap::new();
    for block in blocks {
        if block
            .patterns
            .iter()
            .any(|pattern| pattern == "*" || pattern == alias)
        {
            for (key, value) in &block.options {
                options.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
    }
    options
}

fn is_exact_host_pattern(pattern: &str) -> bool {
    !pattern.is_empty()
        && pattern != "*"
        && !pattern.starts_with('!')
        && !pattern.contains(['*', '?'])
}

fn strip_quotes(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value)
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

    #[test]
    fn favorites_are_durable_and_unknown_ids_are_safe() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("sessions.json");
        let mut store = SessionStore::open(&path).unwrap();
        let session = remote_session();
        let id = session.id;
        store.save(session).unwrap();

        assert!(store.set_favorite(id, true).unwrap());
        assert!(!store.set_favorite(SessionId::new(), true).unwrap());
        let reopened = SessionStore::open(&path).unwrap();
        assert!(reopened.list()[0].favorite);
    }

    #[test]
    fn export_and_import_merge_secret_free_profiles() {
        let source_directory = tempdir().unwrap();
        let source_path = source_directory.path().join("sessions.json");
        let mut source = SessionStore::open(&source_path).unwrap();
        source.save(remote_session()).unwrap();
        let exported = source.export_json().unwrap();
        assert!(exported.contains("credentialRef"));
        assert!(!exported.contains("super-secret"));

        let target_directory = tempdir().unwrap();
        let target_path = target_directory.path().join("sessions.json");
        let mut target = SessionStore::open(&target_path).unwrap();
        let report = target.import_json(&exported).unwrap();
        assert_eq!(report.imported_count, 1);
        assert!(report.skipped.is_empty());
        assert_eq!(target.list().len(), 1);

        let second = target.import_json(&exported).unwrap();
        assert_eq!(second.imported_count, 1);
        assert_eq!(target.list().len(), 1);
    }

    #[test]
    fn invalid_export_is_rejected_without_mutating_store() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("sessions.json");
        let mut store = SessionStore::open(&path).unwrap();
        let error = store.import_json(r#"{"schema_version":1,"sessions":[],"extra":true}"#);
        assert!(matches!(error, Err(StoreError::Decode { .. })));
        assert!(store.list().is_empty());
    }

    #[test]
    fn settings_round_trip_and_reset_are_durable() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let mut store = SettingsStore::open(&path).unwrap();
        let mut settings = AppSettings::default();
        settings.appearance.font_size = 18;
        settings.general.confirm_multiline_paste = false;
        store.save(settings.clone()).unwrap();
        assert_eq!(SettingsStore::open(&path).unwrap().get(), &settings);

        store.reset().unwrap();
        assert_eq!(
            SettingsStore::open(&path).unwrap().get(),
            &AppSettings::default()
        );
    }

    #[test]
    fn corrupt_settings_are_not_silently_replaced() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(
            &path,
            br#"{"schema_version":1,"settings":{},"unknown":true}"#,
        )
        .unwrap();
        let error = SettingsStore::open(&path).unwrap_err();
        assert!(matches!(error, StoreError::SettingsDecode { .. }));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            r#"{"schema_version":1,"settings":{},"unknown":true}"#
        );
    }

    #[test]
    fn imports_supported_openssh_fields_without_private_material() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("config");
        fs::write(
            &path,
            r#"
                Host *
                    ServerAliveInterval 30
                Host prod bastion-alias
                    HostName prod.internal.example
                    User deploy
                    Port 2201
                    IdentityFile "~/.ssh/id_ed25519"
                    ProxyJump jump.example
                    Include ~/.ssh/conf.d/*
                Host staging
                    HostName staging.example
                    User ops
            "#,
        )
        .unwrap();

        let mut store = SessionStore::open(directory.path().join("sessions.json")).unwrap();
        let report = store.import_openssh_config(&path).unwrap();

        assert_eq!(report.imported.len(), 3);
        assert_eq!(report.imported[0].name, "prod");
        assert_eq!(report.imported[1].name, "bastion-alias");
        assert_eq!(report.imported[0].hostname, "prod.internal.example");
        assert_eq!(report.imported[0].port, 2201);
        assert_eq!(report.imported[0].username.as_deref(), Some("deploy"));
        assert_eq!(
            report.imported[0].auth,
            AuthMethod::PrivateKey {
                key_ref: "~/.ssh/id_ed25519".into(),
                credential_ref: None,
            }
        );
        assert_eq!(report.imported[0].jump_hosts, vec!["jump.example"]);
        assert_eq!(
            report.imported[0].notes.as_deref(),
            Some("Imported from OpenSSH; ServerAliveInterval=30")
        );
        assert!(report.unsupported_directives.contains(&"include".into()));
        assert!(!serde_json::to_string(&report).unwrap().contains("password"));
        assert_eq!(store.list().len(), 3);

        let second_report = store.import_openssh_config(&path).unwrap();
        assert_eq!(second_report.imported.len(), 3);
        assert_eq!(store.list().len(), 3);
    }

    #[test]
    fn does_not_create_profiles_for_wildcard_or_invalid_hosts() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("config");
        fs::write(
            &path,
            "Host *\n  User ops\nHost prod-*\n  HostName prod.example\nHost broken\n  Port nope\n",
        )
        .unwrap();

        let mut store = SessionStore::open(directory.path().join("sessions.json")).unwrap();
        let report = store.import_openssh_config(&path).unwrap();

        assert!(report.imported.is_empty());
        assert_eq!(report.skipped_hosts, vec!["broken (invalid Port)"]);
        assert!(store.list().is_empty());
    }
}
