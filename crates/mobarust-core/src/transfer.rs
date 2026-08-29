use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferState {
    Queued,
    Preparing,
    Running,
    Paused,
    Cancelling,
    Cancelled,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferEvent {
    Prepare,
    Start,
    Pause,
    Resume,
    CancelRequested,
    Cancelled,
    Complete,
    Fail,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[error("cannot apply transfer event {event:?} while transfer is {state:?}")]
pub struct TransferTransitionError {
    pub state: TransferState,
    pub event: TransferEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferLifecycle {
    state: TransferState,
}

impl Default for TransferLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl TransferLifecycle {
    pub const fn new() -> Self {
        Self {
            state: TransferState::Queued,
        }
    }

    pub const fn state(&self) -> TransferState {
        self.state
    }

    pub fn apply(
        &mut self,
        event: TransferEvent,
    ) -> Result<TransferState, TransferTransitionError> {
        let next = match (self.state, event) {
            (TransferState::Queued, TransferEvent::Prepare) => TransferState::Preparing,
            (TransferState::Preparing, TransferEvent::Start) => TransferState::Running,
            (TransferState::Running, TransferEvent::Pause) => TransferState::Paused,
            (TransferState::Paused, TransferEvent::Resume) => TransferState::Running,
            (TransferState::Running | TransferState::Paused, TransferEvent::CancelRequested) => {
                TransferState::Cancelling
            }
            (TransferState::Cancelling, TransferEvent::Cancelled) => TransferState::Cancelled,
            (TransferState::Running, TransferEvent::Complete) => TransferState::Completed,
            (TransferState::Preparing | TransferState::Running, TransferEvent::Fail) => {
                TransferState::Failed
            }
            _ => {
                return Err(TransferTransitionError {
                    state: self.state,
                    event,
                });
            }
        };

        self.state = next;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_is_explicit_and_terminal() {
        let mut lifecycle = TransferLifecycle::new();
        lifecycle.apply(TransferEvent::Prepare).unwrap();
        lifecycle.apply(TransferEvent::Start).unwrap();
        lifecycle.apply(TransferEvent::CancelRequested).unwrap();
        lifecycle.apply(TransferEvent::Cancelled).unwrap();

        assert_eq!(lifecycle.state(), TransferState::Cancelled);
        assert!(lifecycle.apply(TransferEvent::Resume).is_err());
    }
}
