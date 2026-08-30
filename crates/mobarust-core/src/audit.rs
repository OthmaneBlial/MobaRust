//! Secret-free local audit event types.
//!
//! Audit events intentionally contain only lifecycle facts. They do not
//! contain terminal commands, remote paths, hostnames, usernames, errors, or
//! credential references. The desktop store can therefore persist useful
//! connection history without turning the audit file into a transcript or a
//! second session database.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Protocol, SessionId};

/// The event kinds that may be retained in the local audit history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuditEventKind {
    SessionOpened,
    ConnectionSucceeded,
    ConnectionFailed,
    Disconnected,
    TransferCompleted,
    TransferFailed,
}

/// A deliberately narrow, secret-free audit record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEvent {
    pub id: Uuid,
    pub timestamp: u64,
    pub kind: AuditEventKind,
    pub session_id: Option<SessionId>,
    pub protocol: Option<Protocol>,
}

impl AuditEvent {
    pub fn new(
        kind: AuditEventKind,
        session_id: Option<SessionId>,
        protocol: Option<Protocol>,
        timestamp: u64,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp,
            kind,
            session_id,
            protocol,
        }
    }
}
