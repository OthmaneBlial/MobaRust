//! Versioned, secret-free persistence for saved session definitions.
//!
//! This crate deliberately stores only [`mobarust_core::SessionRecord`] data.
//! Credential references are safe identifiers; credential material belongs to
//! `mobarust-vault` and never enters this file.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use mobarust_core::{
    AppSettings, AuditEvent, AuditEventKind, AuthMethod, MAX_SERVER_ALIVE_INTERVAL_SECONDS,
    MacroRecord, Protocol, SessionId, SessionRecord, SnippetRecord,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

const CURRENT_SCHEMA_VERSION: u32 = 1;
const CURRENT_AUDIT_SCHEMA_VERSION: u32 = 1;
pub const MAX_AUDIT_EVENTS: usize = 1_000;
const MAX_OPENSSH_CONFIG_BYTES: usize = 1024 * 1024;
const MAX_LOCAL_STORE_FILE_BYTES: usize = 64 * 1024 * 1024;
const MAX_IMPORT_JSON_BYTES: usize = MAX_LOCAL_STORE_FILE_BYTES;

#[derive(Error)]
pub enum StoreError {
    #[error("could not read session store")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("session store contains invalid data")]
    Decode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("session import exceeds the 64 MiB limit")]
    SessionImportTooLarge,
    #[error("session store uses an unsupported schema version {version}")]
    UnsupportedSchema { path: PathBuf, version: u32 },
    #[error("saved session is invalid: {0}")]
    InvalidSession(#[from] mobarust_core::SessionValidationError),
    #[error("could not serialize session store")]
    Encode(#[source] serde_json::Error),
    #[error("could not write session store")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not read OpenSSH config")]
    ImportRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("OpenSSH config is too large (maximum 1 MiB)")]
    ImportTooLarge(PathBuf),
    #[error("OpenSSH config path is not a regular file")]
    ImportPathUnsafe(PathBuf),
    #[error("settings file contains invalid data")]
    SettingsDecode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("settings import exceeds the 64 MiB limit")]
    SettingsImportTooLarge,
    #[error("settings file uses an unsupported schema version {version}")]
    SettingsUnsupportedSchema { path: PathBuf, version: u32 },
    #[error("settings are invalid: {0}")]
    InvalidSettings(#[from] mobarust_core::SettingsValidationError),
    #[error("could not serialize settings")]
    SettingsEncode(#[source] serde_json::Error),
    #[error("could not write settings file")]
    SettingsWrite {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("snippet is invalid: {0}")]
    InvalidSnippet(#[from] mobarust_core::SnippetValidationError),
    #[error("snippet file contains invalid data")]
    SnippetDecode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("snippet file uses an unsupported schema version {version}")]
    SnippetUnsupportedSchema { path: PathBuf, version: u32 },
    #[error("could not serialize snippets")]
    SnippetEncode(#[source] serde_json::Error),
    #[error("could not write snippet file")]
    SnippetWrite {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("macro is invalid: {0}")]
    InvalidMacro(#[from] mobarust_core::MacroValidationError),
    #[error("macro file contains invalid data")]
    MacroDecode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("macro file uses an unsupported schema version {version}")]
    MacroUnsupportedSchema { path: PathBuf, version: u32 },
    #[error("could not serialize macros")]
    MacroEncode(#[source] serde_json::Error),
    #[error("could not write macro file")]
    MacroWrite {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not read audit file")]
    AuditRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("audit file contains invalid data")]
    AuditDecode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("audit file uses an unsupported schema version {version}")]
    AuditUnsupportedSchema { path: PathBuf, version: u32 },
    #[error("audit history cannot contain more than {MAX_AUDIT_EVENTS} events")]
    AuditTooLarge,
    #[error("could not serialize audit file")]
    AuditEncode(#[source] serde_json::Error),
    #[error("could not write audit file")]
    AuditWrite {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl fmt::Debug for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Keep `?error` logging as safe as the user-facing Display text. The
        // structured source chain remains available through Error::source for
        // an explicitly controlled internal diagnostic path.
        write!(formatter, "StoreError({self})")
    }
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

const CURRENT_SNIPPET_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnippetFile {
    schema_version: u32,
    snippets: Vec<SnippetRecord>,
}

const CURRENT_MACRO_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MacroFile {
    schema_version: u32,
    macros: Vec<MacroRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuditFile {
    schema_version: u32,
    events: Vec<AuditEvent>,
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

/// A versioned, secret-free command snippet catalog. Snippets are data only;
/// the desktop never executes a saved command without an explicit operator
/// action in the terminal.
#[derive(Debug)]
pub struct SnippetStore {
    path: PathBuf,
    snippets: Vec<SnippetRecord>,
}

/// A versioned, secret-free macro catalog. Macro actions are typed data and
/// never execute as part of loading or saving; the desktop must request an
/// explicit, visible run.
#[derive(Debug)]
pub struct MacroStore {
    path: PathBuf,
    macros: Vec<MacroRecord>,
}

/// A bounded, secret-free local audit history. This store is intentionally
/// separate from saved sessions and credential storage. It is not exported
/// with session definitions and never stores terminal input or file paths.
#[derive(Debug)]
pub struct AuditStore {
    path: PathBuf,
    events: Vec<AuditEvent>,
}

impl AuditStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        let events = if path.exists() {
            let bytes = read_local_store_file(&path).map_err(|source| StoreError::AuditRead {
                path: path.clone(),
                source,
            })?;
            let file: AuditFile =
                serde_json::from_slice(&bytes).map_err(|source| StoreError::AuditDecode {
                    path: path.clone(),
                    source,
                })?;
            if file.schema_version != CURRENT_AUDIT_SCHEMA_VERSION {
                return Err(StoreError::AuditUnsupportedSchema {
                    path,
                    version: file.schema_version,
                });
            }
            if file.events.len() > MAX_AUDIT_EVENTS {
                return Err(StoreError::AuditTooLarge);
            }
            file.events
        } else {
            Vec::new()
        };

        Ok(Self { path, events })
    }

    pub fn list(&self) -> &[AuditEvent] {
        &self.events
    }

    pub fn append(
        &mut self,
        kind: AuditEventKind,
        session_id: Option<SessionId>,
        protocol: Option<Protocol>,
    ) -> Result<AuditEvent, StoreError> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.append_at(kind, session_id, protocol, timestamp)
    }

    pub fn append_at(
        &mut self,
        kind: AuditEventKind,
        session_id: Option<SessionId>,
        protocol: Option<Protocol>,
        timestamp: u64,
    ) -> Result<AuditEvent, StoreError> {
        let event = AuditEvent::new(kind, session_id, protocol, timestamp);
        let previous = self.events.clone();
        self.events.push(event.clone());
        if self.events.len() > MAX_AUDIT_EVENTS {
            let overflow = self.events.len() - MAX_AUDIT_EVENTS;
            self.events.drain(..overflow);
        }
        if let Err(error) = self.persist() {
            self.events = previous;
            return Err(error);
        }
        Ok(event)
    }

    pub fn clear(&mut self) -> Result<(), StoreError> {
        let previous = self.events.clone();
        self.events.clear();
        if let Err(error) = self.persist() {
            self.events = previous;
            return Err(error);
        }
        Ok(())
    }

    fn persist(&self) -> Result<(), StoreError> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|source| StoreError::AuditWrite {
            path: parent.to_path_buf(),
            source,
        })?;
        let bytes = serde_json::to_vec_pretty(&AuditFile {
            schema_version: CURRENT_AUDIT_SCHEMA_VERSION,
            events: self.events.clone(),
        })
        .map_err(StoreError::AuditEncode)?;
        let file_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("audit.json");
        let temporary_path = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
        let mut temporary = private_temp_options()
            .open(&temporary_path)
            .map_err(|source| StoreError::AuditWrite {
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
            return Err(StoreError::AuditWrite {
                path: self.path.clone(),
                source,
            });
        }
        Ok(())
    }
}

impl MacroStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        let macros = if path.exists() {
            let bytes = read_local_store_file(&path).map_err(|source| StoreError::MacroWrite {
                path: path.clone(),
                source,
            })?;
            let file: MacroFile =
                serde_json::from_slice(&bytes).map_err(|source| StoreError::MacroDecode {
                    path: path.clone(),
                    source,
                })?;
            if file.schema_version != CURRENT_MACRO_SCHEMA_VERSION {
                return Err(StoreError::MacroUnsupportedSchema {
                    path,
                    version: file.schema_version,
                });
            }
            for record in &file.macros {
                record.validate()?;
            }
            file.macros
        } else {
            Vec::new()
        };
        Ok(Self { path, macros })
    }

    pub fn list(&self) -> &[MacroRecord] {
        &self.macros
    }

    pub fn save(&mut self, record: MacroRecord) -> Result<MacroRecord, StoreError> {
        record.validate()?;
        let previous = self.macros.clone();
        if let Some(existing) = self.macros.iter_mut().find(|item| item.id == record.id) {
            *existing = record.clone();
        } else {
            self.macros.push(record.clone());
        }
        if let Err(error) = self.persist() {
            self.macros = previous;
            return Err(error);
        }
        Ok(record)
    }

    pub fn delete(&mut self, id: Uuid) -> Result<bool, StoreError> {
        let previous = self.macros.clone();
        let original_len = self.macros.len();
        self.macros.retain(|record| record.id != id);
        if self.macros.len() == original_len {
            return Ok(false);
        }
        if let Err(error) = self.persist() {
            self.macros = previous;
            return Err(error);
        }
        Ok(true)
    }

    fn persist(&self) -> Result<(), StoreError> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|source| StoreError::MacroWrite {
            path: parent.to_path_buf(),
            source,
        })?;
        let bytes = serde_json::to_vec_pretty(&MacroFile {
            schema_version: CURRENT_MACRO_SCHEMA_VERSION,
            macros: self.macros.clone(),
        })
        .map_err(StoreError::MacroEncode)?;
        let file_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("macros.json");
        let temporary_path = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
        let mut temporary = private_temp_options()
            .open(&temporary_path)
            .map_err(|source| StoreError::MacroWrite {
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
            return Err(StoreError::MacroWrite {
                path: self.path.clone(),
                source,
            });
        }
        Ok(())
    }
}

