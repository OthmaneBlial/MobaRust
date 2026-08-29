use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The only lifecycle states a connection adapter may expose to the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionState {
    Created,
    Resolving,
    Connecting,
    Authenticating,
    Connected,
    Reconnecting,
    Disconnecting,
    Disconnected,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionEvent {
    Resolve,
    BeginConnect,
    BeginAuthentication,
    AuthenticationSucceeded,
    ConnectionLost,
    BeginReconnect,
    DisconnectRequested,
    Disconnected,
    Cancel,
    Fail,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("cannot apply {event:?} while connection is {state:?}")]
pub struct TransitionError {
    pub state: ConnectionState,
    pub event: ConnectionEvent,
}

/// Explicit state transitions make impossible combinations of booleans hard
/// to represent and give every adapter one predictable contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionLifecycle {
    state: ConnectionState,
    revision: u64,
}

impl Default for ConnectionLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectionLifecycle {
    pub const fn new() -> Self {
        Self {
            state: ConnectionState::Created,
            revision: 0,
        }
    }

    pub const fn state(&self) -> ConnectionState {
        self.state
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub fn apply(&mut self, event: ConnectionEvent) -> Result<ConnectionState, TransitionError> {
        let next = match (self.state, &event) {
            (ConnectionState::Created, ConnectionEvent::Resolve) => ConnectionState::Resolving,
            (ConnectionState::Created, ConnectionEvent::BeginConnect)
            | (ConnectionState::Resolving, ConnectionEvent::BeginConnect) => {
                ConnectionState::Connecting
            }
            (ConnectionState::Connecting, ConnectionEvent::BeginAuthentication) => {
                ConnectionState::Authenticating
            }
            (ConnectionState::Authenticating, ConnectionEvent::AuthenticationSucceeded) => {
                ConnectionState::Connected
            }
            (ConnectionState::Connected, ConnectionEvent::ConnectionLost) => {
                ConnectionState::Reconnecting
            }
            (ConnectionState::Reconnecting, ConnectionEvent::BeginReconnect) => {
                ConnectionState::Connecting
            }
            (ConnectionState::Connected, ConnectionEvent::DisconnectRequested)
            | (ConnectionState::Connecting, ConnectionEvent::DisconnectRequested)
            | (ConnectionState::Authenticating, ConnectionEvent::DisconnectRequested)
            | (ConnectionState::Reconnecting, ConnectionEvent::DisconnectRequested) => {
                ConnectionState::Disconnecting
            }
            (ConnectionState::Disconnecting, ConnectionEvent::Disconnected) => {
                ConnectionState::Disconnected
            }
            (ConnectionState::Created, ConnectionEvent::Cancel)
            | (ConnectionState::Resolving, ConnectionEvent::Cancel)
            | (ConnectionState::Connecting, ConnectionEvent::Cancel)
            | (ConnectionState::Authenticating, ConnectionEvent::Cancel)
            | (ConnectionState::Reconnecting, ConnectionEvent::Cancel) => {
                ConnectionState::Cancelled
            }
            (ConnectionState::Connecting, ConnectionEvent::Fail)
            | (ConnectionState::Authenticating, ConnectionEvent::Fail)
            | (ConnectionState::Reconnecting, ConnectionEvent::Fail) => ConnectionState::Failed,
            _ => {
                return Err(TransitionError {
                    state: self.state,
                    event,
                });
            }
        };

        self.state = next;
        self.revision = self.revision.saturating_add(1);
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_connection_follows_the_happy_path() {
        let mut lifecycle = ConnectionLifecycle::new();
        for event in [
            ConnectionEvent::Resolve,
            ConnectionEvent::BeginConnect,
            ConnectionEvent::BeginAuthentication,
            ConnectionEvent::AuthenticationSucceeded,
        ] {
            lifecycle.apply(event).unwrap();
        }

        assert_eq!(lifecycle.state(), ConnectionState::Connected);
        assert_eq!(lifecycle.revision(), 4);
    }

    #[test]
    fn illegal_transitions_are_rejected_without_mutation() {
        let mut lifecycle = ConnectionLifecycle::new();
        let error = lifecycle
            .apply(ConnectionEvent::AuthenticationSucceeded)
            .unwrap_err();

        assert_eq!(error.state, ConnectionState::Created);
        assert_eq!(lifecycle.revision(), 0);
    }

    #[test]
    fn a_lost_connection_can_reconnect_then_authenticate_again() {
        let mut lifecycle = ConnectionLifecycle::new();
        for event in [
            ConnectionEvent::BeginConnect,
            ConnectionEvent::BeginAuthentication,
            ConnectionEvent::AuthenticationSucceeded,
            ConnectionEvent::ConnectionLost,
            ConnectionEvent::BeginReconnect,
            ConnectionEvent::BeginAuthentication,
            ConnectionEvent::AuthenticationSucceeded,
        ] {
            lifecycle.apply(event).unwrap();
        }

        assert_eq!(lifecycle.state(), ConnectionState::Connected);
    }
}
