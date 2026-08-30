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
    AuthMethod, JumpHostRecord, MAX_SERVER_ALIVE_INTERVAL_SECONDS, Protocol, RemoteDesktopProfile,
    SerialProfile, SessionId, SessionRecord, SessionValidationError, TelnetProfile,
};
pub use settings::{
    AppSettings, AppearanceSettings, GeneralSettings, NetworkSettings, SettingsValidationError,
    SshSettings, TerminalSettings, ThemePreference,
};
pub use snippet::{SnippetRecord, SnippetValidationError};
pub use terminal::{OutputBatcher, OutputChunk};
pub use transfer::{TransferEvent, TransferLifecycle, TransferState};