impl SnippetStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        let snippets = if path.exists() {
            let bytes =
                read_local_store_file(&path).map_err(|source| StoreError::SnippetWrite {
                    path: path.clone(),
                    source,
                })?;
            let file: SnippetFile =
                serde_json::from_slice(&bytes).map_err(|source| StoreError::SnippetDecode {
                    path: path.clone(),
                    source,
                })?;
            if file.schema_version != CURRENT_SNIPPET_SCHEMA_VERSION {
                return Err(StoreError::SnippetUnsupportedSchema {
                    path,
                    version: file.schema_version,
                });
            }
            for snippet in &file.snippets {
                snippet.validate()?;
            }
            file.snippets
        } else {
            Vec::new()
        };
        Ok(Self { path, snippets })
    }

    pub fn list(&self) -> &[SnippetRecord] {
        &self.snippets
    }

    pub fn save(&mut self, snippet: SnippetRecord) -> Result<SnippetRecord, StoreError> {
        snippet.validate()?;
        let previous = self.snippets.clone();
        if let Some(existing) = self.snippets.iter_mut().find(|item| item.id == snippet.id) {
            *existing = snippet.clone();
        } else {
            self.snippets.push(snippet.clone());
        }
        if let Err(error) = self.persist() {
            self.snippets = previous;
            return Err(error);
        }
        Ok(snippet)
    }

    pub fn delete(&mut self, id: Uuid) -> Result<bool, StoreError> {
        let previous = self.snippets.clone();
        let original_len = self.snippets.len();
        self.snippets.retain(|snippet| snippet.id != id);
        if self.snippets.len() == original_len {
            return Ok(false);
        }
        if let Err(error) = self.persist() {
            self.snippets = previous;
            return Err(error);
        }
        Ok(true)
    }

    fn persist(&self) -> Result<(), StoreError> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|source| StoreError::SnippetWrite {
            path: parent.to_path_buf(),
            source,
        })?;
        let bytes = serde_json::to_vec_pretty(&SnippetFile {
            schema_version: CURRENT_SNIPPET_SCHEMA_VERSION,
            snippets: self.snippets.clone(),
        })
        .map_err(StoreError::SnippetEncode)?;
        let file_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("snippets.json");
        let temporary_path = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
        let mut temporary = private_temp_options()
            .open(&temporary_path)
            .map_err(|source| StoreError::SnippetWrite {
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
            return Err(StoreError::SnippetWrite {
                path: self.path.clone(),
                source,
            });
        }
        Ok(())
    }
}

