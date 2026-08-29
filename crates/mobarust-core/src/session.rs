use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Protocol {
    Ssh,
    Sftp,
    Scp,
    Rdp,
    Vnc,
    Telnet,
    Serial,
    Local,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AuthMethod {
    None,
    Password {
        #[serde(rename = "credentialRef", alias = "credential_ref")]
        credential_ref: String,
    },
    PrivateKey {
        #[serde(rename = "keyRef", alias = "key_ref")]
        key_ref: String,
        #[serde(rename = "credentialRef", alias = "credential_ref")]
        credential_ref: Option<String>,
    },
    Agent,
    KeyboardInteractive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(Uuid);

impl SessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: SessionId,
    pub name: String,
    pub protocol: Protocol,
    pub hostname: String,
    pub port: u16,
    pub username: Option<String>,
    pub auth: AuthMethod,
    #[serde(default)]
    pub known_hosts_path: Option<String>,
    #[serde(default)]
    pub pinned_fingerprint: Option<String>,
    pub folder: Option<String>,
    pub tags: Vec<String>,
    pub favorite: bool,
    pub startup_directory: Option<String>,
    pub startup_command: Option<String>,
    pub environment: Vec<(String, String)>,
    pub jump_hosts: Vec<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SessionValidationError {
    #[error("session name cannot be empty")]
    EmptyName,
    #[error("hostname cannot be empty")]
    EmptyHostname,
    #[error("a network session must use a non-zero port")]
    MissingPort,
    #[error("session tags cannot be empty")]
    EmptyTag,
}

impl SessionRecord {
    pub fn local_terminal(name: impl Into<String>) -> Self {
        Self {
            id: SessionId::new(),
            name: name.into(),
            protocol: Protocol::Local,
            hostname: "localhost".into(),
            port: 0,
            username: None,
            auth: AuthMethod::None,
            known_hosts_path: None,
            pinned_fingerprint: None,
            folder: Some("Local terminals".into()),
            tags: vec!["local".into()],
            favorite: true,
            startup_directory: None,
            startup_command: None,
            environment: Vec::new(),
            jump_hosts: Vec::new(),
            notes: None,
        }
    }

    pub fn validate(&self) -> Result<(), SessionValidationError> {
        if self.name.trim().is_empty() {
            return Err(SessionValidationError::EmptyName);
        }
        if self.hostname.trim().is_empty() {
            return Err(SessionValidationError::EmptyHostname);
        }
        if self.protocol != Protocol::Local && self.port == 0 {
            return Err(SessionValidationError::MissingPort);
        }
        if self.tags.iter().any(|tag| tag.trim().is_empty()) {
            return Err(SessionValidationError::EmptyTag);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_terminal_has_no_secret_bearing_fields() {
        let session = SessionRecord::local_terminal("Mac shell");
        assert_eq!(session.protocol, Protocol::Local);
        assert_eq!(session.auth, AuthMethod::None);
        session.validate().unwrap();
    }

    #[test]
    fn remote_sessions_require_a_port() {
        let session = SessionRecord {
            id: SessionId::new(),
            name: "production".into(),
            protocol: Protocol::Ssh,
            hostname: "prod.example.test".into(),
            port: 0,
            username: Some("ops".into()),
            auth: AuthMethod::Agent,
            known_hosts_path: None,
            pinned_fingerprint: None,
            folder: None,
            tags: vec!["production".into()],
            favorite: false,
            startup_directory: None,
            startup_command: None,
            environment: Vec::new(),
            jump_hosts: Vec::new(),
            notes: None,
        };

        assert_eq!(session.validate(), Err(SessionValidationError::MissingPort));
    }
}
