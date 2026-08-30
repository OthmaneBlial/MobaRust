//! Versioned, secret-free persistence for saved session definitions.
//!
//! This crate deliberately stores only [`mobarust_core::SessionRecord`] data.
//! Credential references are safe identifiers; credential material belongs to
//! `mobarust-vault` and never enters this file.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use mobarust_core::{
    AppSettings, AuthMethod, MacroRecord, Protocol, SessionId, SessionRecord, SnippetRecord,
};
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
    #[error("snippet is invalid: {0}")]
    InvalidSnippet(#[from] mobarust_core::SnippetValidationError),
    #[error("snippet file {path} contains invalid data: {source}")]
    SnippetDecode {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("snippet file {path} uses unsupported schema version {version}")]
    SnippetUnsupportedSchema { path: PathBuf, version: u32 },
    #[error("could not serialize snippets: {0}")]
    SnippetEncode(serde_json::Error),
    #[error("could not write snippet file {path}: {source}")]
    SnippetWrite { path: PathBuf, source: io::Error },
    #[error("macro is invalid: {0}")]
    InvalidMacro(#[from] mobarust_core::MacroValidationError),
    #[error("macro file {path} contains invalid data: {source}")]
    MacroDecode {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("macro file {path} uses unsupported schema version {version}")]
    MacroUnsupportedSchema { path: PathBuf, version: u32 },
    #[error("could not serialize macros: {0}")]
    MacroEncode(serde_json::Error),
    #[error("could not write macro file {path}: {source}")]
    MacroWrite { path: PathBuf, source: io::Error },
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

impl MacroStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        let macros = if path.exists() {
            let bytes = fs::read(&path).map_err(|source| StoreError::MacroWrite {
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
        let mut temporary = OpenOptions::new()
            .create_new(true)
            .write(true)
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
            let bytes = fs::read(&path).map_err(|source| StoreError::SnippetWrite {
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
        let mut temporary = OpenOptions::new()
            .create_new(true)
            .write(true)
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
                last_used_at: None,
                known_hosts_path: None,
                pinned_fingerprint: None,
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
                remote_desktop_profile: None,
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
    use mobarust_core::{
        AuthMethod, JumpHostRecord, MacroAction, MacroKey, MacroRecord, Protocol,
        RemoteDesktopProfile, SessionRecord, SnippetRecord,
    };
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
            last_used_at: None,
            known_hosts_path: None,
            pinned_fingerprint: None,
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
            width: 1280,
            height: 800,
            color_depth: 32,
            audio_enabled: false,
        });
        store.save(session.clone()).unwrap();

        let json = fs::read_to_string(&path).unwrap();
        assert!(json.contains("remote_desktop_profile"));
        assert!(json.contains("vnc-password-ref"));
        assert!(!json.contains("super-secret"));
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
        }];
        store.save(session.clone()).unwrap();

        let reopened = SessionStore::open(&path).unwrap();
        assert_eq!(reopened.list(), &[session]);
        let json = fs::read_to_string(&path).unwrap();
        assert!(json.contains("bastion-password"));
        assert!(!json.contains("super-secret"));
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

    #[test]
    fn snippets_round_trip_and_delete_durably() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("snippets.json");
        let mut store = SnippetStore::open(&path).unwrap();
        let mut snippet = SnippetRecord::new("Docker logs", "docker logs ${container}");
        snippet.tags = vec!["docker".into(), "debug".into()];
        snippet.variables = vec!["container".into()];
        store.save(snippet.clone()).unwrap();
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
}
