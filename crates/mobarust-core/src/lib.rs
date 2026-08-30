//! Protocol-neutral primitives shared by the MobaRust desktop shell and future
//! SSH/SFTP adapters. Secrets intentionally never appear in these types.

pub mod audit;
pub mod lifecycle;
pub mod r#macro;
pub mod session;
pub mod settings;
pub mod snippet;
pub mod terminal;
pub mod transfer;

pub use audit::{AuditEvent, AuditEventKind};
pub use lifecycle::{ConnectionEvent, ConnectionLifecycle, ConnectionState, TransitionError};
pub use r#macro::{
    MAX_MACRO_ACTIONS, MAX_MACRO_TEXT_BYTES, MAX_MACRO_WAIT_MILLISECONDS, MacroAction,
    MacroApprovalPolicy, MacroKey, MacroRecord, MacroValidationError,
};
pub use session::{
    AuthMethod, DEFAULT_VNC_QUALITY, JumpHostRecord, MAX_SERVER_ALIVE_INTERVAL_SECONDS,
    MAX_SESSION_ENVIRONMENT_ENTRIES, MAX_SESSION_ENVIRONMENT_NAME_BYTES,
    MAX_SESSION_ENVIRONMENT_TOTAL_BYTES, MAX_SESSION_ENVIRONMENT_VALUE_BYTES,
    MAX_SESSION_STARTUP_COMMAND_BYTES, MAX_SESSION_STARTUP_DIRECTORY_BYTES, Protocol,
    RdpGatewayProfile, RemoteDesktopProfile, SerialProfile, SessionId, SessionRecord,
    SessionValidationError, TelnetProfile, validate_session_environment, validate_session_startup,
};
pub use settings::{
    AppSettings, AppearanceSettings, GeneralSettings, KeyboardSettings, NetworkSettings,
    SettingsValidationError, SshSettings, TerminalSettings, ThemePreference,
};
pub use snippet::{SnippetRecord, SnippetValidationError};
pub use terminal::{
    MAX_TERMINAL_INPUT_BYTES, OutputBatcher, OutputChunk, TerminalInputError,
    validate_terminal_input,
};
pub use transfer::{TransferEvent, TransferLifecycle, TransferState};