impl SettingsStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        let settings = if path.exists() {
            let bytes =
                read_local_store_file(&path).map_err(|source| StoreError::SettingsWrite {
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
        let previous = self.settings.clone();
        self.settings = settings.clone();
        if let Err(error) = self.persist() {
            self.settings = previous;
            return Err(error);
        }
        Ok(settings)
    }

    pub fn reset(&mut self) -> Result<AppSettings, StoreError> {
        self.save(AppSettings::default())
    }

    /// Serializes only validated, non-secret preferences. Credentials and
    /// session definitions live in separate stores and cannot enter this
    /// export by construction.
    pub fn export_json(&self) -> Result<String, StoreError> {
        serde_json::to_string_pretty(&SettingsFile {
            schema_version: CURRENT_SETTINGS_SCHEMA_VERSION,
            settings: self.settings.clone(),
        })
        .map_err(StoreError::SettingsEncode)
    }

    /// Replaces preferences only after the complete payload has decoded,
    /// passed schema validation, and been durably persisted. A failed write
    /// restores the in-memory settings so callers never observe a half-import.
    pub fn import_json(&mut self, json: &str) -> Result<AppSettings, StoreError> {
        if json.len() > MAX_IMPORT_JSON_BYTES {
            return Err(StoreError::SettingsImportTooLarge);
        }
        let path = PathBuf::from("<settings-import>");
        let file: SettingsFile =
            serde_json::from_str(json).map_err(|error| StoreError::SettingsDecode {
                path: path.clone(),
                source: error,
            })?;
        if file.schema_version != CURRENT_SETTINGS_SCHEMA_VERSION {
            return Err(StoreError::SettingsUnsupportedSchema {
                path,
                version: file.schema_version,
            });
        }
        file.settings.validate()?;
        let previous = self.settings.clone();
        self.settings = file.settings.clone();
        if let Err(error) = self.persist() {
            self.settings = previous;
            return Err(error);
        }
        Ok(file.settings)
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
        let mut temporary = private_temp_options()
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
            let bytes = read_local_store_file(&path).map_err(|source| StoreError::Read {
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
        let previous = self.sessions.clone();
        if let Some(existing) = self.sessions.iter_mut().find(|item| item.id == session.id) {
            *existing = session.clone();
        } else {
            self.sessions.push(session.clone());
        }
        if let Err(error) = self.persist() {
            self.sessions = previous;
            return Err(error);
        }
        Ok(session)
    }

    pub fn delete(&mut self, id: SessionId) -> Result<bool, StoreError> {
        let previous = self.sessions.clone();
        let original_len = self.sessions.len();
        self.sessions.retain(|session| session.id != id);
        let deleted = self.sessions.len() != original_len;
        if deleted && let Err(error) = self.persist() {
            self.sessions = previous;
            return Err(error);
        }
        Ok(deleted)
    }

    pub fn set_favorite(&mut self, id: SessionId, favorite: bool) -> Result<bool, StoreError> {
        let previous = self.sessions.clone();
        let Some(session) = self.sessions.iter_mut().find(|session| session.id == id) else {
            return Ok(false);
        };
        session.favorite = favorite;
        if let Err(error) = self.persist() {
            self.sessions = previous;
            return Err(error);
        }
        Ok(true)
    }

    pub fn touch(&mut self, id: SessionId) -> Result<bool, StoreError> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.touch_at(id, timestamp)
    }

    pub fn touch_at(&mut self, id: SessionId, timestamp: u64) -> Result<bool, StoreError> {
        let previous = self.sessions.clone();
        let Some(session) = self.sessions.iter_mut().find(|session| session.id == id) else {
            return Ok(false);
        };
        session.last_used_at = Some(timestamp);
        if let Err(error) = self.persist() {
            self.sessions = previous;
            return Err(error);
        }
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
        if json.len() > MAX_IMPORT_JSON_BYTES {
            return Err(StoreError::SessionImportTooLarge);
        }
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
        let previous = self.sessions.clone();
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
        if changed && let Err(error) = self.persist() {
            self.sessions = previous;
            return Err(error);
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
        let contents = read_openssh_config(&path)?;
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
            let server_alive_interval = options
                .get("serveraliveinterval")
                .and_then(|interval| strip_quotes(interval.trim()).parse::<u64>().ok())
                .filter(|seconds| *seconds > 0 && *seconds <= MAX_SERVER_ALIVE_INTERVAL_SECONDS);
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
                last_used_at: None,
                known_hosts_path: None,
                pinned_fingerprint: None,
                x11_display: None,
                x11_single_connection: false,
                server_alive_interval,
                folder: Some("Imported / OpenSSH".into()),
                tags: vec!["imported".into(), "openssh".into()],
                favorite: false,
                startup_directory: None,
                startup_command: None,
                environment: Vec::new(),
                jump_hosts,
                jump_host_profiles: Vec::new(),
                notes,
                serial_profile: None,
                telnet_profile: None,
                remote_desktop_profile: None,
            });
        }

        for session in &imported {
            session.validate()?;
        }
        if !imported.is_empty() {
            let previous = self.sessions.clone();
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
            if let Err(error) = self.persist() {
                self.sessions = previous;
                return Err(error);
            }
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
        let mut temporary = private_temp_options()
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

fn read_openssh_config(path: &Path) -> Result<String, StoreError> {
    let file = open_store_file(path).map_err(|source| StoreError::ImportRead {
        path: path.to_path_buf(),
        source,
    })?;
    let metadata = file.metadata().map_err(|source| StoreError::ImportRead {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.file_type().is_file() {
        return Err(StoreError::ImportPathUnsafe(path.to_path_buf()));
    }
    if metadata.len() > MAX_OPENSSH_CONFIG_BYTES as u64 {
        return Err(StoreError::ImportTooLarge(path.to_path_buf()));
    }

    let mut contents = String::with_capacity(metadata.len() as usize);
    file.take(MAX_OPENSSH_CONFIG_BYTES as u64 + 1)
        .read_to_string(&mut contents)
        .map_err(|source| StoreError::ImportRead {
            path: path.to_path_buf(),
            source,
        })?;
    if contents.len() > MAX_OPENSSH_CONFIG_BYTES {
        return Err(StoreError::ImportTooLarge(path.to_path_buf()));
    }
    Ok(contents)
}

/// Read a persisted application store through a bounded, regular-file handle.
/// Store files contain no secrets, but a hostile local file must not be able to
/// force an allocation proportional to an arbitrary reported file size.
fn read_local_store_file(path: &Path) -> io::Result<Vec<u8>> {
    let file = open_store_file(path)?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "local store is not a regular file",
        ));
    }
    if metadata.len() > MAX_LOCAL_STORE_FILE_BYTES as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "local store exceeds the 64 MiB safety limit",
        ));
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_LOCAL_STORE_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_LOCAL_STORE_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "local store exceeds the 64 MiB safety limit",
        ));
    }
    Ok(bytes)
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
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };

        let temporary = temporary
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let destination = destination
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let flags = MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH;
        if unsafe { MoveFileExW(temporary.as_ptr(), destination.as_ptr(), flags) } == 0 {
            return Err(io::Error::last_os_error());
        }
        return Ok(());
    }

    fs::rename(temporary, destination)
}

