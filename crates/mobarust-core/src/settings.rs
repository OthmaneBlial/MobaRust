use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemePreference {
    #[default]
    Dark,
    Light,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneralSettings {
    #[serde(default)]
    pub theme: ThemePreference,
    #[serde(default = "default_true")]
    pub confirm_multiline_paste: bool,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            theme: ThemePreference::default(),
            confirm_multiline_paste: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceSettings {
    #[serde(default = "default_font_size")]
    pub font_size: u16,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            font_size: default_font_size(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSettings {
    #[serde(default = "default_scrollback")]
    pub scrollback_lines: u32,
    #[serde(default = "default_true")]
    pub cursor_blink: bool,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            scrollback_lines: default_scrollback(),
            cursor_blink: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshSettings {
    #[serde(default = "default_true")]
    pub reconnect_enabled: bool,
    #[serde(default = "default_reconnect_attempts")]
    pub reconnect_attempts: u8,
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_ms: u64,
}

impl Default for SshSettings {
    fn default() -> Self {
        Self {
            reconnect_enabled: true,
            reconnect_attempts: default_reconnect_attempts(),
            connect_timeout_ms: default_connect_timeout(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkSettings {
    #[serde(default = "default_diagnostic_timeout")]
    pub diagnostic_timeout_ms: u64,
    #[serde(default = "default_scan_concurrency")]
    pub scan_concurrency: u16,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self {
            diagnostic_timeout_ms: default_diagnostic_timeout(),
            scan_concurrency: default_scan_concurrency(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(default)]
    pub general: GeneralSettings,
    #[serde(default)]
    pub appearance: AppearanceSettings,
    #[serde(default)]
    pub terminal: TerminalSettings,
    #[serde(default)]
    pub ssh: SshSettings,
    #[serde(default)]
    pub network: NetworkSettings,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SettingsValidationError {
    #[error("terminal font size must be between 8 and 32")]
    InvalidFontSize,
    #[error("terminal scrollback must be between 100 and 100000 lines")]
    InvalidScrollback,
    #[error("SSH reconnect attempts must be between 0 and 10")]
    InvalidReconnectAttempts,
    #[error("SSH connect timeout must be between 100 and 60000 milliseconds")]
    InvalidConnectTimeout,
    #[error("diagnostic timeout must be between 50 and 60000 milliseconds")]
    InvalidDiagnosticTimeout,
    #[error("scan concurrency must be between 1 and 128")]
    InvalidScanConcurrency,
}

impl AppSettings {
    pub fn validate(&self) -> Result<(), SettingsValidationError> {
        if !(8..=32).contains(&self.appearance.font_size) {
            return Err(SettingsValidationError::InvalidFontSize);
        }
        if !(100..=100_000).contains(&self.terminal.scrollback_lines) {
            return Err(SettingsValidationError::InvalidScrollback);
        }
        if self.ssh.reconnect_attempts > 10 {
            return Err(SettingsValidationError::InvalidReconnectAttempts);
        }
        if !(100..=60_000).contains(&self.ssh.connect_timeout_ms) {
            return Err(SettingsValidationError::InvalidConnectTimeout);
        }
        if !(50..=60_000).contains(&self.network.diagnostic_timeout_ms) {
            return Err(SettingsValidationError::InvalidDiagnosticTimeout);
        }
        if !(1..=128).contains(&self.network.scan_concurrency) {
            return Err(SettingsValidationError::InvalidScanConcurrency);
        }
        Ok(())
    }
}

fn default_true() -> bool {
    true
}

fn default_font_size() -> u16 {
    13
}

fn default_scrollback() -> u32 {
    5_000
}

fn default_reconnect_attempts() -> u8 {
    3
}

fn default_connect_timeout() -> u64 {
    12_000
}

fn default_diagnostic_timeout() -> u64 {
    1_500
}

fn default_scan_concurrency() -> u16 {
    32
}

#[cfg(test)]
mod tests {
    use super::{AppSettings, SettingsValidationError};

    #[test]
    fn defaults_are_safe_and_valid() {
        let settings = AppSettings::default();
        settings.validate().unwrap();
        assert!(settings.general.confirm_multiline_paste);
        assert_eq!(settings.terminal.scrollback_lines, 5_000);
    }

    #[test]
    fn unsafe_ranges_are_rejected() {
        let mut settings = AppSettings::default();
        settings.appearance.font_size = 100;
        assert_eq!(
            settings.validate(),
            Err(SettingsValidationError::InvalidFontSize)
        );
        settings = AppSettings::default();
        settings.network.scan_concurrency = 0;
        assert_eq!(
            settings.validate(),
            Err(SettingsValidationError::InvalidScanConcurrency)
        );
    }
}
