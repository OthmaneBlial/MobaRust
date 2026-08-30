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
    KeyboardInteractive {
        #[serde(rename = "credentialRef", alias = "credential_ref")]
        credential_ref: String,
    },
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
pub struct SerialProfile {
    pub device: String,
    pub baud_rate: u32,
    pub data_bits: String,
    pub stop_bits: String,
    pub parity: String,
    pub flow_control: String,
    pub line_ending: String,
}

impl SerialProfile {
    pub fn validate(&self) -> Result<(), SessionValidationError> {
        let valid_choice = |value: &str, choices: &[&str]| choices.contains(&value);
        if self.device.trim().is_empty()
            || self.device.contains('\0')
            || self.baud_rate == 0
            || !valid_choice(&self.data_bits, &["five", "six", "seven", "eight"])
            || !valid_choice(&self.stop_bits, &["one", "two"])
            || !valid_choice(&self.parity, &["none", "odd", "even"])
            || !valid_choice(&self.flow_control, &["none", "software", "hardware"])
            || !valid_choice(&self.line_ending, &["none", "cr-lf", "cr", "lf"])
        {
            return Err(SessionValidationError::InvalidSerialProfile);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JumpHostRecord {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: AuthMethod,
    #[serde(default)]
    pub known_hosts_path: Option<String>,
    #[serde(default)]
    pub pinned_fingerprint: Option<String>,
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
    #[serde(default)]
    pub jump_host_profiles: Vec<JumpHostRecord>,
    pub notes: Option<String>,
    #[serde(default)]
    pub serial_profile: Option<SerialProfile>,
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
    #[error("serial profile is invalid")]
    InvalidSerialProfile,
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
            jump_host_profiles: Vec::new(),
            notes: None,
            serial_profile: None,
        }
    }

    pub fn validate(&self) -> Result<(), SessionValidationError> {
        if self.name.trim().is_empty() {
            return Err(SessionValidationError::EmptyName);
        }
        if self.hostname.trim().is_empty() {
            return Err(SessionValidationError::EmptyHostname);
        }
        if !matches!(self.protocol, Protocol::Local | Protocol::Serial) && self.port == 0 {
            return Err(SessionValidationError::MissingPort);
        }
        if self.tags.iter().any(|tag| tag.trim().is_empty()) {
            return Err(SessionValidationError::EmptyTag);
        }
        match (self.protocol, &self.serial_profile) {
            (Protocol::Serial, Some(profile)) => profile.validate()?,
            (Protocol::Serial, None) => return Err(SessionValidationError::InvalidSerialProfile),
            (_, Some(profile)) => profile.validate()?,
            (_, None) => {}
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
            jump_host_profiles: Vec::new(),
            notes: None,
            serial_profile: None,
        };

        assert_eq!(session.validate(), Err(SessionValidationError::MissingPort));
    }

    #[test]
    fn serial_profile_accepts_only_bounded_wire_choices() {
        let profile = SerialProfile {
            device: "/dev/tty.test".into(),
            baud_rate: 115_200,
            data_bits: "eight".into(),
            stop_bits: "one".into(),
            parity: "none".into(),
            flow_control: "none".into(),
            line_ending: "cr-lf".into(),
        };
        profile.validate().unwrap();
        let mut invalid = profile;
        invalid.line_ending = "shell-command".into();
        assert_eq!(
            invalid.validate(),
            Err(SessionValidationError::InvalidSerialProfile)
        );
    }
}
