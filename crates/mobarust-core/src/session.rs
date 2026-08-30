use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const MAX_SERVER_ALIVE_INTERVAL_SECONDS: u64 = 86_400;

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
pub struct TelnetProfile {
    pub terminal: String,
    pub encoding: String,
    pub columns: u16,
    pub rows: u16,
}

impl TelnetProfile {
    pub fn validate(&self) -> Result<(), SessionValidationError> {
        if self.terminal.trim().is_empty()
            || self.terminal.len() > 128
            || self.terminal.chars().any(char::is_control)
            || !matches!(self.encoding.as_str(), "utf-8" | "windows-1252")
            || self.columns == 0
            || self.rows == 0
        {
            return Err(SessionValidationError::InvalidTelnetProfile);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteDesktopProfile {
    pub domain: Option<String>,
    pub width: u16,
    pub height: u16,
    pub color_depth: u16,
    pub audio_enabled: bool,
}

impl RemoteDesktopProfile {
    pub fn validate(&self) -> Result<(), SessionValidationError> {
        if !(320..=16_384).contains(&self.width)
            || !(200..=16_384).contains(&self.height)
            || self.color_depth == 0
            || self
                .domain
                .as_deref()
                .is_some_and(|domain| domain.chars().any(char::is_control))
        {
            return Err(SessionValidationError::InvalidRemoteDesktopProfile);
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
    /// SSH keepalive interval in seconds for this hop. `None` or `Some(0)`
    /// disables it.
    #[serde(default)]
    pub server_alive_interval: Option<u64>,
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
    pub last_used_at: Option<u64>,
    #[serde(default)]
    pub known_hosts_path: Option<String>,
    #[serde(default)]
    pub pinned_fingerprint: Option<String>,
    /// Explicit local X11 display target. The native SSH layer validates and
    /// uses it; it is never inferred from DISPLAY or Xauthority.
    #[serde(default)]
    pub x11_display: Option<String>,
    #[serde(default)]
    pub x11_single_connection: bool,
    /// SSH keepalive interval in seconds. `None` or `Some(0)` disables it.
    #[serde(default)]
    pub server_alive_interval: Option<u64>,
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
    #[serde(default)]
    pub telnet_profile: Option<TelnetProfile>,
    #[serde(default)]
    pub remote_desktop_profile: Option<RemoteDesktopProfile>,
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
    #[error("saved authentication reference cannot be empty")]
    EmptyAuthReference,
    #[error("saved private-key reference is invalid")]
    InvalidKeyReference,
    #[error("serial profile is invalid")]
    InvalidSerialProfile,
    #[error("Telnet profile is invalid")]
    InvalidTelnetProfile,
    #[error("remote desktop profile is invalid")]
    InvalidRemoteDesktopProfile,
    #[error("X11 display target is invalid")]
    InvalidX11Display,
    #[error("SSH server-alive interval is invalid")]
    InvalidServerAliveInterval,
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
            last_used_at: None,
            known_hosts_path: None,
            pinned_fingerprint: None,
            x11_display: None,
            x11_single_connection: false,
            server_alive_interval: None,
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
            telnet_profile: None,
            remote_desktop_profile: None,
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
        validate_auth_method(&self.auth)?;
        for jump_host in &self.jump_host_profiles {
            validate_auth_method(&jump_host.auth)?;
            if jump_host
                .server_alive_interval
                .is_some_and(|seconds| seconds > MAX_SERVER_ALIVE_INTERVAL_SECONDS)
            {
                return Err(SessionValidationError::InvalidServerAliveInterval);
            }
        }
        match (self.protocol, &self.serial_profile) {
            (Protocol::Serial, Some(profile)) => profile.validate()?,
            (Protocol::Serial, None) => return Err(SessionValidationError::InvalidSerialProfile),
            (_, Some(profile)) => profile.validate()?,
            (_, None) => {}
        }
        match (self.protocol, &self.telnet_profile) {
            (Protocol::Telnet, Some(profile)) => profile.validate()?,
            (Protocol::Telnet, None) => return Err(SessionValidationError::InvalidTelnetProfile),
            (_, Some(profile)) => profile.validate()?,
            (_, None) => {}
        }
        if let Some(profile) = &self.remote_desktop_profile {
            profile.validate()?
        }
        if self.x11_display.as_deref().is_some_and(|display| {
            display.trim().is_empty() || display.chars().any(char::is_control)
        }) {
            return Err(SessionValidationError::InvalidX11Display);
        }
        if self
            .server_alive_interval
            .is_some_and(|seconds| seconds > MAX_SERVER_ALIVE_INTERVAL_SECONDS)
        {
            return Err(SessionValidationError::InvalidServerAliveInterval);
        }
        Ok(())
    }
}

fn validate_auth_method(auth: &AuthMethod) -> Result<(), SessionValidationError> {
    let has_control = |value: &str| value.chars().any(char::is_control);
    match auth {
        AuthMethod::None | AuthMethod::Agent => {}
        AuthMethod::Password { credential_ref }
        | AuthMethod::KeyboardInteractive { credential_ref }
            if credential_ref.trim().is_empty() =>
        {
            return Err(SessionValidationError::EmptyAuthReference);
        }
        AuthMethod::Password { credential_ref }
        | AuthMethod::KeyboardInteractive { credential_ref }
            if has_control(credential_ref) =>
        {
            return Err(SessionValidationError::EmptyAuthReference);
        }
        AuthMethod::PrivateKey {
            key_ref,
            credential_ref,
        } => {
            if key_ref.trim().is_empty() || has_control(key_ref) {
                return Err(SessionValidationError::InvalidKeyReference);
            }
            if credential_ref
                .as_deref()
                .is_some_and(|value| value.trim().is_empty() || has_control(value))
            {
                return Err(SessionValidationError::EmptyAuthReference);
            }
        }
        _ => {}
    }
    Ok(())
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
            last_used_at: None,
            known_hosts_path: None,
            pinned_fingerprint: None,
            x11_display: None,
            x11_single_connection: false,
            server_alive_interval: None,
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
            telnet_profile: None,
            remote_desktop_profile: None,
        };

        assert_eq!(session.validate(), Err(SessionValidationError::MissingPort));
    }

    #[test]
    fn server_alive_interval_is_bounded() {
        let mut session = SessionRecord::local_terminal("local");
        session.server_alive_interval = Some(MAX_SERVER_ALIVE_INTERVAL_SECONDS + 1);

        assert_eq!(
            session.validate(),
            Err(SessionValidationError::InvalidServerAliveInterval)
        );

        let mut jump_session = SessionRecord::local_terminal("local");
        jump_session.jump_host_profiles = vec![JumpHostRecord {
            host: "127.0.0.1".into(),
            port: 22,
            username: "fixture".into(),
            auth: AuthMethod::Agent,
            known_hosts_path: None,
            pinned_fingerprint: None,
            server_alive_interval: Some(MAX_SERVER_ALIVE_INTERVAL_SECONDS + 1),
        }];
        assert_eq!(
            jump_session.validate(),
            Err(SessionValidationError::InvalidServerAliveInterval)
        );
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

    #[test]
    fn telnet_profile_accepts_only_bounded_wire_choices() {
        let profile = TelnetProfile {
            terminal: "xterm-256color".into(),
            encoding: "utf-8".into(),
            columns: 120,
            rows: 32,
        };
        profile.validate().unwrap();
        let mut invalid = profile;
        invalid.encoding = "shell-command".into();
        assert_eq!(
            invalid.validate(),
            Err(SessionValidationError::InvalidTelnetProfile)
        );
    }

    #[test]
    fn remote_desktop_profile_rejects_unsafe_dimensions() {
        let profile = RemoteDesktopProfile {
            domain: Some("LAB".into()),
            width: 1280,
            height: 800,
            color_depth: 32,
            audio_enabled: false,
        };
        profile.validate().unwrap();

        let invalid = RemoteDesktopProfile {
            width: 319,
            ..profile
        };
        assert_eq!(
            invalid.validate(),
            Err(SessionValidationError::InvalidRemoteDesktopProfile)
        );
    }

    #[test]
    fn saved_authentication_references_are_non_empty_and_control_free() {
        let mut session = SessionRecord::local_terminal("auth validation");
        session.protocol = Protocol::Ssh;
        session.hostname = "ssh.example.test".into();
        session.port = 22;
        session.auth = AuthMethod::KeyboardInteractive {
            credential_ref: "ops-response".into(),
        };
        session.validate().unwrap();

        session.auth = AuthMethod::Password {
            credential_ref: "   ".into(),
        };
        assert_eq!(
            session.validate(),
            Err(SessionValidationError::EmptyAuthReference)
        );

        session.auth = AuthMethod::PrivateKey {
            key_ref: "private\nkey".into(),
            credential_ref: None,
        };
        assert_eq!(
            session.validate(),
            Err(SessionValidationError::InvalidKeyReference)
        );
    }
}
