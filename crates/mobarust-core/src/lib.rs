//! Protocol-neutral primitives shared by the MobaRust desktop shell and future
//! SSH/SFTP adapters. Secrets intentionally never appear in these types.

pub mod lifecycle;
pub mod session;
pub mod settings;
pub mod terminal;
pub mod transfer;

pub use lifecycle::{ConnectionEvent, ConnectionLifecycle, ConnectionState, TransitionError};
pub use session::{
    AuthMethod, JumpHostRecord, Protocol, SerialProfile, SessionId, SessionRecord,
    SessionValidationError,
};
pub use settings::{
    AppSettings, AppearanceSettings, GeneralSettings, NetworkSettings, SettingsValidationError,
    SshSettings, TerminalSettings, ThemePreference,
};
pub use terminal::{OutputBatcher, OutputChunk};
pub use transfer::{TransferEvent, TransferLifecycle, TransferState};