#[cfg(unix)]
fn open_store_file(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    options.open(path)
}

#[cfg(not(unix))]
fn open_store_file(path: &Path) -> io::Result<fs::File> {
    fs::File::open(path)
}

fn private_temp_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
}

impl SessionStore {
    pub fn into_sessions(self) -> Vec<SessionRecord> {
        self.sessions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mobarust_core::{
        AuthMethod, JumpHostRecord, MacroAction, MacroKey, MacroRecord, Protocol,
        RdpGatewayProfile, RemoteDesktopProfile, SessionRecord, SnippetRecord, TelnetProfile,
    };
    use tempfile::tempdir;

    #[test]
    fn persistence_errors_redact_paths_and_os_details_from_display() {
        let private_path = PathBuf::from("/Users/example/.ssh/private-key");
        let invalid_json = || {
            serde_json::from_str::<serde_json::Value>("{invalid-json")
                .expect_err("fixture must be invalid")
        };
        let io_error = || io::Error::other(format!("raw OS detail at {private_path:?}"));
        let errors = vec![
            StoreError::Read {
                path: private_path.clone(),
                source: io_error(),
            },
            StoreError::Decode {
                path: private_path.clone(),
                source: invalid_json(),
            },
            StoreError::UnsupportedSchema {
                path: private_path.clone(),
                version: 99,
            },
            StoreError::Write {
                path: private_path.clone(),
                source: io_error(),
            },
            StoreError::ImportRead {
                path: private_path.clone(),
                source: io_error(),
            },
            StoreError::ImportTooLarge(private_path.clone()),
            StoreError::ImportPathUnsafe(private_path.clone()),
            StoreError::SettingsDecode {
                path: private_path.clone(),
                source: invalid_json(),
            },
            StoreError::SettingsUnsupportedSchema {
                path: private_path.clone(),
                version: 99,
            },
            StoreError::SettingsWrite {
                path: private_path.clone(),
                source: io_error(),
            },
            StoreError::SnippetDecode {
                path: private_path.clone(),
                source: invalid_json(),
            },
            StoreError::SnippetUnsupportedSchema {
                path: private_path.clone(),
                version: 99,
            },
            StoreError::SnippetWrite {
                path: private_path.clone(),
                source: io_error(),
            },
            StoreError::MacroDecode {
                path: private_path.clone(),
                source: invalid_json(),
            },
            StoreError::MacroUnsupportedSchema {
                path: private_path.clone(),
                version: 99,
            },
            StoreError::MacroWrite {
                path: private_path.clone(),
                source: io_error(),
            },
            StoreError::AuditRead {
                path: private_path.clone(),
                source: io_error(),
            },
            StoreError::AuditDecode {
                path: private_path.clone(),
                source: invalid_json(),
            },
            StoreError::AuditUnsupportedSchema {
                path: private_path.clone(),
                version: 99,
            },
            StoreError::AuditWrite {
                path: private_path.clone(),
                source: io_error(),
            },
        ];

        for error in errors {
            let display = error.to_string();
            let debug = format!("{error:?}");
            assert!(!display.contains(private_path.to_string_lossy().as_ref()));
            assert!(!display.contains("raw OS detail"));
            assert!(!display.contains("private-key"));
            assert!(!debug.contains(private_path.to_string_lossy().as_ref()));
            assert!(!debug.contains("raw OS detail"));
            assert!(!debug.contains("private-key"));
        }

        let error = StoreError::Read {
            path: private_path.clone(),
            source: io_error(),
        };
        assert!(std::error::Error::source(&error).is_some());
    }

    fn assert_private_file_permissions(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode,
                0o600,
                "{} is not owner-only: {mode:o}",
                path.display()
            );
        }
        #[cfg(not(unix))]
        let _ = path;
    }

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
            last_used_at: None,
            known_hosts_path: None,
            pinned_fingerprint: None,
            x11_display: None,
            x11_single_connection: false,
            server_alive_interval: None,
            folder: Some("Production".into()),
            tags: vec!["prod".into()],
            favorite: true,
            startup_directory: None,
            startup_command: None,
            environment: Vec::new(),
            jump_hosts: Vec::new(),
            jump_host_profiles: Vec::new(),
            notes: None,
            serial_profile: None,
            telnet_profile: None,
            remote_desktop_profile: None,
        }
    }

    #[test]
    fn round_trip_persists_session_references_without_secret_material() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("sessions.json");
        let mut store = SessionStore::open(&path).unwrap();
        let session = remote_session();
        store.save(session.clone()).unwrap();
        assert_private_file_permissions(&path);

        let json = fs::read_to_string(&path).unwrap();
        assert!(json.contains("session-password"));
        assert!(!json.contains("super-secret"));

        let reopened = SessionStore::open(&path).unwrap();
        assert_eq!(reopened.list(), &[session]);
    }

    #[test]
    fn remote_desktop_profiles_persist_metadata_and_opaque_credentials_only() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("sessions.json");
        let mut store = SessionStore::open(&path).unwrap();
        let mut session = remote_session();
        session.protocol = Protocol::Vnc;
        session.auth = AuthMethod::Password {
            credential_ref: "vnc-password-ref".into(),
        };
        session.remote_desktop_profile = Some(RemoteDesktopProfile {
            domain: None,
            gateway: None,
            width: 1280,
            height: 800,
            color_depth: 32,
            audio_enabled: false,
            clipboard_enabled: false,
            vnc_quality: "balanced".into(),
            allow_insecure_vnc: false,
            reconnect_enabled: true,
            reconnect_attempts: 3,
        });
        store.save(session.clone()).unwrap();

        let json = fs::read_to_string(&path).unwrap();
        assert!(json.contains("remote_desktop_profile"));
        assert!(json.contains("vnc-password-ref"));
        assert!(!json.contains("super-secret"));
        assert_eq!(SessionStore::open(&path).unwrap().list(), &[session]);
    }

    #[test]
    fn rdp_gateway_profiles_round_trip_without_plaintext_credentials() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("sessions.json");
        let mut store = SessionStore::open(&path).unwrap();
        let mut session = remote_session();
        session.protocol = Protocol::Rdp;
        session.auth = AuthMethod::Password {
            credential_ref: "rdp-session-password-ref".into(),
        };
        session.remote_desktop_profile = Some(RemoteDesktopProfile {
            domain: Some("LAB".into()),
            gateway: Some(RdpGatewayProfile {
                endpoint: "gateway.invalid:443".into(),
                username: "gateway-user".into(),
                credential_ref: "rdp-gateway-password-ref".into(),
            }),
            width: 1280,
            height: 800,
            color_depth: 32,
            audio_enabled: false,
            clipboard_enabled: false,
            vnc_quality: "balanced".into(),
            allow_insecure_vnc: false,
            reconnect_enabled: true,
            reconnect_attempts: 3,
        });
        store.save(session.clone()).unwrap();

        let json = fs::read_to_string(&path).unwrap();
        assert!(json.contains("gateway.invalid:443"));
        assert!(json.contains("rdp-session-password-ref"));
        assert!(json.contains("rdp-gateway-password-ref"));
        assert!(!json.contains("session-plaintext-secret"));
        assert!(!json.contains("gateway-plaintext-secret"));
        assert_eq!(SessionStore::open(&path).unwrap().list(), &[session]);
    }

    #[test]
    fn telnet_profiles_round_trip_without_authentication_material() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("sessions.json");
        let mut store = SessionStore::open(&path).unwrap();
        let mut session = remote_session();
        session.protocol = Protocol::Telnet;
        session.port = 23;
        session.username = None;
        session.auth = AuthMethod::None;
        session.folder = Some("Telnet sessions".into());
        session.telnet_profile = Some(TelnetProfile {
            terminal: "xterm-256color".into(),
            encoding: "utf-8".into(),
            columns: 120,
            rows: 32,
        });
        store.save(session.clone()).unwrap();

        let json = fs::read_to_string(&path).unwrap();
        assert!(json.contains("telnet_profile"));
        assert!(json.contains("xterm-256color"));
        assert!(!json.contains("password"));
        assert_eq!(SessionStore::open(&path).unwrap().list(), &[session]);
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
    fn session_store_rejects_an_oversized_file_before_parsing() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("sessions.json");
        let file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.set_len(super::MAX_LOCAL_STORE_FILE_BYTES as u64 + 1)
            .unwrap();

        let error = SessionStore::open(&path).unwrap_err();
        assert!(
            matches!(error, StoreError::Read { source, .. } if source.kind() == io::ErrorKind::InvalidData)
        );
    }

    #[test]
    fn jump_host_profiles_round_trip_as_credential_references() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("sessions.json");
        let mut store = SessionStore::open(&path).unwrap();
        let mut session = remote_session();
        session.jump_hosts = vec!["bastion".into()];
        session.jump_host_profiles = vec![JumpHostRecord {
            host: "bastion.example.test".into(),
            port: 22,
            username: "ops".into(),
            auth: AuthMethod::Password {
                credential_ref: "bastion-password".into(),
            },
            known_hosts_path: None,
            pinned_fingerprint: Some("SHA256:test".into()),
            server_alive_interval: Some(15),
        }];
        store.save(session.clone()).unwrap();

        let reopened = SessionStore::open(&path).unwrap();
        assert_eq!(reopened.list(), &[session]);
        let json = fs::read_to_string(&path).unwrap();
        assert!(json.contains("bastion-password"));
        assert!(!json.contains("super-secret"));
    }

    #[test]
    fn x11_display_profile_round_trip_remains_explicit_and_secret_free() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("sessions.json");
        let mut store = SessionStore::open(&path).unwrap();
        let mut session = remote_session();
        session.x11_display = Some("tcp://127.0.0.1:6000".into());
        session.x11_single_connection = true;
        store.save(session.clone()).unwrap();

        let reopened = SessionStore::open(&path).unwrap();
        assert_eq!(reopened.list(), &[session]);
        let json = fs::read_to_string(path).unwrap();
        assert!(json.contains("tcp://127.0.0.1:6000"));
        assert!(json.contains("session-password"));
        assert!(!json.contains("plaintext-secret"));
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
        object.remove("last_used_at");
        object.remove("known_hosts_path");
        object.remove("pinned_fingerprint");
        let file = serde_json::json!({ "schema_version": 1, "sessions": [serialized] });
        fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();

        let reopened = SessionStore::open(&path).unwrap();
        assert_eq!(reopened.list()[0].known_hosts_path, None);
        assert_eq!(reopened.list()[0].pinned_fingerprint, None);
        assert_eq!(reopened.list()[0].last_used_at, None);
    }

    #[test]
    fn legacy_remote_desktop_profiles_default_vnc_quality_safely() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("sessions.json");
        let mut session = remote_session();
        session.protocol = Protocol::Vnc;
        session.remote_desktop_profile = Some(RemoteDesktopProfile {
            domain: None,
            gateway: None,
            width: 1280,
            height: 800,
            color_depth: 32,
            audio_enabled: false,
            clipboard_enabled: false,
            vnc_quality: "balanced".into(),
            allow_insecure_vnc: false,
            reconnect_enabled: true,
            reconnect_attempts: 3,
        });
        let mut serialized = serde_json::to_value(&session).unwrap();
        serialized["remote_desktop_profile"]
            .as_object_mut()
            .unwrap()
            .remove("vnc_quality");
        let file = serde_json::json!({ "schema_version": 1, "sessions": [serialized] });
        fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();

        let reopened = SessionStore::open(&path).unwrap();
        assert_eq!(
            reopened.list()[0]
                .remote_desktop_profile
                .as_ref()
                .unwrap()
                .vnc_quality,
            "balanced"
        );
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
    fn session_mutations_roll_back_when_persistence_fails() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("sessions.json");
        let mut store = SessionStore::open(&path).unwrap();
        let original = remote_session();
        store.save(original.clone()).unwrap();

        let blocked_parent = directory.path().join("not-a-directory");
        fs::write(&blocked_parent, b"fixture blocker").unwrap();
        store.path = blocked_parent.join("sessions.json");

        let mut replacement = original.clone();
        replacement.name = "Changed but not durable".into();
        assert!(store.save(replacement).is_err());
        assert_eq!(store.list(), std::slice::from_ref(&original));

        assert!(store.set_favorite(original.id, false).is_err());
        assert!(store.list()[0].favorite);

        assert!(store.delete(original.id).is_err());
        assert_eq!(store.list(), std::slice::from_ref(&original));

        let mut imported = original.clone();
        imported.name = "Imported but not durable".into();
        let payload = serde_json::json!({
            "schema_version": 1,
            "sessions": [imported],
        });
        assert!(store.import_json(&payload.to_string()).is_err());
        assert_eq!(store.list(), std::slice::from_ref(&original));

        let config = directory.path().join("config");
        fs::write(&config, "Host fixture\n  HostName 127.0.0.1\n").unwrap();
        assert!(store.import_openssh_config(&config).is_err());
        assert_eq!(store.list(), &[original]);
    }

    #[test]
    fn recent_session_timestamps_are_durable_and_unknown_ids_are_safe() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("sessions.json");
        let mut store = SessionStore::open(&path).unwrap();
        let first = remote_session();
        let first_id = first.id;
        let second = remote_session();
        let second_id = second.id;
        store.save(first).unwrap();
        store.save(second).unwrap();

        assert!(store.touch_at(first_id, 10).unwrap());
        assert!(store.touch_at(second_id, 20).unwrap());
        assert!(!store.touch_at(SessionId::new(), 30).unwrap());

        let reopened = SessionStore::open(&path).unwrap();
        assert_eq!(reopened.list()[0].last_used_at, Some(10));
        assert_eq!(reopened.list()[1].last_used_at, Some(20));
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
    fn session_import_rejects_an_oversized_payload_before_parsing() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("sessions.json");
        let mut store = SessionStore::open(&path).unwrap();
        let oversized = "x".repeat(super::MAX_IMPORT_JSON_BYTES + 1);

        let error = store.import_json(&oversized).unwrap_err();
        assert!(matches!(error, StoreError::SessionImportTooLarge));
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
        assert_private_file_permissions(&path);
        assert_eq!(SettingsStore::open(&path).unwrap().get(), &settings);

        store.reset().unwrap();
        assert_eq!(
            SettingsStore::open(&path).unwrap().get(),
            &AppSettings::default()
        );
    }

    #[test]
    fn settings_save_rolls_back_when_persistence_fails() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let mut store = SettingsStore::open(&path).unwrap();
        let original = store.get().clone();

        let blocked_parent = directory.path().join("not-a-directory");
        fs::write(&blocked_parent, b"fixture blocker").unwrap();
        store.path = blocked_parent.join("settings.json");

        let mut changed = original.clone();
        changed.appearance.font_size = 18;
        assert!(store.save(changed).is_err());
        assert_eq!(store.get(), &original);
    }

    #[test]
    fn settings_without_keyboard_section_use_safe_defaults() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(
            &path,
            br#"{"schema_version":1,"settings":{"terminal":{"scrollbackLines":6000}}}"#,
        )
        .unwrap();

        let store = SettingsStore::open(&path).unwrap();
        assert_eq!(store.get().terminal.scrollback_lines, 6_000);
        assert_eq!(
            store.get().keyboard,
            mobarust_core::KeyboardSettings::default()
        );
        assert_eq!(store.get().keyboard.command_palette, "Mod+Shift+P");
    }

    #[test]
    fn settings_export_import_is_secret_free_and_rejects_invalid_payloads() {
        let directory = tempdir().unwrap();
        let source_path = directory.path().join("source-settings.json");
        let target_path = directory.path().join("target-settings.json");
        let mut source = SettingsStore::open(&source_path).unwrap();
        let mut settings = AppSettings::default();
        settings.appearance.font_size = 19;
        settings.network.scan_concurrency = 8;
        source.save(settings.clone()).unwrap();

        let json = source.export_json().unwrap();
        assert!(json.contains("scanConcurrency"));
        assert!(!json.contains("password"));
        assert!(!json.contains("credential"));

        let mut target = SettingsStore::open(&target_path).unwrap();
        assert_eq!(target.import_json(&json).unwrap(), settings);
        assert_eq!(SettingsStore::open(&target_path).unwrap().get(), &settings);

        let error = target.import_json(r#"{"schema_version":1,"settings":{},"secret":"nope"}"#);
        assert!(matches!(error, Err(StoreError::SettingsDecode { .. })));
        assert_eq!(target.get(), &settings);
    }

    #[test]
    fn settings_import_rejects_an_oversized_payload_before_parsing() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let mut store = SettingsStore::open(&path).unwrap();
        let oversized = "x".repeat(super::MAX_IMPORT_JSON_BYTES + 1);

        let error = store.import_json(&oversized).unwrap_err();
        assert!(matches!(error, StoreError::SettingsImportTooLarge));
        assert_eq!(store.get(), &AppSettings::default());
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
        assert_eq!(report.imported[0].server_alive_interval, Some(30));
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
    fn openssh_import_rejects_an_oversized_config_before_parsing() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("config");
        let file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.set_len(super::MAX_OPENSSH_CONFIG_BYTES as u64 + 1)
            .unwrap();

        let mut store = SessionStore::open(directory.path().join("sessions.json")).unwrap();
        assert!(matches!(
            store.import_openssh_config(&path),
            Err(StoreError::ImportTooLarge(rejected)) if rejected == path
        ));
        assert!(store.list().is_empty());
    }

    #[test]
    fn imports_keepalive_zero_or_out_of_range_values_without_applying_them() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("config");
        fs::write(
            &path,
            format!(
                "Host disabled\n    ServerAliveInterval 0\nHost too-long\n    ServerAliveInterval {}\n",
                MAX_SERVER_ALIVE_INTERVAL_SECONDS + 1
            ),
        )
        .unwrap();

        let mut store = SessionStore::open(directory.path().join("sessions.json")).unwrap();
        let report = store.import_openssh_config(&path).unwrap();

        assert_eq!(report.imported.len(), 2);
        assert_eq!(report.imported[0].server_alive_interval, None);
        assert_eq!(report.imported[1].server_alive_interval, None);
        assert_eq!(
            report.imported[0].notes.as_deref(),
            Some("Imported from OpenSSH; ServerAliveInterval=0")
        );
        let expected_note = format!(
            "Imported from OpenSSH; ServerAliveInterval={}",
            MAX_SERVER_ALIVE_INTERVAL_SECONDS + 1
        );
        assert_eq!(
            report.imported[1].notes.as_deref(),
            Some(expected_note.as_str())
        );
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

    #[test]
    fn snippets_round_trip_and_delete_durably() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("snippets.json");
        let mut store = SnippetStore::open(&path).unwrap();
        let mut snippet = SnippetRecord::new("Docker logs", "docker logs ${container}");
        snippet.tags = vec!["docker".into(), "debug".into()];
        snippet.variables = vec!["container".into()];
        store.save(snippet.clone()).unwrap();
        assert_private_file_permissions(&path);
        assert_eq!(
            SnippetStore::open(&path).unwrap().list(),
            &[snippet.clone()]
        );
        assert!(store.delete(snippet.id).unwrap());
        assert!(SnippetStore::open(&path).unwrap().list().is_empty());
    }

    #[test]
    fn corrupt_snippets_are_not_silently_replaced() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("snippets.json");
        fs::write(
            &path,
            br#"{"schema_version":1,"snippets":[],"unknown":true}"#,
        )
        .unwrap();
        let error = SnippetStore::open(&path).unwrap_err();
        assert!(matches!(error, StoreError::SnippetDecode { .. }));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            r#"{"schema_version":1,"snippets":[],"unknown":true}"#
        );
    }

    #[test]
    fn macros_round_trip_and_delete_durably() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("macros.json");
        let mut store = MacroStore::open(&path).unwrap();
        let mut record = MacroRecord::new("Restart service");
        record.tags = vec!["ops".into()];
        record.actions = vec![
            MacroAction::ExecuteCommand {
                command: "sudo systemctl restart app".into(),
            },
            MacroAction::SendKey {
                key: MacroKey::Enter,
            },
            MacroAction::Wait { milliseconds: 250 },
        ];
        store.save(record.clone()).unwrap();
        assert_private_file_permissions(&path);
        assert_eq!(MacroStore::open(&path).unwrap().list(), &[record.clone()]);
        assert!(store.delete(record.id).unwrap());
        assert!(MacroStore::open(&path).unwrap().list().is_empty());
    }

    #[test]
    fn corrupt_macros_are_not_silently_replaced() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("macros.json");
        fs::write(&path, br#"{"schema_version":1,"macros":[],"unknown":true}"#).unwrap();
        let error = MacroStore::open(&path).unwrap_err();
        assert!(matches!(error, StoreError::MacroDecode { .. }));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            r#"{"schema_version":1,"macros":[],"unknown":true}"#
        );
    }

    #[test]
    fn audit_history_is_bounded_secret_free_and_durable() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("audit.json");
        let session_id = SessionId::new();
        let mut store = AuditStore::open(&path).unwrap();

        for timestamp in 0..=MAX_AUDIT_EVENTS as u64 {
            store
                .append_at(
                    AuditEventKind::ConnectionSucceeded,
                    Some(session_id),
                    Some(Protocol::Ssh),
                    timestamp,
                )
                .unwrap();
        }
        assert_private_file_permissions(&path);

        assert_eq!(store.list().len(), MAX_AUDIT_EVENTS);
        assert_eq!(store.list().first().unwrap().timestamp, 1);
        assert_eq!(
            store.list().last().unwrap().timestamp,
            MAX_AUDIT_EVENTS as u64
        );

        let json = fs::read_to_string(&path).unwrap();
        assert!(!json.contains("password"));
        assert!(!json.contains("private_key"));
        assert_eq!(
            AuditStore::open(&path).unwrap().list().len(),
            MAX_AUDIT_EVENTS
        );
    }

    #[test]
    fn audit_clear_is_durable_and_corrupt_data_is_not_replaced() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("audit.json");
        let mut store = AuditStore::open(&path).unwrap();
        store
            .append_at(
                AuditEventKind::SessionOpened,
                None,
                Some(Protocol::Local),
                42,
            )
            .unwrap();
        store.clear().unwrap();
        assert!(AuditStore::open(&path).unwrap().list().is_empty());

        fs::write(&path, br#"{"schema_version":1,"events":[],"unknown":true}"#).unwrap();
        let error = AuditStore::open(&path).unwrap_err();
        assert!(matches!(error, StoreError::AuditDecode { .. }));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            r#"{"schema_version":1,"events":[],"unknown":true}"#
        );
    }

    #[test]
    fn nested_unknown_fields_are_rejected_in_each_persisted_store() {
        let directory = tempdir().unwrap();

        let session_path = directory.path().join("nested-sessions.json");
        let mut session = serde_json::to_value(remote_session()).unwrap();
        session["auth"]["unknown"] = serde_json::json!(true);
        fs::write(
            &session_path,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "sessions": [session]
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(matches!(
            SessionStore::open(&session_path),
            Err(StoreError::Decode { .. })
        ));

        let settings_path = directory.path().join("nested-settings.json");
        fs::write(
            &settings_path,
            br#"{"schema_version":1,"settings":{"terminal":{"scrollbackLines":5000,"unknown":true}}}"#,
        )
        .unwrap();
        assert!(matches!(
            SettingsStore::open(&settings_path),
            Err(StoreError::SettingsDecode { .. })
        ));

        let snippet_path = directory.path().join("nested-snippets.json");
        fs::write(
            &snippet_path,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "snippets": [{
                    "id": uuid::Uuid::nil(),
                    "title": "Fixture",
                    "description": "",
                    "command": "printf ready",
                    "tags": [],
                    "variables": [],
                    "unknown": true
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(matches!(
            SnippetStore::open(&snippet_path),
            Err(StoreError::SnippetDecode { .. })
        ));

        let macro_path = directory.path().join("nested-macros.json");
        fs::write(
            &macro_path,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "macros": [{
                    "id": uuid::Uuid::nil(),
                    "title": "Fixture",
                    "description": "",
                    "tags": [],
                    "actions": [{"kind": "sendKey", "key": "enter", "unknown": true}]
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(matches!(
            MacroStore::open(&macro_path),
            Err(StoreError::MacroDecode { .. })
        ));

        let audit_path = directory.path().join("nested-audit.json");
        fs::write(
            &audit_path,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "events": [{
                    "id": uuid::Uuid::nil(),
                    "timestamp": 42,
                    "kind": "sessionOpened",
                    "sessionId": null,
                    "protocol": "LOCAL",
                    "unknown": true
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(matches!(
            AuditStore::open(&audit_path),
            Err(StoreError::AuditDecode { .. })
        ));
    }
}
